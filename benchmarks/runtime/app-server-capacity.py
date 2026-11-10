"""Real WebSocket App Server capacity probe; process counters exclude its driver/provider.

Requires the already-installed websockets package. This is a bounded workload,
not a production capacity or P99 guarantee. No Pi or DeepSeek execution.
"""
import argparse
import asyncio
import base64
import collections
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import sqlite3
import subprocess
import sys
import time
import urllib.request

import websockets

REPO = Path(__file__).resolve().parents[2]
TOKEN = 'capacity_fixture_' + 'x' * 48
PROTOCOL_VERSION = 76


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def counter(pid):
    base = Path('/proc') / str(pid)
    return {name: (base / name).read_text() for name in ['stat', 'io', 'smaps_rollup', 'status']}


def retained_files(root):
    """Endpoint residency, not cumulative writes; include sparse allocation."""
    result = {}
    for path in root.rglob('*'):
        try:
            if not path.is_file():
                continue
            stat = path.stat()
        except FileNotFoundError:
            continue
        kind = next((kind for suffix, kind in [('.sqlite-wal', 'wal'),
                    ('.sqlite-shm', 'shm'), ('.sqlite', 'main')] if path.name.endswith(suffix)), 'other')
        group = result.setdefault(kind, {'files': 0, 'logical_bytes': 0,
                                         'allocated_bytes': 0, 'largest_logical_bytes': 0})
        group['files'] += 1
        group['logical_bytes'] += stat.st_size
        group['allocated_bytes'] += stat.st_blocks * 512
        group['largest_logical_bytes'] = max(group['largest_logical_bytes'], stat.st_size)
    return result


def effects(value):
    if isinstance(value, dict):
        if value.get('type') == 'attempt_settled':
            yield value
        else:
            for child in value.values():
                yield from effects(child)
    elif isinstance(value, list):
        for child in value:
            yield from effects(child)


class Client:
    def __init__(self, ws, path):
        self.ws, self.path = ws, path
        self.sequence = 0
        self.pending = {}
        self.settled = collections.defaultdict(asyncio.Queue)
        self.notifications = collections.Counter()
        # Match native lanes; the advertised total also includes eight Agent
        # observation slots. Control intent keeps its two independent credits.
        self.ordinary = asyncio.Semaphore(6)
        # Concurrent native thread/read calls use the separate two-slot
        # metadata lane, including while ordinary turn requests are in flight.
        self.metadata = asyncio.Semaphore(2)
        self.control = asyncio.Semaphore(2)
        self.task = asyncio.create_task(self.read())

    @classmethod
    async def open(cls, endpoint, path):
        bearer = base64.urlsafe_b64encode(TOKEN.encode()).decode().rstrip('=')
        ws = await websockets.connect(endpoint,
                                      subprotocols=[f'rustx.app-server.v{PROTOCOL_VERSION}', 'rustx-token.' + bearer],
                                      compression=None, max_size=1048576, max_queue=32)
        self = cls(ws, path)
        initialized = await self.call('initialize', {
            'protocol_version': PROTOCOL_VERSION, 'client': {'name': 'capacity', 'version': '1'},
            'presentation': {'images': False, 'questionnaires': False, 'reviews': False}})
        assert initialized['type'] == 'initialized', initialized
        return self

    async def read(self):
        try:
            with self.path.open('w') as log:
                async for raw in self.ws:
                    value = json.loads(raw)
                    if 'id' in value:
                        log.write(json.dumps({'id': value['id'], 'error': value.get('error')}) + '\n')
                        future = self.pending.pop(value['id'])
                        future.set_result(value)
                    else:
                        method = value['method']
                        self.notifications[method] += 1
                        if method == 'session/event':
                            params = value['params']
                            for terminal in effects(params['event']):
                                self.settled[params['target']['conversation_id']].put_nowait(terminal)
                        elif method in ['session/observationUnavailable', 'session/resyncRequired']:
                            raise AssertionError(value)
        except BaseException as error:
            for future in self.pending.values():
                if not future.done():
                    future.set_exception(error)
            if not isinstance(error, asyncio.CancelledError):
                raise

    async def call(self, method, params, *, allow_unavailable=False):
        lane = (self.control if method in ['turn/interrupt', 'session/detach']
                else self.metadata if method == 'thread/read' else self.ordinary)
        async with lane:
            self.sequence += 1
            identity = self.sequence
            future = asyncio.get_running_loop().create_future()
            self.pending[identity] = future
            await self.ws.send(json.dumps({'jsonrpc': '2.0', 'id': identity,
                                           'method': method, 'params': params}))
            value = await asyncio.wait_for(future, 60)
            if (allow_unavailable and value.get('error', {}).get('data', {}).get('kind')
                    == 'observation_unavailable'):
                return None
            assert 'error' not in value, value
            return value['result']

    async def close(self):
        await self.ws.close()
        await self.task


async def main(a):
    out = a.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    os.sched_setaffinity(0, [2, 8])
    assert subprocess.run(['pgrep', '-x', 'rustc'], capture_output=True).returncode != 0, 'compiler overlap'
    provider_sources = subprocess.check_output(
        ['git', 'ls-files', '--', 'test-support/fake-provider'], cwd=REPO, text=True).splitlines()
    inputs = {str(path.relative_to(REPO)): hashlib.sha256(path.read_bytes()).hexdigest()
              for path in [Path(__file__), Path(__file__).with_name('app-server-capacity-provider.py'),
                           *(REPO / name for name in provider_sources)] if path.is_file()}
    write(out / 'identity.json', {
        'binary': str(a.binary.resolve()),
        'sha256': hashlib.sha256(a.binary.read_bytes()).hexdigest(),
        'websockets': websockets.__version__, 'affinity': sorted(os.sched_getaffinity(0)),
        'sessions': a.sessions, 'history_turns': a.history, 'widths': a.widths,
        'rounds': a.rounds, 'cache_probes': a.cache_probes,
        'protocol_version': PROTOCOL_VERSION, 'memory_only': a.memory_only,
        'rotate_active': a.rotate_active,
        'idle_seconds': a.idle_seconds,
        'history_checkpoints': a.history_checkpoints,
        'session_names': 'Explicit fixture names; automatic title generation is not part of this workload.',
        'retirement': 'Observe native idle after terminal delivery before starting another turn; CPU windows include these reads.',
        'scope': 'Actual native App Server over authenticated WebSocket. '
        'Single-process samples, not a production capacity/P99 guarantee. '
        'Driver and provider are siblings; only server PID resource counters are reported.',
        'inputs': inputs, 'tokio_workers': 4})
    if a.cpu_profiler:
        write(out / 'profiler.json', {
            'library': str(a.cpu_profiler.resolve()),
            'sha256': hashlib.sha256(a.cpu_profiler.read_bytes()).hexdigest(),
            'frequency': 250, 'signal': int(signal.SIGUSR2),
            'scope': 'Diagnostic only: native server CPU signal samples in the final active width. '
                     'No driver/provider CPU and no ordinary timing claims.'})
    seed_requests = a.sessions * a.history
    provider_log = (out / 'provider.stderr').open('w')
    provider = await asyncio.create_subprocess_exec(
        'uv', 'run', '--project', str(REPO / 'test-support/fake-provider'), '--frozen',
        'python', str(REPO / 'benchmarks/runtime/app-server-capacity-provider.py'),
        '--seed-requests', str(seed_requests), '--widths', ','.join(map(str, a.widths)),
        '--rounds', str(a.rounds),
        *(['--memory-only'] if a.memory_only else []),
        '--cache-probe-requests', str(24 + 2 * a.sessions if a.cache_probes else 0),
        stdout=asyncio.subprocess.PIPE, stderr=provider_log,
        env=os.environ | {'UV_OFFLINE': '1'})
    process = None
    clients = []
    phases, latencies, sessions, targets, owners = [], [], [], [], []
    threads = {}
    try:
        ready = json.loads(await asyncio.wait_for(provider.stdout.readline(), 30))
        base = f"http://127.0.0.1:{ready['port']}"

        async def control(path, post=False):
            def fetch():
                req = urllib.request.Request(base + '/__control/' + path,
                                             data=b'' if post else None)
                with urllib.request.urlopen(req, timeout=30) as response:
                    return json.load(response)
            return await asyncio.to_thread(fetch)

        async def gate(name):
            await control('observations/await?kind=gate_reached&name=' + name + '&count=1&timeoutMs=30000')

        config = out / 'rustx.toml'
        config.write_text(f'''[agent]
[agent.tools]
builtin = ["read", "write", "edit", "glob", "grep", "bash", "job_status"]
[agent.model]
model = "fixture/capacity"
[providers.fixture]
base_url = "{base}/v1"
api_key = "$RUSTX_CAPACITY_KEY"
[models."fixture/capacity"]
provider = "fixture"
id = "capacity-model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 4096
[models."fixture/capacity".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false
[models."fixture/capacity".compat]
chat_reasoning_replay = "omit"
[app_server]
max_resident_runtimes = {max(8, a.sessions)}
max_external_attachments = 512
max_connections = 32
idle_grace_ms = 1000
shutdown_deadline_ms = 30000
''')
        token = out / 'token'
        token.write_text(TOKEN)
        token.chmod(0o600)
        server_env = os.environ | {'RUSTX_CAPACITY_KEY': 'fixture', 'TOKIO_WORKER_THREADS': '4'}
        if a.cpu_profiler:
            server_env.update(LD_PRELOAD=str(a.cpu_profiler.resolve()),
                              CPUPROFILE=str(out / 'active.cpu'),
                              CPUPROFILESIGNAL=str(int(signal.SIGUSR2)),
                              CPUPROFILE_FREQUENCY='250')
        process = await asyncio.create_subprocess_exec(
            str(a.binary.resolve()), 'app-server', '--listen', 'ws://127.0.0.1:0',
            '--config', str(config), '--runtime-root', str(out / 'runtime'),
            '--token-file', str(token), cwd=out,
            env=server_env,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
        stderr_log = (out / 'server.stderr').open('w')
        while True:
            line = (await asyncio.wait_for(process.stderr.readline(), 60)).decode()
            stderr_log.write(line)
            stderr_log.flush()
            assert line, 'server exited before readiness'
            if line.startswith('rustx app-server listening '):
                endpoint = line.strip().split()[-1]
                break

        async def drain():
            async for line in process.stderr:
                stderr_log.write(line.decode())
        draining = asyncio.create_task(drain())
        client = await Client.open(endpoint, out / 'client.log')
        clients.append(client)

        async def sample(name):
            diagnostic = await client.call('server/diagnostics', {})
            phases.append({'phase': name, 'monotonic_ns': time.monotonic_ns(),
                           'process': counter(process.pid), 'diagnostics': diagnostic['snapshot'],
                           'notifications': dict(sum((owner.notifications for owner in clients),
                                                     collections.Counter())),
                           'retained_files': retained_files(out / 'runtime')})
            write(out / 'phases.json', phases)

        async def attach(session, observer=True, owner=client):
            target = (await owner.call('thread/resume', {'threadId': threads[session]}))['target']
            if observer:
                await subscribe(target, owner)
            return target

        async def subscribe(target, owner=client):
            checkpoint = (await owner.call('session/checkpoint', {'target': target, 'trace_records': []}))['checkpoint']
            await owner.call('session/subscribe', {'target': target, 'after_cursor': checkpoint['cursor']})

        turn_counts = collections.Counter()

        async def turn(index, expected='completed', accepted_turn=None):
            target = targets[index]
            turn = turn_counts[index]
            turn_counts[index] += 1
            began = time.monotonic_ns()
            acknowledgement = await owners[index].call('turn/start', {'target': target,
                'inputId': f'capacity-{index}-{turn}', 'content': [
                {'type': 'text', 'text': f'capacity turn session={index:04} turn={turn:04}'}]})
            accepted = time.monotonic_ns()
            if accepted_turn is not None:
                accepted_turn.set_result(acknowledgement['turnId'])
            terminal = await asyncio.wait_for(owners[index].settled[target['conversation_id']].get(), 60)
            assert terminal['outcome']['type'] == expected, terminal
            finished = time.monotonic_ns()
            # A delivered terminal commits the result before the coordinator
            # finishes retirement. Observe readiness rather than racing another
            # turn/start against the closed steering gate of that old attempt.
            checks = 0
            deadline = time.monotonic() + 30
            while True:
                status = await owners[index].call('thread/read',
                    {'threadId': target['conversation_id']}, allow_unavailable=True)
                checks += 1
                if status is not None:
                    state = status['thread']['status']['type']
                    assert state in ['idle', 'active'], status
                    if state == 'idle':
                        break
                assert time.monotonic() < deadline, status
                await asyncio.sleep(0)
            return {'session': index, 'turn': turn, 'accepted_us': (accepted - began) / 1000,
                    'settled_us': (finished - began) / 1000,
                    'retirement_checks': checks, 'idle_us': (time.monotonic_ns() - began) / 1000,
                    'began_monotonic_ns': began, 'settled_monotonic_ns': finished,
                    'acknowledgement': acknowledgement, 'terminal': terminal}

        await sample('empty_server')
        for index in range(a.sessions):
            workspace = out / 'workspaces' / str(index)
            workspace.mkdir(parents=True)
            result = await client.call('thread/start', {'settings': {'cwd': str(workspace)}})
            session = result['session']['id']
            assert result['thread']['sessionId'] == session
            threads[session] = result['thread']['id']
            sessions.append(session)
            # Use normal naming so the finite provider scenario has exactly
            # one request per turn, without auxiliary title generation.
            await client.call('session/name', {'session_id': session, 'name': f'Capacity {index}'})
        await sample('metadata_only')
        primary_connections = (a.sessions + 31) // 32
        for i in range(1, primary_connections):
            clients.append(await Client.open(endpoint, out / f'controller-{i}.log'))
        for index, session in enumerate(sessions):
            # Spread active prefixes too, rather than giving one connection
            # every active Session while the other connections remain idle.
            owner = clients[index % primary_connections]
            owners.append(owner)
            targets.append(await attach(session, observer=False, owner=owner))
            if index + 1 in set([1, 8, 32, 64, a.sessions]):
                await sample(f'empty_attached_{index + 1}')
        for index, session in enumerate(sessions):
            await subscribe(targets[index], owners[index])
            if index + 1 in set([1, 8, 32, 64, a.sessions]):
                await sample(f'subscribed_{index + 1}')
        await sample('all_subscribed_empty')
        routing = [dict(target) for target in targets]
        write(out / 'routing.json', routing)
        for seed in range(a.history):
            for start in range(0, a.sessions, 8):
                await asyncio.gather(*(turn(i) for i in range(start, min(start + 8, a.sessions))))
            if seed + 1 in [1, a.history, *a.history_checkpoints]:
                await sample(f'seeded_history_{seed + 1}')
        if a.idle_seconds:
            idle_began = time.monotonic()
            for seconds in a.idle_seconds:
                await asyncio.sleep(max(0, idle_began + seconds - time.monotonic()))
                # Manager diagnostics do not access conversation SQLite. Every
                # actor/subscription and its exact seeded history stay loaded.
                await sample(f'idle_{seconds}s')
        if a.cache_probes:
            # All endpoints below still hold every actor and subscription.
            # Diagnostic RPCs touch manager metadata, not conversation SQLite.
            idle_began = time.monotonic()
            await asyncio.sleep(32)
            await sample('cache_pages_idle')
            mixed_before = counter(process.pid)
            kept_warm = []
            for repeat in range(24):
                await asyncio.sleep(max(0, idle_began + 36 + repeat * 4 - time.monotonic()))
                kept_warm.append(await turn(repeat % 4))
            await asyncio.sleep(max(0, idle_began + 130 - time.monotonic()))
            await sample('cache_statements_idle')
            write(out / 'cache-kept-warm.json', {'before': mixed_before, 'after': counter(process.pid), 'rows': kept_warm,
                'scope': '24 turns in four sessions at16s/session while the other124 stay subscribed and idle. Counter window includes idle maintenance, not pure active CPU.'})
            inspect_before = counter(process.pid)
            inspect_began = time.monotonic_ns()
            checkpoint = await owners[a.sessions-2].call('session/checkpoint', {'target': targets[a.sessions-2], 'trace_records': []})
            history = await client.call('session/history', {'session_id': sessions[a.sessions-3], 'node_id': None, 'at': {'type': 'latest'}, 'limit': 32})
            content_ref = next(binding['content_ref']
                for descriptor in checkpoint['checkpoint']['snapshot']['message_descriptors']
                for binding in descriptor['content'] if binding['content_ref']['readable_end'] > 0)
            chunks = [await owners[a.sessions-2].call('conversation/contentRead', {
                'target': targets[a.sessions-2], 'agent_id': None, 'content_ref': content_ref,
                'offset': 0, 'max_bytes': 4096}) for _ in range(2)]
            assert chunks[0] == chunks[1]
            chunk = chunks[0]['chunk']
            assert chunk['content_ref'] == content_ref and chunk['offset'] == 0
            assert len(base64.b64decode(chunk['data_base64'], validate=True)) == chunk['end_offset']
            write(out / 'cache-inspector.json', {'checkpoint': checkpoint, 'history': history, 'chunks': chunks,
                'elapsed_ms': (time.monotonic_ns()-inspect_began)/1e6,
                'before': inspect_before, 'after': counter(process.pid)})
            for label in ['cold', 'warm']:
                before = counter(process.pid)
                rows = [await turn(index) for index in range(a.sessions)]
                write(out / f'cache-rotation-{label}.json', {'before': before, 'after': counter(process.pid), 'rows': rows,
                    'scope': 'One sequential turn per session. Cold excludes sessions0..3 kept active and the two Inspector targets when aggregating cold-eligible rows; warm repeats the same order promptly.'})
                await sample('cache_rotation_' + label)
        if not a.memory_only:
            for width in a.widths:
                before = counter(process.pid)
                profiling = a.cpu_profiler is not None and width == a.widths[-1]
                if profiling:
                    process.send_signal(signal.SIGUSR2)
                for repeat in range(a.rounds):
                    # Only the first repeat deliberately holds requests to measure
                    # resident active state. Other repeats run without a gate delay.
                    if repeat:
                        await asyncio.gather(*(control(f'gates/active-{width}-{repeat}-{i}/release', True)
                                               for i in range(width)))
                    start = repeat * width if a.rotate_active else 0
                    tasks = [asyncio.create_task(turn((start + i) % a.sessions)) for i in range(width)]
                    if repeat == 0:
                        await asyncio.gather(*(gate(f'active-{width}-{repeat}-{i}') for i in range(width)))
                        await sample(f'active_held_{width}')
                        release_ns = time.monotonic_ns()
                        await asyncio.gather(*(control(f'gates/active-{width}-{repeat}-{i}/release', True)
                                               for i in range(width)))
                    else:
                        release_ns = None
                    rows = await asyncio.gather(*tasks)
                    latencies.append({'width': width, 'repeat': repeat, 'turns': rows,
                                      'held': repeat == 0, 'release_began_ns': release_ns})
                if profiling:
                    process.send_signal(signal.SIGUSR2)
                await sample(f'active_finished_{width}')
                write(out / f'active-{width}.json', {'before': before, 'after': counter(process.pid),
                                                   'rows': [r for r in latencies if r['width'] == width]})
            # One controller per Session is the product contract. Move disjoint
            # idle Sessions to other connections instead of inventing observers.
            for copy in range(3):
                observer = await Client.open(endpoint, out / f'observer-{copy}.log')
                clients.append(observer)
                for index in range(8 + 8 * copy, min(16 + 8 * copy, a.sessions - 1)):
                    await owners[index].call('session/detach', {'target': targets[index]})
                    await attach(sessions[index], owner=observer)
                    targets[index] = None
            await sample('extra_controller_connections')
            for session in sessions:
                await client.call('session/history', {'session_id': session, 'node_id': None,
                                                      'at': {'type': 'latest'}, 'limit': 32})
            await sample('history_browsed')
            task = asyncio.create_task(turn(0))
            await gate('browse')
            # History reads run while native generation is held, then settle normally.
            await client.call('session/traceHistory', {'session_id': sessions[0], 'node_id': None,
                                                       'before': None, 'limit': 16})
            await control('gates/browse/release', True)
            await task
            slow = await Client.open(endpoint, out / 'slow-client.log')
            clients.append(slow)
            slow_index = a.sessions - 1
            await owners[slow_index].call('session/detach', {'target': targets[slow_index]})
            slow_target = await attach(sessions[slow_index], owner=slow)
            targets[slow_index] = None
            slow_turn = turn_counts[slow_index]
            turn_counts[slow_index] += 1
            await slow.call('turn/start', {'target': slow_target, 'inputId': f'capacity-{slow_index}-{slow_turn}', 'content': [
                {'type': 'text', 'text': f'capacity turn session={slow_index:04} turn={slow_turn:04}'}]})
            await gate('slow')
            slow.task.cancel()
            await slow.task
            await sample('slow_consumer_held')
            await control('gates/slow/release', True)
            deadline = time.monotonic() + 30
            while (await client.call('server/diagnostics', {}))['snapshot']['active_roots']:
                assert time.monotonic() < deadline
                await asyncio.sleep(0.01)
            history = await client.call('session/history', {'session_id': sessions[slow_index],
                                                            'node_id': None, 'at': {'type': 'latest'},
                                                            'limit': 32})
            write(out / 'slow-completed-history.json', history)
            await sample('slow_consumer_settled')
            await slow.ws.close()
            clients.remove(slow)
            accepted_turn = asyncio.get_running_loop().create_future()
            cancel = asyncio.create_task(turn(0, 'cancelled', accepted_turn))
            await gate('cancel')
            began = time.monotonic_ns()
            await client.call('turn/interrupt', {'target': targets[0],
                'turnId': await asyncio.wait_for(accepted_turn, 30)})
            cancelled = await cancel
            await control('observations/await?kind=client_disconnected&count=1&timeoutMs=30000')
            await control('gates/cancel/release', True)
            await control(f"observations/await?kind=response_completed&count={ready['steps']}&timeoutMs=30000")
            write(out / 'cancellation.json', {'cancel_to_settled_us': (time.monotonic_ns() - began) / 1000,
                                             'turn': cancelled})
        for index, target in enumerate(targets):
            if target is not None:
                await owners[index].call('session/detach', {'target': target})
        for observer in clients[1:]:
            await observer.close()
        clients[:] = [client]
        deadline = time.monotonic() + 30
        while True:
            diagnostic = (await client.call('server/diagnostics', {}))['snapshot']
            if diagnostic['loaded'] == diagnostic['loading'] == diagnostic['unloading'] == 0:
                break
            assert time.monotonic() < deadline, diagnostic
            await asyncio.sleep(0.1)
        await sample('all_unloaded')
        for cycle in range(3):
            reused = [await attach(s, observer=False) for s in sessions[:min(8, a.sessions)]]
            for target in reused:
                await client.call('session/checkpoint', {'target': target, 'trace_records': []})
                await client.call('session/detach', {'target': target})
            deadline = time.monotonic() + 30
            while True:
                diagnostic = (await client.call('server/diagnostics', {}))['snapshot']
                if diagnostic['loaded'] == diagnostic['loading'] == diagnostic['unloading'] == 0:
                    break
                assert time.monotonic() < deadline, diagnostic
                await asyncio.sleep(0.1)
            await sample(f'reload_unload_cycle_{cycle}')
        await client.close()
        clients.clear()
        process.send_signal(signal.SIGTERM)
        assert await asyncio.wait_for(process.wait(), 60) == 0
        await draining
        stderr_log.close()
        report = await control('shutdown', True)
        write(out / 'provider-report.json', report)
        provider_output = (await provider.stdout.read()).decode()
        (out / 'provider.stdout').write_text(provider_output)
        assert await asyncio.wait_for(provider.wait(), 30) == 0
        assert report['ok'] and report['requestCount'] == ready['steps'], report
        expected_turns = collections.Counter()
        request_bindings = []
        for request in report['requests']:
            body = request['body']
            if isinstance(body, str):
                body = json.loads(body)
            markers = re.findall(r'capacity turn session=(\d{4}) turn=(\d{4})',
                                 json.dumps(body['messages']))
            assert markers, request['index']
            index, turn = map(int, markers[-1])
            assert turn == expected_turns[index]
            assert markers == [(f'{index:04}', f'{i:04}') for i in range(turn + 1)]
            expected_turns[index] += 1
            request_bindings.append({'index': request['index'], 'session': index, 'turn': turn})
        assert expected_turns == turn_counts
        write(out / 'request-bindings.json', request_bindings)
        # Native closed-database checks are independent of observer delivery.
        databases = sorted((out / 'runtime').rglob('conversation.sqlite'))
        assert len(databases) == a.sessions, len(databases)
        native = []
        for db in databases:
            files = {suffix or 'main': (Path(str(db) + suffix).stat().st_size
                                        if Path(str(db) + suffix).exists() else 0)
                     for suffix in ['', '-wal', '-shm']}
            connection = sqlite3.connect(f'file:{db}?mode=ro', uri=True)
            assert connection.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
            index = next(i for i, target in enumerate(routing)
                         if target['conversation_id'] == db.parent.name)
            messages = [json.loads(row[0]) for row in connection.execute(
                'SELECT message_json FROM message_ledger ORDER BY position')]
            users = [m for m in messages if m['role'] == 'user']
            assistants = [m for m in messages if m['role'] == 'assistant']
            assert len(users) == turn_counts[index]
            assert len(assistants) == turn_counts[index] - int(not a.memory_only and index == 0)
            for turn, user in enumerate(users):
                assert user['content'] == [{'type': 'text',
                    'text': f'capacity turn session={index:04} turn={turn:04}'}]
            for turn, assistant in enumerate(assistants):
                text = 'S' * 32768 if not a.memory_only and index == slow_index and turn == len(assistants) - 1 else 'capacity response'
                assert assistant['content'] == [{'type': 'text', 'text': text}]
            assert not any('forbidden' in json.dumps(m) for m in messages)
            native.append({'path': str(db.relative_to(out)),
                           'session_index': index, 'messages': len(messages),
                           'events': connection.execute('SELECT count(*) FROM events').fetchone()[0],
                           'pages': connection.execute('PRAGMA page_count').fetchone()[0],
                           'free_pages': connection.execute('PRAGMA freelist_count').fetchone()[0],
                           'page_size': connection.execute('PRAGMA page_size').fetchone()[0],
                           'post_process_exit_files': files})
            backup_dir = out / 'native-snapshots'
            backup_dir.mkdir(exist_ok=True)
            backup_path = backup_dir / f'{index:04}.sqlite'
            backup = sqlite3.connect(backup_path)
            connection.backup(backup)
            backup.close()
            native[-1]['consistent_backup_bytes'] = backup_path.stat().st_size
            native[-1]['consistent_backup_sha256'] = hashlib.sha256(backup_path.read_bytes()).hexdigest()
            connection.close()
        write(out / 'native-closed.json', native)
        write(out / 'completed.json', {'accepted': True, 'requests': ready['steps'],
                                        'turn_counts': dict(turn_counts), 'phases': len(phases)})
        assert all(hashlib.sha256((REPO / name).read_bytes()).hexdigest() == digest
                   for name, digest in inputs.items())
        print(json.dumps({'accepted': True, 'requests': ready['steps'], 'sessions': a.sessions}), flush=True)
    finally:
        for client in clients:
            try:
                await client.ws.close()
            except Exception:
                pass
        for child in [process, provider]:
            if child is not None and child.returncode is None:
                child.terminate()
                try:
                    await asyncio.wait_for(child.wait(), 15)
                except asyncio.TimeoutError:
                    child.kill()
                    await child.wait()
        provider_log.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--sessions', type=int, default=128)
    parser.add_argument('--history', type=int, default=8)
    parser.add_argument('--widths', type=int, nargs='+', default=[1, 4, 8])
    parser.add_argument('--rounds', type=int, default=16)
    parser.add_argument('--cpu-profiler', type=Path)
    parser.add_argument('--rotate-active', action='store_true', help='Advance the active session group each repeat instead of keeping a fixed prefix')
    parser.add_argument('--idle-seconds', type=int, nargs='+', default=[], help='Sample unchanged loaded memory at these seconds after seeding; requires memory-only')
    parser.add_argument('--history-checkpoints', type=int, nargs='+', default=[], help='Sample these completed history depths in the same loaded process')
    parser.add_argument('--memory-only', action='store_true', help='Measure empty/seeded memory and native close/reload checks without active, browse, slow-consumer or cancel scenarios')
    parser.add_argument('--cache-probes', action='store_true', help='Measure page/statement idle windows, four warm sessions, Inspector and cold/warm128-session rotation')
    options = parser.parse_args()
    assert (1 if options.memory_only else 2) <= options.sessions <= 256
    assert options.memory_only or max(options.widths) <= options.sessions
    assert options.history > 0 and options.rounds > 0
    assert (options.history_checkpoints == sorted(set(options.history_checkpoints))
            and all(0 < depth <= options.history for depth in options.history_checkpoints))
    assert not options.idle_seconds or (options.memory_only and not options.cache_probes
        and options.idle_seconds == sorted(set(options.idle_seconds)) and min(options.idle_seconds) > 0)
    assert not options.cache_probes or options.sessions >= 8
    asyncio.run(main(options))
