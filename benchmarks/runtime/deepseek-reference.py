#!/usr/bin/env python3
"""Pinned DeepSeek Harness SDK reference, using its production CLI and JSONL store.

The SDK always publishes notifications. Its mandatory profile context, native
tool roster, durability, source-launch startup, and verification continuation
differ from the Pi/rustX benchmark. Results are references, not an equivalence
claim. The provider is a sibling process and never enters runtime counters.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import signal
import subprocess
import threading
import time
import traceback
import urllib.request

REVISION = '687f282e8625c6b8d11ff58211a90524be2e4317'
CASES = ('short-small', 'short-large', 'large', 'stream')
MODEL_TIME = '2026-10-19 00:00:00'
REMINDER = f'<system-reminder>\nTimezone: UTC\nCurrent time: {MODEL_TIME}\n</system-reminder>'
REPO = Path(__file__).resolve().parents[2]
SDK_ENVELOPE = json.loads(Path(__file__).with_name('deepseek-sdk-envelope.json').read_text())
assert SDK_ENVELOPE['revision'] == REVISION


def control(base, endpoint, method='GET'):
    with urllib.request.urlopen(urllib.request.Request(base + '/__control/' + endpoint, method=method), timeout=35) as response:
        return json.load(response)


def process_sample(pid):
    return {name: Path(f'/proc/{pid}/{name}').read_text() for name in ('stat', 'io', 'smaps_rollup')}


def process_tree(pid):
    """Keep raw counters for the CLI and each extant child, without summing RSS."""
    pending = [pid]
    result = {}
    while pending:
        current = pending.pop()
        try:
            result[str(current)] = process_sample(current)
            pending.extend(int(value) for value in Path(f'/proc/{current}/task/{current}/children').read_text().split())
        except FileNotFoundError:
            if current == pid:
                raise
    return result


def footprint(root):
    result = []
    for path in sorted(root.rglob('*')):
        if path.is_file():
            info = path.stat()
            result.append({'path': str(path.relative_to(root)), 'logical': info.st_size, 'allocated': info.st_blocks * 512})
    return result


def sha256(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def identity(checkout):
    revision = subprocess.check_output(['git', '-C', str(checkout), 'rev-parse', 'HEAD'], text=True).strip()
    assert revision == REVISION, revision
    subprocess.run(['git', '-C', str(checkout), 'diff', '--exit-code', 'HEAD'], check=True, capture_output=True)
    files = ['pnpm-lock.yaml', 'package.json', 'apps/cli/src/bin.ts', 'apps/cli/tsconfig.json',
             'apps/cli/src/sdk-source.cordis.patch.yml', 'packages/bundle/base/cordis.patch.yml',
             'packages/bundle/sdk-app/cordis.patch.yml', 'packages/sdk/server/src/server.ts',
             'packages/sdk/server/src/index.ts', 'packages/session/session-persistence-jsonl/src/storage.ts',
             'packages/bundle/sdk-minimal/cordis.patch.yml', 'node_modules/.pnpm/lock.yaml',
             'node_modules/tsx/dist/esm/index.mjs']
    native_files = list(checkout.glob('native/system/packages/*/bin/**/*.node'))
    assert native_files, 'missing freshly built native flock addon'
    files += [str(path.relative_to(checkout)) for path in native_files]
    return {'revision': revision, 'tree': subprocess.check_output(['git', '-C', str(checkout), 'rev-parse', 'HEAD^{tree}'], text=True).strip(),
            'checkout': str(checkout), 'node': subprocess.check_output(['node', '--version'], text=True).strip(),
            'launch': 'supported source CLI via tsx ESM; no built JavaScript substitution',
            'files': {name: sha256(checkout / name) for name in files}, 'adapter_sha256': sha256(Path(__file__)),
            'verification_reader_sha256': sha256(Path(__file__).with_name('deepseek-read-log.mjs')),
            'native_envelope_sha256': sha256(Path(__file__).with_name('deepseek-sdk-envelope.json'))}


class Provider:
    def __init__(self, case, workspace, output, env):
        self.log = output.with_suffix('.provider.stderr').open('w')
        self.output = output
        try:
            self.proc = subprocess.Popen(['uv', 'run', '--project', str(REPO / 'test-support/fake-provider'), '--frozen',
                                          'fake-provider', '--scenario', f'runtime_benchmark_{case}',
                                          '--workspace', str(workspace), '--port', '0'],
                                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
                                         text=True, env=env, start_new_session=True)
            lines = queue.Queue()
            def read_ready():
                try:
                    lines.put(self.proc.stdout.readline())
                except Exception as error:
                    lines.put(error)
            threading.Thread(target=read_ready, daemon=True).start()
            line = lines.get(timeout=30)
            if isinstance(line, Exception):
                raise line
            output.with_suffix('.provider.ready').write_text(line)
            ready = json.loads(line)
            assert ready['ready'] is True and ready['host'] == '127.0.0.1', ready
            self.base = f"http://127.0.0.1:{ready['port']}"
        except BaseException:
            if hasattr(self, 'proc'):
                self.kill()
            else:
                self.log.close()
            raise

    def close(self):
        report = control(self.base, 'shutdown', 'POST')
        self.output.with_suffix('.provider.stdout').write_text(self.proc.stdout.read())
        assert self.proc.wait(timeout=20) == 0
        self.log.close()
        self.output.with_suffix('.provider.json').write_text(json.dumps(report, indent=2))
        assert report['ok'] is True, report
        return report

    def kill(self):
        if self.proc.poll() is None:
            os.killpg(self.proc.pid, signal.SIGKILL)
            self.proc.wait()
        self.log.close()


class Runtime:
    def __init__(self, checkout, workspace, home, patch, output, env, profile='sdk-minimal'):
        self.frames = queue.Queue()
        self.notifications = []
        self.transcript = []
        self.next_id = 0
        self.stderr = output.with_suffix('.runtime.stderr').open('w')
        self.argv = ['node', '--import', str(checkout / 'node_modules/tsx/dist/esm/index.mjs'),
                     str(checkout / 'apps/cli/src/bin.ts'), '--profile', profile]
        if profile == 'sdk':
            # This exact compatibility patch is shipped by the official SDK's
            # supported source-launch resolver (sdk/client/src/launch.ts).
            self.argv += ['--patch', str(checkout / 'apps/cli/src/sdk-source.cordis.patch.yml')]
        self.argv += ['--patch', str(patch)]
        self.proc = subprocess.Popen(self.argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr,
                                     text=True, env=env | {'DSH_HOME': str(home), 'DSH_AGENTS_HOME': str(home / 'agents'),
                                     'TSX_TSCONFIG_PATH': str(checkout / 'apps/cli/tsconfig.json')},
                                     cwd=checkout, start_new_session=True)
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.output = output
        try:
            self.request('initialize', {'cwd': str(workspace), 'provider': 'benchmark', 'model': 'chat-model', 'maxTokens': 1024})
        except BaseException:
            self.kill()
            raise

    def _read(self):
        try:
            for line in self.proc.stdout:
                value = json.loads(line)
                self.transcript.append(value)
                self.frames.put(value)
        except Exception as error:
            self.frames.put(error)
        finally:
            self.frames.put(EOFError('runtime stdout closed'))

    def send(self, method, params=None):
        self.next_id += 1
        self.proc.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': self.next_id, 'method': method, 'params': params}) + '\n')
        self.proc.stdin.flush()
        return self.next_id

    def receive(self, request_id):
        while True:
            frame = self.frames.get(timeout=45)
            if isinstance(frame, Exception):
                raise frame
            if 'method' in frame:
                self.notifications.append(frame)
                continue
            assert frame.get('id') == request_id, frame
            assert 'error' not in frame, frame
            return frame['result']

    def request(self, method, params=None):
        return self.receive(self.send(method, params))

    def close(self):
        began = time.monotonic_ns()
        final_sample = process_tree(self.proc.pid)
        self.request('shutdown')
        self.proc.stdin.close()
        # wait4 preserves complete CLI CPU, peak RSS, and block-I/O counters,
        # including children the CLI reaped. Popen.wait must not reap first.
        deadline = time.monotonic() + 20
        while True:
            pid, status, usage = os.wait4(self.proc.pid, os.WNOHANG)
            if pid:
                break
            if time.monotonic() >= deadline:
                raise TimeoutError('native CLI did not exit after SDK shutdown')
            time.sleep(.005)
        assert pid == self.proc.pid
        self.proc.returncode = os.waitstatus_to_exitcode(status)
        close_ns = time.monotonic_ns() - began
        self.reader.join(timeout=5)
        self.stderr.close()
        self.output.with_suffix('.wire.json').write_text(json.dumps(self.transcript, indent=2))
        assert self.proc.returncode == 0, self.proc.returncode
        return {'last_before_shutdown': final_sample, 'close_ns': close_ns, 'exit_code': self.proc.returncode,
                'wait4': {name: getattr(usage, name) for name in ('ru_utime', 'ru_stime', 'ru_maxrss', 'ru_inblock', 'ru_oublock')}}

    def kill(self):
        if self.proc.returncode is None:
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.proc.wait()
        self.stderr.close()
        self.output.with_suffix('.wire.json').write_text(json.dumps(self.transcript, indent=2))


def patch_config(base, profile='sdk-minimal'):
    provider = {'id': 'llm-pi-ai' if profile == 'sdk' else 'benchmark-provider',
                'name': '@deepseek-ai/dsh-llm-pi-ai', 'config': {'providers': {
                'benchmark': {'api': 'openai-completions', 'baseURL': base + '/v1', 'apiKeyEnv': 'BENCHMARK_API_KEY',
                              'retryPolicy': {'mode': 'normal', 'maxRetries': 0},
                              'compat': {'supportsStore': False, 'maxTokensField': 'max_completion_tokens'},
                              'models': [{'id': 'chat-model', 'contextWindow': 128000, 'maxTokens': 1024}]}}}}
    return [{'id': 'llm-deepseek', 'disabled': True}, provider if profile == 'sdk' else {'insert': [provider]}]


def expected_reply(case, turn):
    if case == 'large':
        return 'L' * 32768
    if case == 'stream':
        return ''.join(f'stream-delta-{index:04} ' for index in range(128))
    return f'reply {turn:04}'


def visible_log_prefix(home):
    """Observe complete lines visible after idle; this does not prove fsync."""
    paths = list(home.rglob('session.v4.jsonl*'))
    if not paths:
        return {'files': 0, 'bytes': 0, 'complete_lines': 0, 'last_event_seq': None}
    assert len(paths) == 1, paths
    if paths[0].suffix == '.zstd':
        return {'files': 1, 'bytes': paths[0].stat().st_size, 'complete_lines': None, 'last_event_seq': None}
    data = paths[0].read_bytes()
    lines = data.split(b'\n')[:-1]
    last = json.loads(lines[-1]) if lines else {}
    return {'files': 1, 'bytes': len(data), 'complete_lines': len(lines), 'last_event_seq': last.get('seq')}


def expected_messages(case, turn, workspace, verification=False, profile='sdk-minimal'):
    """Build independently specified ordered input, including every native extra."""
    messages = [
        {'role': 'system', 'content': 'You are a helpful software engineer assistant.'},
        {'role': 'user', 'content': 'benchmark turn 0000' + REMINDER},
        {'role': 'user', 'content': 'Current runtime context. This snapshot supersedes earlier runtime-context snapshots.\n\n'
         + f'Current working directory: "{workspace}".'},
    ]
    if profile == 'sdk':
        messages[0]['content'] = SDK_ENVELOPE['system']
        messages[2]['content'] = SDK_ENVELOPE['runtime_context_template'].replace('<WORKSPACE>', workspace)
    for previous in range(turn):
        messages.append({'role': 'assistant', 'content': expected_reply(case, previous)})
        messages.append({'role': 'user', 'content': f'benchmark turn {0 if verification and previous == turn - 1 else previous + 1:04}'})
    return messages


def message_projection(message):
    assert message['role'] in ('user', 'assistant', 'system'), message
    assert all(set(block) == {'type', 'text'} and block['type'] == 'text' for block in message['content']), message
    return {'role': message['role'], 'content': ''.join(block['text'] for block in message['content'])}


def verify_result(result):
    """Reject lost/reordered input, output, native context, or restored history."""
    case, workspace, profile = result['case'], result['workspace'], result['profile']
    assert profile in ('sdk', 'sdk-minimal')
    history = {'short-small': 8, 'short-large': 64}.get(case, 0)
    measured = 32 if history else 1
    count = history + measured
    assert result['history'] == history and result['measured'] == measured
    assert len(result['samples']) == count
    assert len(result['requests']) == count and len(result['verification_requests']) == 1
    before, after = result['persisted_before_reopen'], result['persisted_after_reopen']
    assert after[:len(before)] == before, 'reopen altered committed native log prefix'
    assert before[0]['type'] == 'session' and before[0]['version'] == 4
    assert before[0]['id'] == 'benchmark' and before[0]['cwd'] == workspace
    assert [event['seq'] for event in after[1:]] == list(range(len(after) - 1)), 'noncontiguous durable events'
    assert result['first_notifications'] == before[1:], 'SDK events differ from closed native log'
    # The SDK starts forwarding after native resume, which appends this one
    # documented seed marker before attaching the live subscription.
    resume_marker = after[len(before)]
    assert resume_marker['type'] == 'session/end-seed' and resume_marker['data'] == {}
    assert result['verification_notifications'] == after[len(before) + 1:], 'resumed SDK events differ from appended native log'
    assert result['provider_report']['requests'] == result['requests']
    assert result['verification_provider_report']['requests'] == result['verification_requests']
    assert result['provider_report']['ok'] is True and result['verification_provider_report']['ok'] is True
    assert result['first_exit']['exit_code'] == result['verification_exit']['exit_code'] == 0
    allowed = {'agent/inbox/spliced', 'turn/start', 'turn/end', 'step/start', 'step/end', 'system/message',
               'user/message', 'assistant/message', 'request/header', 'request/context', 'session/title', 'session/end-seed'}
    if profile == 'sdk':
        allowed |= {'permission/preset', 'sandbox/mode', 'approval/policy'}
    messages, tools, assistant_count = [], None, 0
    all_requests = result['requests'] + result['verification_requests']
    for event in after[1:]:
        assert event['type'] in allowed, event
        if 'surfaceOp' in event:
            assert event['surfaceOp'] == 'append', 'unexpected native surface replacement'
        if event['type'] == 'request/header':
            header = event['data']['header']
            assert header['config'] == {'provider': 'benchmark', 'model': 'chat-model', 'maxTokens': 1024}
            next_tools = [{'type': 'function', 'function': tool} for tool in header['tools']]
            if profile == 'sdk':
                assert next_tools == SDK_ENVELOPE['tools'], 'full SDK native schemas differ from pinned fixture'
            else:
                assert [tool['name'] for tool in header['tools']] == ['bash', 'read_image', 'working_directory']
            assert tools is None or tools == next_tools, 'native tool roster changed at reopen'
            tools = next_tools
        if event['type'] in ('system/message', 'assistant/message', 'user/message'):
            message = event['data'] if event['type'] == 'user/message' else event['data']['message']
            projected = message_projection(message)
            if event['type'] == 'assistant/message':
                assert assistant_count <= count
                expected = expected_messages(case, assistant_count, workspace, verification=assistant_count == count, profile=profile)
                assert messages == expected, f'persisted ordered input differs at request {assistant_count}'
                request = all_requests[assistant_count]
                assert request['method'] == 'POST' and request['path'] == '/v1/chat/completions'
                assert request['protocol'] == 'openai_chat_completions'
                assert request['body'] == {'model': 'chat-model', 'messages': expected, 'stream': True,
                                           'stream_options': {'include_usage': True}, 'max_completion_tokens': 1024,
                                           'tools': tools}, f'wire differs from expected native input at {assistant_count}'
                reply = 'L' * 32768 if assistant_count == count else expected_reply(case, assistant_count)
                assert projected == {'role': 'assistant', 'content': reply}, f'assistant output differs at {assistant_count}'
                assistant_count += 1
            messages.append(projected)
    assert assistant_count == count + 1
    ends = [event['data'] for event in after[1:] if event['type'] == 'turn/end']
    assert ends == [{'turn': turn + 1, 'reason': {'kind': 'completed'}} for turn in range(count + 1)]
    assert sum(event['type'] == 'session/end-seed' for event in after[1:]) == 1
    for turn, sample in enumerate(result['samples']):
        assert sample['turn'] == turn and sample['measured'] == (turn >= history)
        assert sample['settled_ns'] >= sample['accepted_ns'] >= 0
        assert sample['before'] and sample['after']
    return {'version': 1, 'ordered_requests': count + 1, 'persisted_assistant_messages': count + 1,
            'native_resume': True, 'unchanged_persisted_prefix': True,
            'expected_history_origin': 'independently specified prompts and deterministic provider replies',
            'native_envelope_origin': 'reviewed first capture at pinned source; static fixture, not an independently authored native specification',
            'cross_engine_full_request_equivalence': False,
            'differences': ['native system prompt', 'native current-directory context',
                            'first prompt/reminder merged into one message', 'native tool schema',
                            'mandatory SDK committed-event/status publication; no partial token publication',
                            'JSONL batch/checkpoint durability with unacknowledged final fsync', 'source CLI launch',
                            'separate 32KiB verification continuation']}


def read_log(path, checkout):
    if path.suffix == '.zstd':
        data = subprocess.check_output(['node', '--import', str(checkout / 'node_modules/tsx/dist/esm/index.mjs'),
                                       str(Path(__file__).with_name('deepseek-read-log.mjs')), str(checkout), str(path)],
                                      env=os.environ | {'TSX_TSCONFIG_PATH': str(checkout / 'apps/cli/tsconfig.json')}, text=True)
    else:
        data = path.read_text()
    return [json.loads(line) for line in data.splitlines()]


def run_case(checkout, case, destination, env, profile='sdk-minimal'):
    destination.mkdir(parents=True, exist_ok=False)
    workspace, home = destination / 'workspace', destination / 'home'
    workspace.mkdir()
    (workspace / 'note.txt').write_text('deterministic-note-payload-6d41')
    stem = destination / 'sample'
    provider = runtime = None
    started = time.monotonic_ns()
    phases, samples = [], []
    result = {'case': case, 'workspace': str(workspace), 'accepted': False, 'profile': profile, 'observer_mode': 'mandatory-sdk-notifications',
              'model_time': MODEL_TIME,
              'completion_boundary': 'SDK session/wait idle; does not await JSONL flush/fsync',
              'observation_surface': 'SDK session.event and session.status; final assistant/message only, no streamed partial text notifications',
              'durability': {'compression': 'zstd' if profile == 'sdk' else 'none', 'batch_max_delay_ms': 200, 'checkpoint_policy_mounted': profile == 'sdk',
                             'final_verification': 'SDK shutdown response, actual exit, complete closed log, and native resume/continuation',
                             'fsync_success_acknowledged': False,
                             'shutdown_limitation': 'native SDK ignores root-disposal rejection via Promise.allSettled and exits zero; complete readable log is not an fsync-success or power-loss certificate',
                             'visible_prefix_scope': 'complete lines visible after idle; not fsync confirmation'},
              'unsupported_cases': {'cancel': 'SDK has no mid-turn cancellation',
              'tool': 'native read argument is file_path, not fixture path; sdk-minimal also omits text read'}}

    def phase(name):
        phases.append({'phase': name, 'elapsed_ns': time.monotonic_ns() - started,
                       'runtime': process_tree(runtime.proc.pid) if runtime is not None and runtime.proc.returncode is None else None,
                       'driver': process_sample(os.getpid())})

    try:
        phase('before_provider_start')
        provider = Provider(case, workspace, stem, env)
        phase('after_provider_start')
        patch = destination / 'profile.patch.json'
        patch.write_text(json.dumps(patch_config(provider.base, profile), indent=2))
        runtime = Runtime(checkout, workspace, home, patch, stem, env, profile)
        phase('after_initialization')
        history = {'short-small': 8, 'short-large': 64}.get(case, 0)
        measured = 32 if history else 1
        result.update(history=history, measured=measured, runtime_argv=runtime.argv)
        for turn in range(history + measured):
            before = process_tree(runtime.proc.pid)
            began = time.monotonic_ns()
            blocks = [{'type': 'text', 'text': f'benchmark turn {turn:04}'}]
            if turn == 0:
                blocks.append({'type': 'text', 'text': REMINDER})
            runtime.request('session/prompt', {'sessionId': 'benchmark', 'contentBlocks': blocks})
            accepted_ns = time.monotonic_ns() - began
            if case == 'stream':
                for delta in range(128):
                    control(provider.base, f'observations/await?kind=gate_reached&name=delta-{delta}&timeoutMs=20000')
                    time.sleep(.01)
                    control(provider.base, f'gates/delta-{delta}/release', 'POST')
            runtime.request('session/wait', {'sessionId': 'benchmark'})
            settled_ns = time.monotonic_ns() - began
            samples.append({'turn': turn, 'measured': turn >= history, 'accepted_ns': accepted_ns,
                            'settled_ns': settled_ns, 'before': before, 'after': process_tree(runtime.proc.pid),
                            'footprint': footprint(home), 'visible_log_prefix': visible_log_prefix(home)})
        phase('after_turns')
        result['requests'] = control(provider.base, 'requests')['requests']
        result['first_exit'] = runtime.close()
        result['first_notifications'] = [frame['params']['event'] for frame in runtime.transcript if frame.get('method') == 'session.event']
        phase('after_final_writer_close')
        result['provider_report'] = provider.close()
        provider = None
        result['closed_footprint'] = footprint(home)
        logs = list(home.rglob('session.v4.jsonl*'))
        assert len(logs) == 1, logs
        persisted = read_log(logs[0], checkout)
        result['persisted_before_reopen'] = persisted
        # The public SDK restores the exact native session. A separately priced
        # continuation exercises its reconstructed history through the real LLM
        # adapter, using a second sibling fixture outside measured turns.
        provider = Provider('large', workspace, destination / 'verification', env)
        patch.write_text(json.dumps(patch_config(provider.base, profile), indent=2))
        runtime = Runtime(checkout, workspace, home, patch, destination / 'verification', env, profile)
        runtime.request('session/resume', {'sessionId': 'benchmark'})
        phase('after_native_resume')
        runtime.request('session/prompt', {'sessionId': 'benchmark', 'contentBlocks': [{'type': 'text', 'text': 'benchmark turn 0000'}]})
        runtime.request('session/wait', {'sessionId': 'benchmark'})
        result['verification_requests'] = control(provider.base, 'requests')['requests']
        result['verification_exit'] = runtime.close()
        result['verification_notifications'] = [frame['params']['event'] for frame in runtime.transcript if frame.get('method') == 'session.event']
        result['verification_provider_report'] = provider.close()
        provider = None
        result['persisted_after_reopen'] = read_log(logs[0], checkout)
        phase('after_reopen_verification')
        result['samples'] = samples
        result['oracle'] = verify_result(result)
        result['accepted'] = True
    except Exception:
        result['error'] = traceback.format_exc()
    finally:
        if runtime is not None:
            runtime.kill()
        if provider is not None:
            provider.kill()
        result.update(lifecycle=phases, samples=samples, elapsed_ns=time.monotonic_ns() - started)
        (destination / 'result.json').write_text(json.dumps(result, indent=2))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkout', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--case', action='append', choices=CASES)
    parser.add_argument('--repetitions', type=int, default=1)
    parser.add_argument('--profile', choices=('sdk', 'sdk-minimal'), default='sdk')
    args = parser.parse_args()
    checkout, output = args.checkout.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    manifest = identity(checkout)
    manifest.update(clock_ticks=os.sysconf('SC_CLK_TCK'), profile=args.profile, cases=args.case or CASES, repetitions=args.repetitions,
                    scope='direct production CLI only; sibling provider and Python driver excluded from runtime counters; separate verification continuation included only in lifecycle')
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2))
    env = {key: value for key, value in os.environ.items() if not any(word in key.upper() for word in ('TOKEN', 'SECRET', 'CREDENTIAL', 'API_KEY'))
           and not key.startswith(('DSH_', 'DEEPSEEK_'))}
    env.setdefault('UV_CACHE_DIR', str(Path.home() / '.cache/uv'))
    env.update(BENCHMARK_API_KEY='conformance-secret', DSH_TELEMETRY_DISABLED='1')
    accepted = True
    for repetition in range(args.repetitions):
        for case in args.case or CASES:
            result = run_case(checkout, case, output / f'{repetition}-{case}', env, args.profile)
            print(json.dumps({key: result.get(key) for key in ('case', 'accepted', 'error')}), flush=True)
            accepted &= result['accepted']
    raise SystemExit(0 if accepted else 1)


if __name__ == '__main__':
    main()
