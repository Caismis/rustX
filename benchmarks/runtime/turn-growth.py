#!/usr/bin/env python3
"""Deterministic, one-session App Server history-growth probe (Linux).

  python3 benchmarks/runtime/turn-growth.py --output /tmp/turn-growth

Builds release in the existing target directory; requires Python websockets.
The sibling fake provider uses the repository's real HTTP/SSE codecs. Only the
server PID is counted. Diagnostic tracing is opt-in and excluded from baseline.
"""
import argparse
import asyncio
import collections
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import signal
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.request

REPO = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('capacity', Path(__file__).with_name('app-server-capacity.py'))
CAPACITY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CAPACITY)
# A changed protocol must actually handshake; no legacy wire vocabulary/fallback.
VERSION = int(re.search(r'APP_SERVER_PROTOCOL_VERSION: u16 = (\d+)',
                       (REPO / 'src/app_server/protocol.rs').read_text())[1])
CAPACITY.PROTOCOL_VERSION = VERSION
PROMPT, RESPONSE = 'turn growth', 'ok'


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def sample(pid, runtime):
    root = Path('/proc') / str(pid)
    stat = (root / 'stat').read_text().rsplit(')', 1)[1].split()
    io = {k: int(v) for k, v in (line.split(':') for line in (root / 'io').read_text().splitlines())}
    mem = {k: int(v.split()[0]) * 1024 for k, v in
           (line.split(':') for line in (root / 'smaps_rollup').read_text().splitlines()[1:])}
    files = []
    for db in sorted(runtime.rglob('*.sqlite')):
        files.append({'path': str(db.relative_to(runtime)), **{
            key: path.stat().st_size if path.exists() else 0
            for key, path in [('main', db), ('wal', Path(str(db) + '-wal'))]}})
    return {'pid': pid, 'monotonic_ns': time.monotonic_ns(), 'epoch_ns': time.time_ns(),
            'cpu_ms': (int(stat[11]) + int(stat[12])) * 1000 / os.sysconf('SC_CLK_TCK'),
            'rss_bytes': mem['Rss'], 'pss_bytes': mem['Pss'],
            'wchar': io['wchar'], 'write_bytes': io['write_bytes'],
            'database_files': files,
            'sqlite_main_bytes': sum(row['main'] for row in files),
            'sqlite_wal_bytes': sum(row['wal'] for row in files)}


def delta(before, after):
    return {key: after[key] - before[key] for key in ['cpu_ms', 'wchar', 'write_bytes']}


def dbstat(runtime, pages=False):
    result = []
    for db in sorted(runtime.rglob('conversation.sqlite')):
        with sqlite3.connect(f'file:{db}?mode=ro', uri=True) as connection:
            item = {'path': str(db.relative_to(runtime)),
                'page_size': connection.execute('PRAGMA page_size').fetchone()[0],
                'page_count': connection.execute('PRAGMA page_count').fetchone()[0],
                'freelist_count': connection.execute('PRAGMA freelist_count').fetchone()[0],
                'owners': [dict(zip(['name', 'pages', 'bytes', 'payload', 'unused'], row))
                    for row in connection.execute('SELECT name,count(*),sum(pgsize),sum(payload),sum(unused) FROM dbstat GROUP BY name')],
                'rows': {name: connection.execute('SELECT count(*) FROM "' + name + '"').fetchone()[0]
                         for (name,) in connection.execute("SELECT name FROM sqlite_schema WHERE type='table'")}}
            if pages:
                item['page_owners'] = dict(connection.execute('SELECT pageno,name FROM dbstat'))
            result.append(item)
    return result


def regress(rows, keys):
    result = {}
    xs = [r['turn_midpoint'] for r in rows]
    for key in keys:
        ys = [r[key] for r in rows]
        xbar, ybar = statistics.mean(xs), statistics.mean(ys)
        xx = sum((x - xbar) ** 2 for x in xs)
        slope = sum((x - xbar) * (y - ybar) for x, y in zip(xs, ys)) / xx if xx else None
        residual = sum((y - (ybar + slope * (x - xbar))) ** 2 for x, y in zip(xs, ys)) if xx else 0
        total = sum((y - ybar) ** 2 for y in ys)
        result[key] = {'slope_per_history_turn': slope,
                       'intercept': ybar - slope * xbar if xx else None,
                       'r_squared': 1 - residual / total if total and xx else None}
    return result


async def provider_main(a):
    sys.path.insert(0, str(REPO / 'test-support/fake-provider/src'))
    from fake_provider.control import ScenarioRun
    from fake_provider.scenario import OPENAI_CHAT_COMPLETIONS, Expect, Finish, Scenario, Step, Stream, Text, ToolCall, Usage
    from fake_provider.server import ProviderServer
    expect = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model='turn-growth', body_contains=(PROMPT,))
    steps = []
    for i in range(a.turns):
        if a.provider == 'tool':
            steps.append(Step(expect, Stream(ToolCall(f'read-{i:08}', 'read', '{"path":"fixture.txt"}'), Finish('tool_calls'), Usage(16, 1))))
        steps.append(Step(expect, Stream(Text(RESPONSE), Finish(), Usage(16, 1))))
    state = ScenarioRun(Scenario('turn_growth_' + a.provider, *steps))
    server = ProviderServer(state)
    _, port = await server.start('127.0.0.1', 0)
    print(json.dumps({'port': port, 'steps': len(steps)}), flush=True)
    await server.serve_until_shutdown()
    # Provider bodies are quadratic across the run. Retain compact assertions,
    # lengths and hashes, rather than copying the complete report over HTTP.
    report = state.report()
    requests = []
    for i, request in enumerate(state.requests):
        body = request.json
        users = [m for m in body['messages'] if m['role'] == 'user']
        expected = i // (2 if a.provider == 'tool' else 1) + 1
        assert len(users) == expected, (i, len(users), expected)
        assert all(m['content'] == PROMPT or m['content'] == [{'type': 'text', 'text': PROMPT}] for m in users)
        tools = [m for m in body['messages'] if m['role'] == 'tool']
        assert len(tools) == (i // 2 + i % 2 if a.provider == 'tool' else 0), i
        assert all('fixed tool result' in json.dumps(m['content']) for m in tools), i
        encoded = request.body_text
        if isinstance(encoded, str):
            encoded = encoded.encode()
        requests.append({'index': i, 'body_bytes': len(encoded), 'sha256': hashlib.sha256(encoded).hexdigest(),
                         'messages': len(body['messages']), 'users': len(users),
                         'tools': sum(m['role'] == 'tool' for m in body['messages'])})
    write(a.provider_output, {'ok': report['ok'], 'requestCount': report['requestCount'],
                              'failures': report['failures'], 'requests': requests})
    return 0 if report['ok'] else 1


class Server:
    def __init__(self, a, out):
        self.a, self.out, self.generation = a, out, 0
        self.runtime = out / 'runtime'

    async def start(self):
        self.generation += 1
        prefix = self.out / f'server-{self.generation}'
        env = os.environ | {'RUSTX_CAPACITY_KEY': 'fixture', 'TOKIO_WORKER_THREADS': '4'}
        if self.a.cpu_profiler:
            env.update(LD_PRELOAD=str(self.a.cpu_profiler.resolve()),
                       CPUPROFILE=str(prefix) + '.cpu', CPUPROFILESIGNAL=str(int(signal.SIGUSR2)),
                       CPUPROFILE_FREQUENCY='500')
        argv = [str(self.a.binary.resolve()), 'app-server', '--listen', 'ws://127.0.0.1:0',
                '--config', str(self.out / 'rustx.toml'), '--runtime-root', str(self.runtime),
                '--token-file', str(self.out / 'token')]
        if self.a.strace:
            argv = ['strace', '-f', '-ttt', '-yy', '-xx', '-s', '128', '-e',
                    'trace=write,writev,pwrite64,pwritev,fsync,fdatasync', '-o', str(prefix) + '.strace', *argv]
        self.process = await asyncio.create_subprocess_exec(*argv, cwd=self.out, env=env,
            stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.PIPE)
        self.log = Path(str(prefix) + '.stderr').open('w')
        while True:
            raw = await asyncio.wait_for(self.process.stderr.readline(), 60)
            assert raw, 'server exited before readiness'
            line = raw.decode()
            self.log.write(line)
            self.log.flush()
            if line.startswith('rustx app-server listening '):
                endpoint = line.strip().split()[-1]
                break
        self.pid = self.process.pid
        if self.a.strace:
            children = Path(f'/proc/{self.pid}/task/{self.pid}/children').read_text().split()
            assert len(children) == 1, children
            self.pid = int(children[0])

        async def drain():
            async for line in self.process.stderr:
                self.log.write(line.decode())
        self.draining = asyncio.create_task(drain())
        self.client = await CAPACITY.Client.open(endpoint, Path(str(prefix) + '.client.jsonl'))
        return sample(self.pid, self.runtime)

    async def attach(self, thread):
        result = await self.client.call('thread/resume', {'threadId': thread})
        self.target = result['target']

    async def subscribe(self):
        checkpoint = await self.client.call('session/checkpoint', {'target': self.target, 'trace_records': []})
        await self.client.call('session/subscribe', {'target': self.target, 'after_cursor': checkpoint['checkpoint']['cursor']})

    async def stop(self):
        await self.client.close()
        os.kill(self.pid, signal.SIGTERM)
        assert await asyncio.wait_for(self.process.wait(), 60) == 0
        await self.draining
        self.log.close()


async def scenario(a, kind, out):
    out.mkdir()
    # Never put the workspace inside the repository/evidence tree: native
    # tool change recording would include growing evidence and repo artifacts.
    workspace_owner = tempfile.TemporaryDirectory(prefix='rustx-turn-growth-workspace-')
    workspace = Path(workspace_owner.name)
    (workspace / 'fixture.txt').write_text('fixed tool result\n')
    token = out / 'token'
    token.write_text(CAPACITY.TOKEN)
    token.chmod(0o600)
    provider_log = (out / 'provider.stderr').open('w')
    provider = await asyncio.create_subprocess_exec(sys.executable, str(Path(__file__).resolve()),
        '--provider', kind, '--turns', str(a.turns), '--provider-output', str(out / 'provider.json'),
        stdout=asyncio.subprocess.PIPE, stderr=provider_log)
    server = Server(a, out)
    rows, windows, reopen = [], [], []
    try:
        ready = json.loads(await asyncio.wait_for(provider.stdout.readline(), 30))
        base = f"http://127.0.0.1:{ready['port']}"
        (out / 'rustx.toml').write_text(f'''[agent]
[agent.tools]
builtin = ["read", "write", "edit", "glob", "grep", "bash", "job_status"]
[agent.model]
model = "fixture/turn-growth"
[providers.fixture]
protocol = "openai_chat_completions"
base_url = "{base}/v1"
api_key = "$RUSTX_CAPACITY_KEY"
[models."fixture/turn-growth"]
provider = "fixture"
id = "turn-growth"
context_window = 1000000
max_output_tokens = 4096
[models."fixture/turn-growth".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false
[models."fixture/turn-growth".compat]
chat_reasoning_replay = "omit"
[app_server]
max_resident_runtimes = 8
idle_grace_ms = 1000
shutdown_deadline_ms = 30000
''')
        await server.start()
        result = await server.client.call('thread/start', {'settings': {'cwd': str(workspace)}})
        session, thread = result['session']['id'], result['thread']['id']
        await server.client.call('session/name', {'session_id': session, 'name': 'Turn growth'})
        await server.attach(thread)
        await server.subscribe()
        await asyncio.sleep(a.settle)
        before = sample(server.pid, server.runtime)
        write(out / 'session.json', {'session_id': session, 'thread_id': thread, 'initial': before,
                                    'initial_dbstat': dbstat(server.runtime, a.strace)})
        for turn in range(1, a.turns + 1):
            if a.cpu_profiler and turn == a.profile_from:
                os.kill(server.pid, signal.SIGUSR2)
            began = time.monotonic_ns()
            ack = await server.client.call('turn/start', {'target': server.target,
                'inputId': f'growth-{turn:08}', 'content': [{'type': 'text', 'text': PROMPT}]})
            terminal = await asyncio.wait_for(server.client.settled[thread].get(), 60)
            assert terminal['outcome']['type'] == 'completed', terminal
            finished = time.monotonic_ns()
            checks = 0
            while True:
                status = await server.client.call('thread/read', {'threadId': thread}, allow_unavailable=True)
                checks += 1
                if status is not None and status['thread']['status']['type'] == 'idle':
                    break
                assert time.monotonic_ns() - finished < 30_000_000_000, status
                await asyncio.sleep(0)
            rows.append({'turn': turn, 'settled_ms': (finished - began) / 1e6,
                         'idle_ms': (time.monotonic_ns() - began) / 1e6,
                         'retirement_checks': checks, 'turn_id': ack['turnId']})
            if turn % a.window == 0 or turn == a.turns:
                await asyncio.sleep(a.settle)
                after = sample(server.pid, server.runtime)
                start = windows[-1]['turn_end'] + 1 if windows else 1
                subset = rows[start - 1:turn]
                n = len(subset)
                change = delta(before, after)
                window = {'turn_start': start, 'turn_end': turn, 'turn_midpoint': (start + turn) / 2,
                    'turns': n, **{key + '_per_turn': value / n for key, value in change.items()},
                    'wall_ms_per_turn': statistics.mean(r['settled_ms'] for r in subset),
                    'idle_ms_per_turn': statistics.mean(r['idle_ms'] for r in subset),
                    'wall_p95_ms': sorted(r['settled_ms'] for r in subset)[min(n - 1, int(n * .95))],
                    **{k: after[k] for k in ['rss_bytes', 'pss_bytes', 'sqlite_main_bytes', 'sqlite_wal_bytes']},
                    'before': before, 'after': after, 'dbstat': dbstat(server.runtime, a.strace),
                    'notifications': dict(server.client.notifications)}
                windows.append(window)
                write(out / 'windows.json', windows)
                write(out / 'turns.json', rows)
                print(f'{kind:4} {start:4}-{turn:<4} CPU {window["cpu_ms_per_turn"]:8.3f} ms '
                      f'wall {window["wall_ms_per_turn"]:8.3f} ms RSS {after["rss_bytes"]/1048576:8.2f} MiB '
                      f'PSS {after["pss_bytes"]/1048576:8.2f} MiB wchar {change["wchar"]/n/1024:9.2f} KiB '
                      f'write {change["write_bytes"]/n/1024:9.2f} KiB '
                      f'DB {after["sqlite_main_bytes"]/1048576:8.2f} WAL {after["sqlite_wal_bytes"]/1048576:6.2f} MiB', flush=True)
                if turn in a.reopen:
                    snapshot = out / f'cut-{turn}.sqlite'
                    db = next(server.runtime.rglob('conversation.sqlite'))
                    # Capture a transaction-consistent cut. The hot server
                    # remains resident; a separate process opens its copy.
                    source = sqlite3.connect(f'file:{db}?mode=ro', uri=True)
                    destination = sqlite3.connect(snapshot)
                    source.backup(destination)
                    destination.close()
                    source.close()
                    cold_dir = out / f'reopen-{turn}'
                    cold_dir.mkdir()
                    for name in ['rustx.toml', 'token']:
                        shutil.copyfile(out / name, cold_dir / name)
                    (cold_dir / 'token').chmod(0o600)
                    shutil.copytree(server.runtime, cold_dir / 'runtime',
                        ignore=shutil.ignore_patterns('*.sqlite-wal', '*.sqlite-shm'))
                    for original in server.runtime.rglob('*.sqlite'):
                        source = sqlite3.connect(f'file:{original}?mode=ro', uri=True)
                        destination = sqlite3.connect(cold_dir / 'runtime' / original.relative_to(server.runtime))
                        source.backup(destination)
                        destination.close()
                        source.close()
                    cold = Server(a, cold_dir)
                    empty = await cold.start()
                    began = time.monotonic_ns()
                    await cold.attach(thread)
                    attached_ns = time.monotonic_ns()
                    attached = sample(cold.pid, cold.runtime)
                    await cold.subscribe()
                    subscribed_ns = time.monotonic_ns()
                    await asyncio.sleep(a.settle)
                    loaded = sample(cold.pid, cold.runtime)
                    reopen.append({'history_turns': turn,
                        'attach_wall_ms': (attached_ns - began) / 1e6,
                        'subscribe_wall_ms': (subscribed_ns - attached_ns) / 1e6,
                        'attach': delta(empty, attached), 'attach_and_subscribe': delta(empty, loaded),
                        'rss_growth_bytes': loaded['rss_bytes'] - empty['rss_bytes'],
                        'pss_growth_bytes': loaded['pss_bytes'] - empty['pss_bytes'],
                        'empty': empty, 'attached': attached, 'loaded': loaded})
                    write(out / 'reopen.json', reopen)
                    await cold.stop()
                    shutil.rmtree(cold.runtime)
                before = sample(server.pid, server.runtime)
        if a.cpu_profiler and a.turns >= a.profile_from:
            os.kill(server.pid, signal.SIGUSR2)
        await server.stop()
        # Avoid serializing the provider's O(N^2) request report through its
        # control endpoint. Shutdown is a signal; its local report is compact.
        def shutdown():
            with urllib.request.urlopen(urllib.request.Request(base + '/__control/shutdown', data=b''), timeout=60) as response:
                while response.read(1024 * 1024):
                    pass
        await asyncio.to_thread(shutdown)
        assert await asyncio.wait_for(provider.wait(), 60) == 0
        report = json.loads((out / 'provider.json').read_text())
        assert report['ok'] and report['requestCount'] == ready['steps'], report
        db = next(server.runtime.rglob('conversation.sqlite'))
        with sqlite3.connect(f'file:{db}?mode=ro', uri=True) as connection:
            assert connection.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
            messages = [json.loads(r[0]) for r in connection.execute('SELECT message_json FROM message_ledger ORDER BY position')]
            counts = collections.Counter(m['role'] for m in messages)
            assert counts['user'] == a.turns, counts
            assert counts['tool'] == (a.turns if kind == 'tool' else 0), counts
            assert counts['assistant'] == a.turns * (2 if kind == 'tool' else 1), counts
            assert sum(m['content'] == [{'type': 'text', 'text': RESPONSE}] for m in messages if m['role'] == 'assistant') == a.turns
        slopes = regress(windows, ['cpu_ms_per_turn', 'wall_ms_per_turn', 'idle_ms_per_turn',
            'wchar_per_turn', 'write_bytes_per_turn', 'rss_bytes', 'pss_bytes', 'sqlite_main_bytes', 'sqlite_wal_bytes'])
        result = {'scenario': kind, 'accepted': True, 'windows': windows, 'slopes': slopes,
                  'reopen': reopen, 'closed_dbstat': dbstat(server.runtime), 'message_counts': dict(counts)}
        write(out / 'result.json', result)
        return result
    finally:
        if hasattr(server, 'process') and server.process.returncode is None:
            os.kill(server.pid, signal.SIGTERM)
            await asyncio.wait_for(server.process.wait(), 60)
        if provider.returncode is None:
            provider.terminate()
            await provider.wait()
        provider_log.close()
        workspace_owner.cleanup()


async def main(a):
    if not a.skip_build:
        subprocess.run(['cargo', 'build', '--release', '--locked', '--bin', 'rustx'], cwd=REPO, check=True)
    if a.affinity:
        os.sched_setaffinity(0, a.affinity)
    out = a.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    inputs = [Path(__file__).resolve(), Path(CAPACITY.__file__), REPO / 'Cargo.lock',
              *sorted((REPO / 'test-support/fake-provider/src').rglob('*.py'))]
    write(out / 'identity.json', {'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(),
        'binary': str(a.binary.resolve()), 'binary_sha256': hashlib.sha256(a.binary.read_bytes()).hexdigest(),
        'inputs': {str(p.relative_to(REPO)): hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs},
        'protocol_version': VERSION, 'python': sys.version, 'websockets': CAPACITY.websockets.__version__,
        'affinity': sorted(os.sched_getaffinity(0)), 'tokio_workers': 4, 'turns': a.turns, 'window': a.window,
        'settle_seconds': a.settle, 'reopen_at': a.reopen, 'strace': a.strace,
        'cpu_profiler': str(a.cpu_profiler) if a.cpu_profiler else None, 'profile_from': a.profile_from,
        'scope': 'Only server PID utime+stime, /proc/io, RSS/PSS. Fake provider and driver are siblings. '
                 'Settled/idle latency excludes between-window settle. CPU/I/O includes settle and retirement reads. '
                 'Reopen uses consistent backup copies in a separate fresh server process; the hot PID stays resident. '
                 'OS file cache remains warm; this is not a dropped-cache storage test. '
                 'Per-turn resource slopes use window midpoint; residency slopes use the same midpoint convention.'})
    results = [await scenario(a, kind, out / kind) for kind in a.scenarios]
    write(out / 'result.json', {'accepted': True, 'scenarios': results})
    for result in results:
        print(result['scenario'] + ' slopes per additional history turn: ' + json.dumps(result['slopes']), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('/tmp/rustx-turn-growth-' + time.strftime('%Y%m%d-%H%M%S')))
    parser.add_argument('--binary', type=Path, default=REPO / 'target/release/rustx')
    parser.add_argument('--skip-build', action='store_true')
    parser.add_argument('--turns', type=int, default=512)
    parser.add_argument('--window', type=int, default=32)
    parser.add_argument('--scenarios', nargs='+', choices=['text', 'tool'], default=['text', 'tool'])
    parser.add_argument('--reopen', nargs='*', type=int, default=[32, 128, 512])
    parser.add_argument('--settle', type=float, default=.25)
    parser.add_argument('--affinity', nargs='+', type=int, default=[2, 8])
    parser.add_argument('--strace', action='store_true', help='Diagnostic only; traces the server, not provider/driver')
    parser.add_argument('--cpu-profiler', type=Path, help='Diagnostic only: libprofiler.so, 500 Hz, toggled by SIGUSR2')
    parser.add_argument('--profile-from', type=int, default=1)
    parser.add_argument('--provider', choices=['text', 'tool'], help=argparse.SUPPRESS)
    parser.add_argument('--provider-output', type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    assert args.turns > 0 and args.window > 0 and args.settle >= 0
    if args.provider:
        sys.exit(asyncio.run(provider_main(args)))
    asyncio.run(main(args))
