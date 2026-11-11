#!/usr/bin/env python3
"""One-session reference benchmark. See README.md for boundaries and schema."""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import statistics
import subprocess
import sys
import threading
import time
import traceback
import urllib.request

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
PI_REV = '1cedd32724abfcb0915f76cc61b6827e2c16dbad'
RPI_REV = '4056176bf4867f913099b101fd65b773895c99ba'
RPI_SHA = '4762ec079d86ce548d38a2840b2970ac6f8b093ada210692479b0c384b7a7017'
PROMPT, RESPONSE = 'turn growth', 'ok'
COUNTERS = ['cpu_ms', 'wchar', 'write_bytes']


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(path, *args):
    return subprocess.check_output(['git', '-C', str(path), *args], text=True).strip()


def footprint(root):
    files = []
    for path in sorted(root.rglob('*')):
        if path.is_file():
            info = path.stat()
            files.append({'path': str(path.relative_to(root)), 'logical_bytes': info.st_size,
                          'allocated_bytes': info.st_blocks * 512})
    return {'session_files': files, 'session_bytes': sum(r['logical_bytes'] for r in files),
            'session_allocated_bytes': sum(r['allocated_bytes'] for r in files)}


def sample(pid, sessions):
    root = Path('/proc') / str(pid)
    raw = {name: (root / name).read_text() for name in ('stat', 'io', 'smaps_rollup')}
    fields = raw['stat'].rsplit(')', 1)[1].split()
    io = {k: int(v) for k, v in (line.split(':') for line in raw['io'].splitlines())}
    mem = {k: int(v.split()[0]) * 1024 for k, v in
           (line.split(':') for line in raw['smaps_rollup'].splitlines()[1:])}
    result = {'monotonic_ns': time.monotonic_ns(), 'epoch_ns': time.time_ns(),
              'cpu_ms': (int(fields[11]) + int(fields[12])) * 1000 / os.sysconf('SC_CLK_TCK'),
              'rss_bytes': mem['Rss'], 'pss_bytes': mem['Pss'], 'wchar': io['wchar'],
              'write_bytes': io['write_bytes'], 'cancelled_write_bytes': io['cancelled_write_bytes'],
              'raw_proc': raw, **footprint(sessions)}
    dbs = [r for r in result['session_files'] if r['path'].endswith('.db')]
    result['sqlite_main_bytes'] = sum(r['logical_bytes'] for r in dbs)
    result['sqlite_wal_bytes'] = sum(r['logical_bytes'] for r in result['session_files'] if r['path'].endswith('.db-wal'))
    return result


def delta(before, after):
    return {k: after[k] - before[k] for k in COUNTERS}


def quantile(values, fraction):
    return sorted(values)[min(len(values) - 1, int(len(values) * fraction))]


def regress(rows, keys):
    result = {}
    xs = [r['turn_midpoint'] for r in rows]
    xbar = statistics.mean(xs)
    xx = sum((x - xbar) ** 2 for x in xs)
    for key in keys:
        ys = [r[key] for r in rows]
        ybar = statistics.mean(ys)
        slope = sum((x - xbar) * (y - ybar) for x, y in zip(xs, ys)) / xx if xx else None
        residual = sum((y - (ybar + slope * (x - xbar))) ** 2 for x, y in zip(xs, ys)) if xx else 0
        total = sum((y - ybar) ** 2 for y in ys)
        result[key] = {'slope_per_history_turn': slope, 'intercept': ybar - slope * xbar if xx else None,
                       'r_squared': 1 - residual / total if total and xx else None}
    return result


class Child:
    def __init__(self, argv, cwd, env, log):
        self.queue = queue.Queue()
        self.log = log.open('w')
        self.start_ns = time.monotonic_ns()
        self.proc = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=self.log, text=True, bufsize=1, start_new_session=True)
        self.argv = argv
        self.request_id = 0
        self.thread = threading.Thread(target=self.read, daemon=True)
        self.thread.start()

    def read(self):
        try:
            for line in self.proc.stdout:
                arrived = time.monotonic_ns()
                self.queue.put((arrived, json.loads(line)))
        except Exception as e:
            self.queue.put((None, e))
        finally:
            self.queue.put((None, EOFError(f'Child exited: {self.argv}')))

    def frame(self):
        timestamp, value = self.queue.get(timeout=60)
        if isinstance(value, Exception):
            raise value
        return timestamp, value

    def send(self, kind, **kwargs):
        self.request_id += 1
        self.proc.stdin.write(json.dumps({'type': kind, 'id': self.request_id, **kwargs}) + '\n')
        self.proc.stdin.flush()
        return self.request_id

    def response(self, request_id, refused=None):
        while True:
            arrived, frame = self.frame()
            if frame.get('type') == 'response' and frame.get('id') == request_id:
                if refused and not frame['success'] and frame.get('error') == refused:
                    return arrived, None
                assert frame['success'], frame
                return arrived, frame['data']

    def request(self, kind, **kwargs):
        return self.response(self.send(kind, **kwargs))[1]

    def terminate(self):
        if self.proc.poll() is None:
            os.killpg(self.proc.pid, signal.SIGKILL)
            self.proc.wait()
        self.thread.join(timeout=5)
        self.log.close()


class Product:
    def __init__(self, a, engine, scenario, root, base, env):
        self.a, self.engine, self.scenario, self.root, self.base, self.env = a, engine, scenario, root, base, env
        self.generation = 0
        self.prompt_retries = 0
        self.sessions = root / 'storage'
        self.workspace = root / 'workspace'
        self.sessions.mkdir()
        self.workspace.mkdir()
        (self.workspace / 'fixture.txt').write_text('fixed tool result\n')
        if engine == 'pi-rust':
            config = root / 'config'
            config.mkdir()
            write(config / 'models.json', {'providers': {'fixture': {
                'api': 'openai-completions', 'baseUrl': base + '/v1', 'apiKey': 'fixture',
                'models': [{'id': 'turn-growth', 'name': 'turn-growth', 'reasoning': False,
                            'input': ['text'], 'contextWindow': 1000000, 'maxTokens': 4096}]}}})
            self.env = env | {'RPI_CODING_AGENT_DIR': str(config), 'RPI_OFFLINE': '1'}

    def start(self, resume=False):
        self.generation += 1
        if self.engine == 'pi-rust':
            argv = [str(self.a.rpi_binary), '--mode', 'rpc', '--offline', '--model', 'fixture/turn-growth',
                    '--thinking', 'off', '--session-dir', str(self.sessions)]
            if resume:
                candidates = [p for p in self.sessions.rglob('*.jsonl') if not p.name.endswith('.progress.jsonl')]
                assert len(candidates) == 1, candidates
                argv += ['--session', str(candidates[0])]
        else:
            argv = [shutil.which('node'), '--conditions=source', '--import', str(HERE / 'pi-imports.mjs'),
                    str(HERE / 'pi-worker.mjs'), str(self.sessions), self.base, self.scenario]
        argv = ['taskset', '-c', self.a.product_cpus, *argv]
        self.child = Child(argv, self.workspace, self.env, self.root / f'process-{self.generation}.stderr')
        before_open = None
        if self.engine == 'pi-rust':
            arrived, state = self.child.response(self.child.send('get_state'))
            assert state['modelId'] == 'turn-growth' and state['provider'] == 'fixture', state
            info = {'state': state}
        else:
            _, frame = self.child.frame()
            assert frame['type'] == 'before_open', frame
            before_open = sample(self.child.proc.pid, self.sessions)
            arrived, info = self.child.frame()
            assert info['type'] == 'ready', info
        loaded = sample(self.child.proc.pid, self.sessions)
        self.ready = {'startup_wall_ms': (arrived - self.child.start_ns) / 1e6, 'loaded': loaded,
                      'before_open': before_open, 'info': info, 'argv': self.child.argv}
        return self.ready

    def turn(self, turn):
        began = time.monotonic_ns()
        rid = self.child.send('prompt', message=PROMPT)
        if self.engine == 'pi-rust':
            # pi-rust can emit agent_settled before it releases the run slot;
            # an immediate next prompt is then refused and must be repeated.
            while True:
                accepted_at, data = self.child.response(rid, refused='a run is already active')
                if data is not None:
                    break
                self.prompt_retries += 1
                time.sleep(.0005)
                rid = self.child.send('prompt', message=PROMPT)
            assert data['accepted'], data
            deltas, tool_ends, counts = [], 0, Counter()
            while True:
                arrived, frame = self.child.frame()
                counts[frame.get('type')] += 1
                if frame.get('type') == 'message_update' and frame.get('eventType') == 'text_delta':
                    deltas.append(frame['delta'])
                if frame.get('type') == 'tool_execution_end':
                    assert not frame.get('isError', False), frame
                    tool_ends += 1
                if frame.get('type') == 'agent_settled':
                    assert frame['outcome'] == 'completed' and not frame['pendingResume'], frame
                    assert counts['agent_end'] == 1, counts
                    break
            assert ''.join(deltas) == RESPONSE, deltas
            assert tool_ends == (1 if self.scenario == 'tool' else 0), tool_ends
            accepted_ms = (accepted_at - began) / 1e6
        else:
            arrived, frame = self.child.frame()
            assert frame['type'] == 'terminal' and frame['id'] == rid, frame
            accepted_ms = frame['accepted_ms']
        return {'turn': turn, 'settled_ms': (arrived - began) / 1e6, 'accepted_ms': accepted_ms}

    def verify(self, turns):
        if self.engine == 'pi-durable':
            rid = self.child.send('verify', turns=turns)
            _, frame = self.child.frame()
            assert frame['type'] == 'verified' and frame['id'] == rid, frame
            return frame
        # Full persisted transcript oracle; outside measured turn windows.
        candidates = [p for p in self.sessions.rglob('*.jsonl') if not p.name.endswith('.progress.jsonl')]
        assert len(candidates) == 1, candidates
        records = [json.loads(line) for line in candidates[0].read_text().splitlines()]
        messages = [r['message'] for r in records if r.get('type') == 'message']
        counts = Counter(m['role'] for m in messages)
        assert counts['user'] == turns, counts
        assert counts['assistant'] == turns * (2 if self.scenario == 'tool' else 1), counts
        assert counts['toolResult'] == (turns if self.scenario == 'tool' else 0), counts
        return {'counts': dict(counts), 'persisted_messages': len(messages)}

    def close(self):
        before = sample(self.child.proc.pid, self.sessions)
        began = time.monotonic_ns()
        if self.engine == 'pi-durable':
            rid = self.child.send('close')
            _, frame = self.child.frame()
            assert frame['type'] == 'closed' and frame['id'] == rid, frame
        self.child.proc.stdin.close()
        pid, status, usage = os.wait4(self.child.proc.pid, 0)
        self.child.proc.returncode = os.waitstatus_to_exitcode(status)
        assert self.child.proc.returncode == 0, self.child.proc.returncode
        self.child.thread.join(timeout=5)
        self.child.log.close()
        return {'close_wall_ms': (time.monotonic_ns() - began) / 1e6, 'before': before,
                'total_cpu_ms': (usage.ru_utime + usage.ru_stime) * 1000,
                'maxrss_bytes': usage.ru_maxrss * 1024, 'oublock': usage.ru_oublock,
                'closed': footprint(self.sessions)}


def scenario(a, engine, kind, out, env):
    out.mkdir()
    log = (out / 'provider.stderr').open('w')
    provider_argv = ['taskset', '-c', a.provider_cpus, sys.executable, str(HERE / 'provider.py'),
                     '--scenario', kind, '--turns', str(a.turns), '--output', str(out / 'provider.json')]
    provider = subprocess.Popen(provider_argv, stdout=subprocess.PIPE, stderr=log, text=True, env=env)
    product = None
    result = {'engine': engine, 'scenario': kind, 'accepted': False}
    try:
        ready = json.loads(provider.stdout.readline())
        base = f"http://127.0.0.1:{ready['port']}"
        product = Product(a, engine, kind, out, base, env)
        initial = product.start()
        write(out / 'startup.json', initial)
        rows, windows, reopen, closes = [], [], [], []
        time.sleep(a.settle)
        before = sample(product.child.proc.pid, product.sessions)
        for turn in range(1, a.turns + 1):
            rows.append(product.turn(turn))
            if turn % a.window != 0 and turn != a.turns:
                continue
            time.sleep(a.settle)
            after = sample(product.child.proc.pid, product.sessions)
            start = windows[-1]['turn_end'] + 1 if windows else 1
            subset = rows[start - 1:turn]
            n = len(subset)
            window = {'turn_start': start, 'turn_end': turn, 'turn_midpoint': (start + turn) / 2, 'turns': n,
                      **{k + '_per_turn': v / n for k, v in delta(before, after).items()},
                      'wall_ms_per_turn': statistics.mean(r['settled_ms'] for r in subset),
                      'wall_p50_ms': quantile([r['settled_ms'] for r in subset], .5),
                      'wall_p95_ms': quantile([r['settled_ms'] for r in subset], .95),
                      **{k: after[k] for k in ['rss_bytes', 'pss_bytes', 'session_bytes', 'session_allocated_bytes',
                                              'sqlite_main_bytes', 'sqlite_wal_bytes']},
                      'before': before, 'after': after, 'process_generation': product.generation}
            windows.append(window)
            write(out / 'windows.json', windows)
            write(out / 'turns.json', rows)
            print(f'{engine:10} {kind:4} {start:3}-{turn:<3} CPU {window["cpu_ms_per_turn"]:7.3f} ms '
                  f'wall {window["wall_ms_per_turn"]:7.3f} ms p95 {window["wall_p95_ms"]:7.3f} '
                  f'PSS {after["pss_bytes"]/1048576:7.2f} MiB wchar {window["wchar_per_turn"]/1024:8.2f} KiB '
                  f'write {window["write_bytes_per_turn"]/1024:8.2f} KiB', flush=True)
            if turn in a.reopen:
                verification = product.verify(turn)
                closes.append({'history_turns': turn, **product.close()})
                reopened = product.start(resume=True)
                assert product.verify(turn), 'resume oracle failed'
                reopen.append({'history_turns': turn, **reopened, 'verified': verification})
                write(out / 'reopen.json', reopen)
            before = sample(product.child.proc.pid, product.sessions)
        verification = product.verify(a.turns)
        closes.append({'history_turns': a.turns, **product.close()})
        write(out / 'closes.json', closes)
        # The control route may return the full provider report; drain after timing.
        with urllib.request.urlopen(urllib.request.Request(base + '/__control/shutdown', data=b''), timeout=90) as response:
            while response.read(1024 * 1024):
                pass
        assert provider.wait(timeout=90) == 0, 'provider oracle failed; see provider.stderr'
        provider_report = json.loads((out / 'provider.json').read_text())
        assert provider_report['ok'] and provider_report['requestCount'] == ready['steps']
        slopes = regress(windows, ['cpu_ms_per_turn', 'wall_ms_per_turn', 'wall_p50_ms', 'wall_p95_ms',
            'wchar_per_turn', 'write_bytes_per_turn', 'rss_bytes', 'pss_bytes', 'session_bytes'])
        result.update(accepted=True, windows=windows, reopen=reopen, closes=closes, slopes=slopes,
                      verification=verification, prompt_retries=product.prompt_retries, closed_footprint=footprint(product.sessions))
        write(out / 'result.json', result)
        return result
    except BaseException:
        result['error'] = traceback.format_exc()
        write(out / 'result.json', result)
        raise
    finally:
        if product and hasattr(product, 'child'):
            product.child.terminate()
        if provider.poll() is None:
            provider.terminate()
            provider.wait(timeout=10)
        log.close()


def main(a):
    a.output = a.output.resolve()
    a.output.mkdir(parents=True, exist_ok=False)
    for checkout, expected in [(a.pi_checkout, PI_REV), (a.rpi_checkout, RPI_REV)]:
        assert git(checkout, 'rev-parse', 'HEAD') == expected
        assert not git(checkout, 'diff', 'HEAD'), f'Tracked edits in {checkout}'
    assert sha(a.rpi_binary) == RPI_SHA, 'Use the authenticated stock 4056176b release binary, or update identity constants explicitly.'
    env = {k: v for k, v in os.environ.items() if not any(t in k.upper() for t in
           ('TOKEN', 'SECRET', 'CREDENTIAL', 'API_KEY')) and not k.startswith(('RPI_', 'REFERENCE_PI_', 'NODE_OPTIONS'))}
    env.update(REFERENCE_PI_CHECKOUT=str(a.pi_checkout), REFERENCE_PI_DEPS=str(a.pi_deps), TOKIO_WORKER_THREADS='4')
    input_paths = [*HERE.glob('*.py'), *HERE.glob('*.mjs'), REPO / 'benchmarks/runtime/turn-growth.py',
                   *sorted((REPO / 'test-support/fake-provider/src').rglob('*.py'))]
    dependency_versions = {}
    for name in ['openai', 'typebox', 'diff', 'esbuild', 'http-proxy-agent', 'https-proxy-agent']:
        path = a.pi_deps / 'node_modules' / name / 'package.json'
        dependency_versions[name] = {'version': json.loads(path.read_text())['version'], 'manifest_sha256': sha(path)}
    identity = {'pi_revision': PI_REV, 'pi_checkout': str(a.pi_checkout), 'pi_dependency_checkout': str(a.pi_deps),
                'pi_dependency_versions': dependency_versions, 'pi_lock_sha256': sha(a.pi_checkout / 'package-lock.json'),
                'rpi_revision': RPI_REV, 'rpi_binary': str(a.rpi_binary), 'rpi_binary_sha256': sha(a.rpi_binary),
                'rpi_profile': 'stock release: opt-level=3, thin LTO, codegen-units=16, strip=symbols; default features',
                'node': subprocess.check_output(['node', '--version'], text=True).strip(),
                'node_binary': shutil.which('node'), 'node_sha256': sha(Path(shutil.which('node'))),
                'python': sys.version, 'clock_ticks': os.sysconf('SC_CLK_TCK'), 'linux': list(os.uname()),
                'turns': a.turns, 'window': a.window, 'reopen_at': a.reopen, 'settle_seconds': a.settle,
                'product_cpus': a.product_cpus, 'provider_cpus': a.provider_cpus,
                'driver_cpus': sorted(os.sched_getaffinity(0)), 'tokio_workers': 4,
                'inputs': {str(p): sha(p) for p in input_paths},
                'reference_trees': {str(p): git(p, 'rev-parse', 'HEAD^{tree}') for p in [a.pi_checkout, a.rpi_checkout]},
                'scope': 'Only product PID /proc stat/io/smaps_rollup. Provider and controller are siblings. '
                         'Wall is parent submit -> received terminal event, including Pi run_end watch consumption. '
                         'Reopen uses a fresh process and warm OS file cache. No cache drop. '
                         'Window counters include the end settle interval; exclude close/reopen and verification.'}
    write(a.output / 'identity.json', identity)
    results = []
    for engine in a.engines:
        for kind in a.scenarios:
            results.append(scenario(a, engine, kind, a.output / f'{engine}-{kind}', env))
    write(a.output / 'result.json', {'accepted': True, 'results': results})


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--pi-checkout', type=Path, default=Path('/home/capi/Documents/codes/pi'))
    parser.add_argument('--pi-deps', type=Path, default=Path('/home/capi/.cache/rustx-pr485-request-hotpath-20261103/pi-1ced'))
    parser.add_argument('--rpi-checkout', type=Path, default=Path('/home/capi/Documents/codes/pi-rust'))
    parser.add_argument('--rpi-binary', type=Path, default=Path('/home/capi/.cache/rustx-four-harness-20261109/pi-rust-target/release/rpi'))
    parser.add_argument('--engines', nargs='+', choices=['pi-rust', 'pi-durable'], default=['pi-rust', 'pi-durable'])
    parser.add_argument('--scenarios', nargs='+', choices=['text', 'tool'], default=['text', 'tool'])
    parser.add_argument('--turns', type=int, default=512)
    parser.add_argument('--window', type=int, default=32)
    parser.add_argument('--reopen', nargs='*', type=int, default=[32, 128, 512])
    parser.add_argument('--settle', type=float, default=.25)
    parser.add_argument('--product-cpus', default='24-27')
    parser.add_argument('--provider-cpus', default='31')
    args = parser.parse_args()
    assert args.turns > 0 and args.window > 0 and args.settle >= 0
    main(args)
