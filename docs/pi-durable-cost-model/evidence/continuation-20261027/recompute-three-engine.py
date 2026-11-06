#!/usr/bin/env python3
"""Independent, standard-library audit of the fixed PR485 three-engine cohort.

Reads raw JSON and /proc text; never imports or runs a producer summarizer,
adapter, semantic oracle, benchmark, or decompressor. Existing semantic-oracle
records are checked for completeness and consistency, not reimplemented here.
SDK idle is not a final fsync barrier. No equal-durability ratios are calculated.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import re
import statistics
import subprocess
import sys
import unittest
from unittest.mock import patch

DSH_REV = '687f282e8625c6b8d11ff58211a90524be2e4317'
DSH_TREE = 'f127f0f96a815eac4dea336a2ae0d9cd0f2aa82d'
PI_REV = '90d863f7c1183b965f099855715d6e88c7b8a52c'
CASES = {'short-small': (8, 32), 'short-large': (64, 32), 'large': (0, 1), 'stream': (0, 1)}
ENGINES = ('rust', 'pi', 'deepseek')
METRICS = ('mean_turn_ms', 'turn_cpu_ms_per_turn', 'turn_write_mib_per_turn',
           'max_sampled_turn_pss_mib', 'max_sampled_turn_anon_pss_mib')
DEFAULT_ROOT = Path('/home/capi/.cache/rustx-pr485-next')
DEFAULT_HARNESS = Path('/home/capi/.codex/worktrees/pr485-pi-performance/rustX')
HARNESS_FILES = {'deepseek-compare.py', 'deepseek-read-log.mjs', 'deepseek-reference.py',
                 'deepseek-reference.md', 'deepseek-reference.test.py', 'deepseek-sdk-envelope.json'}
NATIVE_FILES = {'pnpm-lock.yaml', 'node_modules/.pnpm/lock.yaml', 'node_modules/tsx/dist/esm/index.mjs',
                'apps/cli/src/bin.ts', 'apps/cli/tsconfig.json', 'apps/cli/src/sdk-source.cordis.patch.yml',
                'packages/bundle/base/cordis.patch.yml', 'packages/bundle/sdk-app/cordis.patch.yml',
                'packages/sdk/server/src/server.ts', 'packages/sdk/server/src/index.ts',
                'packages/session/session-persistence-jsonl/src/storage.ts'}
LIMITS = [
    'This audit independently recomputes numeric metrics and checks evidence structure. It checks existing semantic-oracle records; it does not independently reimplement all request, output, and subscriber semantics.',
    'DeepSeek session/wait idle does not await final JSONL flush/fsync. Its turn latency is not a durable-settlement equivalent of Rust/Pi; no equal-durability ratios are produced.',
    'DeepSeek native close/resume evidence proves retained readable content. SDK disposal uses allSettled and may swallow failure, so exit zero is not a positive fsync-success or power-loss certificate.',
    'The standard-library audit checks retained decoded JSONL against SDK events and hashes the retained compressed file; it does not independently decode Zstd or rerun native resume.',
    'DeepSeek has native prompt/context/25 tools and committed-event/status publication without partial token text snapshots. Its reviewed captured envelope is not an independently authored native specification.',
    'DeepSeek CPU includes extant processes and waited children from sampled main-thread child traversal; I/O excludes exited descendants and is not complete tree/device I/O. Rust/Pi self CPU includes their in-process benchmark drivers. Memory is maximum sampled PSS, not peak.',
    'Source/loader/binary hashes establish file identity at audit time and against recorded identity. They do not by themselves attest that a Rust binary was built from the claimed source; external expected build identity can be supplied.',
]


def read(path):
    return json.loads(Path(path).read_text())


def digest(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def nonnegative(value, label):
    require(type(value) in (int, float) and math.isfinite(value) and value >= 0, label)
    return value


def index_rows(rows, fields):
    result = {}
    for row in rows:
        key = tuple(row[field] for field in fields)
        require(key not in result, f'duplicate row {key}')
        result[key] = row
    return result


def equal_numbers(actual, expected, label):
    if actual is None or expected is None:
        require(actual is expected, f'{label}: {actual!r} != {expected!r}')
    elif isinstance(expected, (list, tuple)):
        require(isinstance(actual, (list, tuple)) and len(actual) == len(expected), label)
        for index, (a, b) in enumerate(zip(actual, expected)):
            equal_numbers(a, b, f'{label}[{index}]')
    else:
        require(type(actual) in (int, float) and math.isfinite(actual) and
                math.isclose(actual, expected, rel_tol=1e-10, abs_tol=1e-9),
                f'{label}: {actual!r} != independently computed {expected!r}')


def parse_process(raw, ticks):
    """Linux proc fields by documented 1-based field number, not producer code."""
    values = {key: None for key in ('self_cpu_ms', 'waited_cpu_ms', 'write_bytes', 'pss_mib', 'anon_pss_mib')}
    errors, identity = {}, None
    try:
        text = raw['stat']
        left, _, rest = text.partition('(')
        _, close, tail = rest.rpartition(')')
        require(close and left.strip().isdigit(), 'malformed stat')
        fields = tail.split()
        field = lambda n: int(fields[n - 3])
        identity = (int(left.strip()), field(22))
        for key, numbers in (('self_cpu_ms', (14, 15)), ('waited_cpu_ms', (16, 17))):
            counts = [field(n) for n in numbers]
            require(all(n >= 0 for n in counts), 'negative CPU ticks')
            values[key] = sum(counts) * 1000 / ticks
    except (KeyError, TypeError, ValueError, IndexError, AttributeError) as error:
        errors['self_cpu_ms'] = errors['waited_cpu_ms'] = f'missing/invalid /proc stat: {error}'
    for source, output in (('io', {'write_bytes': 'write_bytes'}),
                           ('smaps_rollup', {'Pss': 'pss_mib', 'Pss_Anon': 'anon_pss_mib'})):
        try:
            parsed = dict(line.split(':', 1) for line in raw[source].splitlines() if ':' in line)
        except (KeyError, TypeError, AttributeError, ValueError) as error:
            for name in output.values():
                errors[name] = f'missing/invalid /proc {source}: {error}'
            continue
        for source_key, key in output.items():
            try:
                parts = parsed[source_key].split()
                number = int(parts[0])
                require(number >= 0, 'negative counter')
                if source == 'smaps_rollup':
                    require(parts[1:] == ['kB'], 'unexpected memory units')
                    number /= 1024
                values[key] = number
            except (KeyError, IndexError, ValueError) as error:
                errors[key] = f'missing/invalid /proc {source} {source_key}: {error}'
    return {'identity': identity, 'values': values, 'unavailable': errors}


def snapshot(tree, ticks, include_waited):
    require(type(ticks) is int and ticks > 0, 'invalid clock_ticks')
    if not isinstance(tree, dict) or not tree:
        return {'identities': None, 'members': {}, 'values': {}, 'unavailable': {'all': 'missing/empty process snapshot'}}
    members = {str(pid): parse_process(raw, ticks) for pid, raw in tree.items()}
    identities = {pid: item['identity'] for pid, item in members.items()}
    if any(value is None or (pid != 'self' and str(value[0]) != pid) for pid, value in identities.items()):
        identities = None
    values, errors = {}, {}
    mapping = {'cpu_ms': ('self_cpu_ms', 'waited_cpu_ms') if include_waited else ('self_cpu_ms',),
               'extant_process_self_cpu_ms': ('self_cpu_ms',), 'waited_child_cpu_ms': ('waited_cpu_ms',),
               'write_bytes': ('write_bytes',), 'pss_mib': ('pss_mib',), 'anon_pss_mib': ('anon_pss_mib',)}
    for key, components in mapping.items():
        failures = [f'{pid}: {item["unavailable"][part]}' for pid, item in members.items()
                    for part in components if part in item['unavailable']]
        if failures:
            values[key] = None
            errors[key] = '; '.join(failures)
        else:
            values[key] = sum(item['values'][part] for item in members.values() for part in components)
    return {'identities': identities, 'members': members, 'values': values, 'unavailable': errors}


def valid_delta(points, key):
    for point in points:
        if point['identities'] is None:
            return None, 'missing/invalid process identity; counter continuity cannot be established'
        if point['values'].get(key) is None:
            return None, point['unavailable'].get(key, point['unavailable'].get('all', 'missing counter'))
    if any(point['identities'] != points[0]['identities'] for point in points[1:]):
        return None, 'extant PID/start-time set changed; exited-process counters cannot be reconstructed from snapshots'
    if any(b['values'][key] < a['values'][key] for a, b in zip(points, points[1:])):
        return None, 'process counter decreased across measurement window'
    return points[-1]['values'][key] - points[0]['values'][key], None


def derive(data, engine, ticks):
    """Five metrics, only from raw turn timings and raw process snapshots."""
    samples = data['samples']
    case = data.get('case', data.get('scenario'))
    history, count = CASES[case]
    require((data['history'], data['measured']) == (history, count), 'history/measured count mismatch')
    require(len(samples) == history + count, 'sample count mismatch')
    require([s['turn'] for s in samples] == list(range(len(samples))), 'turn order mismatch')
    require([s['measured'] for s in samples] == [False] * history + [True] * count, 'measured flags mismatch')
    measured = samples[history:]
    native = engine == 'deepseek'
    unit, divisor = ('ns', 1e6) if native else ('us', 1000)
    for sample in samples:
        accepted = nonnegative(sample[f'accepted_{unit}'], 'invalid accepted duration')
        settled = nonnegative(sample[f'settled_{unit}'], 'invalid settled duration')
        require(accepted <= settled, 'accepted later than settled')
    if native:
        start = samples[history - 1]['after'] if history else measured[0]['before']
        trees = [start] + [s['after'] for s in measured]
    else:
        phase = {p['phase']: p for p in data['lifecycle']}
        start = samples[history - 1] if history else phase['after_initialization']
        trees = [{'self': p['process']} for p in [start] + measured]
    points = [snapshot(tree, ticks, native) for tree in trees]
    row = {'mean_turn_ms': statistics.mean(s[f'settled_{unit}'] / divisor for s in measured)}
    reasons = {}
    for source, key, scale in (('cpu_ms', 'turn_cpu_ms_per_turn', count),
                                ('write_bytes', 'turn_write_mib_per_turn', count * 2**20)):
        value, reason = valid_delta(points, source)
        row[key] = None if value is None else value / scale
        if reason:
            reasons[key] = reason
    for source, key in (('pss_mib', 'max_sampled_turn_pss_mib'), ('anon_pss_mib', 'max_sampled_turn_anon_pss_mib')):
        values = [p['values'].get(source) for p in points[1:]]
        if any(v is None for v in values):
            row[key] = None
            reasons[key] = 'at least one measured sample lacks valid ' + source + '; maximum is indeterminate'
        else:
            row[key] = max(values)
    row['unavailable_metrics'] = reasons
    row['raw_window_totals'] = {'start': points[0]['values'], 'end': points[-1]['values']}
    row['process_identities'] = {'start': points[0]['identities'], 'end': points[-1]['identities']}
    return row


def check_lifecycle(data, engine):
    if engine == 'deepseek':
        phases = ['before_provider_start', 'after_provider_start', 'after_initialization', 'after_turns',
                  'after_final_writer_close', 'after_native_resume', 'after_reopen_verification']
        clock = 'elapsed_ns'
    else:
        phases = ['before_initialization', 'after_provider_start', 'after_initialization', 'after_turns',
                  'after_late_snapshot', 'after_verification']
        if engine == 'rust':
            phases += ['after_shutdown', 'after_driver_drop']
        phases += ['after_final_writer_close', 'after_reopen_verification', 'after_cleanup']
        clock = 'elapsed_us'
    require([p['phase'] for p in data['lifecycle']] == phases, 'lifecycle phases/order mismatch')
    times = [nonnegative(p[clock], 'invalid lifecycle duration') for p in data['lifecycle']]
    require(times == sorted(times), 'nonmonotonic lifecycle clock')
    return {'phases': phases, 'first': times[0], 'last': times[-1], 'unit': clock}


def check_provider(report, count):
    require(report['ok'] is True and report['complete'] is True, 'provider incomplete/failed')
    require(report['failures'] == [] and report['unsettledSteps'] == [], 'provider failures/unsettled steps')
    for key in ('stepsTotal', 'stepsMatched', 'stepsSettled', 'requestCount'):
        require(report[key] == count, f'provider {key} mismatch')
    require(report['stepStates'] == ['settled'] * count, 'provider step state mismatch')
    require(len(report['requests']) == count, 'provider request list mismatch')
    require([r['index'] for r in report['requests']] == list(range(count)), 'provider request order mismatch')


def check_wire(path, expected, replies):
    frames = read(path)
    require(all(frame.get('jsonrpc') == '2.0' and 'error' not in frame for frame in frames), 'invalid/error SDK frame')
    responses = [frame for frame in frames if 'id' in frame]
    require([frame['id'] for frame in responses] == list(range(1, replies + 1)), 'SDK response sequence mismatch')
    require(responses[0]['result']['serverInfo']['name'] == 'deepseek-harness-sdk-runtime', 'unexpected SDK server')
    events = [frame['params']['event'] for frame in frames if frame.get('method') == 'session.event']
    require(events == expected, 'retained wire events differ from result events')
    require(responses[-1]['result'] == {}, 'missing successful SDK shutdown response')


def recorded_output_path(recorded, archived, raw_only):
    """Only raw-only mode permits relocation; recorded product names must match."""
    recorded = Path(recorded)
    require(recorded.is_absolute(), 'recorded product path is not absolute')
    if raw_only:
        require(recorded.name == archived.name, 'recorded/archived product filename mismatch')
        return str(recorded)
    require(recorded == archived, 'recorded product path differs from current product path')
    return str(archived)


def check_native(data, raw_path, manifest, raw_only=False):
    count = data['history'] + data['measured']
    require(data['accepted'] is True and data['profile'] == 'sdk', 'unaccepted/non-sdk DeepSeek result')
    require(data['observer_mode'] == 'mandatory-sdk-notifications', 'unexpected SDK observation profile')
    require(data['completion_boundary'] == 'SDK session/wait idle; does not await JSONL flush/fsync', 'incorrect SDK completion boundary')
    durability = data['durability']
    require(durability['compression'] == 'zstd' and durability['batch_max_delay_ms'] == 200 and
            durability['checkpoint_policy_mounted'] is True and durability['fsync_success_acknowledged'] is False,
            'native durability profile mismatch')
    oracle = data['oracle']
    require(oracle['version'] == 1 and oracle['ordered_requests'] == count + 1 and
            oracle['persisted_assistant_messages'] == count + 1, 'native oracle count/version mismatch')
    require(oracle['native_resume'] is True and oracle['unchanged_persisted_prefix'] is True and
            oracle['cross_engine_full_request_equivalence'] is False, 'native oracle claims mismatch')
    require(oracle['expected_history_origin'] == 'independently specified prompts and deterministic provider replies', 'missing independent history origin')
    require('reviewed first capture' in oracle['native_envelope_origin'] and len(oracle['differences']) >= 8, 'missing native limitations')
    before, after = data['persisted_before_reopen'], data['persisted_after_reopen']
    require(len(before) > 1 and after[:len(before)] == before, 'native restored prefix changed/missing')
    header = before[0]
    require(header['type'] == 'session' and header['version'] == 4 and header['id'] == 'benchmark' and
            header['cwd'] == data['workspace'], 'native log header mismatch')
    require([e['seq'] for e in after[1:]] == list(range(len(after) - 1)), 'native log event sequence mismatch')
    require(sum(e['type'] == 'assistant/message' for e in before[1:]) == count and
            sum(e['type'] == 'assistant/message' for e in after[1:]) == count + 1, 'native assistant event counts mismatch')
    require(data['first_notifications'] == before[1:], 'native first notifications/log mismatch')
    marker = after[len(before)]
    require(marker['type'] == 'session/end-seed' and marker['data'] == {}, 'native resume marker missing')
    require(data['verification_notifications'] == after[len(before) + 1:], 'native resumed notifications/log mismatch')
    for prefix, n in (('first', count), ('verification', 1)):
        exit_result = data[f'{prefix}_exit']
        require(exit_result['exit_code'] == 0, 'native nonzero exit')
        nonnegative(exit_result['close_ns'], 'invalid close timing')
        for value in exit_result['wait4'].values():
            nonnegative(value, 'invalid wait4 metric')
        report_key = 'provider_report' if prefix == 'first' else 'verification_provider_report'
        requests_key = 'requests' if prefix == 'first' else 'verification_requests'
        check_provider(data[report_key], n)
        require(data[report_key]['requests'] == data[requests_key], 'native provider requests mismatch')
        label = 'sample' if prefix == 'first' else 'verification'
        require(read(raw_path.with_name(label + '.provider.json')) == data[report_key], 'retained provider report mismatch')
        check_wire(raw_path.with_name(label + '.wire.json'), data[f'{prefix}_notifications'], 2 * n + (2 if prefix == 'first' else 3))
    checkout = Path(manifest['deepseek']['checkout'])
    recorded_patch = Path(data['workspace']).parent / 'profile.patch.json'
    require(recorded_patch.parent.name == raw_path.parent.name, 'native recorded product directory mismatch')
    patch_path = recorded_output_path(recorded_patch, raw_path.with_name('profile.patch.json'), raw_only)
    require(data['runtime_argv'] == ['node', '--import', str(checkout / 'node_modules/tsx/dist/esm/index.mjs'),
                                   str(checkout / 'apps/cli/src/bin.ts'), '--profile', 'sdk', '--patch',
                                   str(checkout / 'apps/cli/src/sdk-source.cordis.patch.yml'), '--patch',
                                   patch_path], 'native launcher/loader mismatch')
    logs = list(raw_path.parent.glob('home/sessions/**/session.v4.jsonl.zstd'))
    require(len(logs) == 1, 'missing/ambiguous retained native compressed log')
    closed = [p for p in data['closed_footprint'] if p['path'].endswith('/session.v4.jsonl.zstd')]
    require(len(closed) == 1 and closed[0]['logical'] > 0, 'missing first-close log footprint')
    require(logs[0].stat().st_size >= closed[0]['logical'], 'retained native log shorter than first close')
    with logs[0].open('rb') as stream:
        require(stream.read(4) == bytes.fromhex('28b52ffd'), 'native log lacks Zstd magic')
    return {'closed_log_events': len(before) - 1, 'reopened_log_events': len(after) - 1,
            'compressed_log_sha256': digest(logs[0]), 'compressed_log_bytes': logs[0].stat().st_size,
            'checked_existing_oracle': True, 'independently_reimplemented_semantic_oracle': False,
            'fsync_success_acknowledged': False}


def check_existing(data, raw_path, engine, manifest, raw_only=False):
    count = data['history'] + data['measured']
    require(data['oracle'] == 'passed' and data['workload_version'] == 3, 'existing product oracle/version failed')
    require(data['observer_mode'] == 'live-subscriber' and
            all(p['observer_mode'] == 'live-subscriber' for p in data['samples']), 'subscriber mode mismatch')
    process = read(raw_path.with_suffix('.process.json'))
    require(process['accepted'] is True and process['product_exit'] == 0 and process['provider_exit'] == 0, 'product process failed')
    require(process['case'] == data.get('case', data.get('scenario')) and process['observer_mode'] == 'live-subscriber', 'process case/profile mismatch')
    check_provider(process['provider_report'], count)
    if engine == 'rust':
        require(process['argv'] == [manifest['rust']['path'], 'agent_loop::runtime_benchmark::measure_runtime', '--exact', '--ignored', '--nocapture'], 'Rust binary launch identity mismatch')
        require(data['requests'] == count and data['verified_live_requests'] == count and
                data['verified_reopened_requests'] == count and data['oracle_version'] == 2, 'Rust oracle count mismatch')
        require(data['live_requests'] == data['reopened_requests'] and len(data['live_requests']) == count, 'Rust reopened context mismatch')
        require(data['wire_requests'] == process['provider_report']['requests'], 'Rust provider capture mismatch')
        obs = data['observer_validation']
        require(obs['oracle_version'] == 2 and obs['completed_attempts'] == count and len(obs['explicit_settlements']) == count, 'Rust observer oracle mismatch')
        require(isinstance(data['canonical'], list) and isinstance(data['late_snapshot'], dict), 'Rust persistence/snapshot evidence missing')
    else:
        output_path = recorded_output_path(process['argv'][-1], raw_path, raw_only)
        require(process['argv'] == ['node', '--conditions=source', 'benchmarks/runtime/pi-http.mjs', manifest['pi']['path'],
                                   data['scenario'], output_path], 'Pi launch identity mismatch')
        require(data['revision'] == PI_REV and data['observer_oracle_version'] == 2 and data['observer_failure'] is None, 'Pi revision/observer oracle mismatch')
        require(data['requests'] == process['provider_report']['requests'], 'Pi provider capture mismatch')
        require([r['messages'] for r in data['captures']] == data['reopenedRequests'] and
                len(data['captures']) == count, 'Pi reopened context mismatch')
        require(len(data['observer_explicit_run_ends']) == count and all(v['valid'] is True for v in data['observer_versions']), 'Pi observer validity/count mismatch')
        require(isinstance(data['entries'], list) and isinstance(data['lateSnapshot'], dict), 'Pi persistence/snapshot evidence missing')
    require(data['closed_footprint'], 'missing native closed footprint')
    return {'checked_existing_oracle': True, 'independently_reimplemented_semantic_oracle': False,
            'reopened_requests': count, 'provider_requests': count}


def git(path, *args):
    return subprocess.check_output(['git', '-C', str(path), *args], text=True, stderr=subprocess.PIPE).strip()


def git_blob_sha256(path, revision, filename):
    # Keep exact bytes, including trailing newlines; the textual git helper strips.
    blob = subprocess.check_output(['git', '-C', str(path), 'show', f'{revision}:{filename}'], stderr=subprocess.PIPE)
    return hashlib.sha256(blob).hexdigest()


def check_identity(manifest, harness, freeze, expected_binary, expected_revision, raw_only=False):
    """Archive consistency always; external file/commit checks only in strict mode."""
    require(manifest['clock_ticks'] > 0 and manifest['repetitions'] == 3 and manifest['cases'] == list(CASES), 'cohort configuration mismatch')
    require(manifest['order'] == 'rotate rust/pi/deepseek by repetition' and manifest['deepseek_profile'] == 'sdk', 'rotation/profile mismatch')
    require(manifest['durability_equivalence'] is False and 'absent' in manifest['cross_engine_lifecycle_ratios'], 'incorrect equivalence claims')
    require(re.fullmatch('[0-9a-f]{64}', manifest['rust']['sha256']), 'invalid recorded Rust binary hash')
    if expected_binary:
        require(manifest['rust']['sha256'] == expected_binary, 'Rust binary differs from independent expected build hash')
    native = manifest['deepseek']
    require(native['revision'] == DSH_REV and native['tree'] == DSH_TREE, 'DeepSeek revision/tree mismatch')
    require(manifest['pi']['revision'] == PI_REV, 'Pi revision mismatch')
    require(native['launch'] == 'supported source CLI via tsx ESM; no built JavaScript substitution', 'native launch profile changed')
    require(native['node'] == 'v24.19.0', 'native Node version differs from setup identity')
    require(NATIVE_FILES <= native['files'].keys() and any(f.endswith('.node') for f in native['files']), 'native source/loader identity incomplete')
    for name, sha in native['files'].items():
        require(re.fullmatch('[0-9a-f]{64}', sha), f'invalid recorded DeepSeek file hash: {name}')
    require(native['files']['pnpm-lock.yaml'] == native['files']['node_modules/.pnpm/lock.yaml'], 'frozen dependency lock mismatch')
    measured_revision = manifest['harness']['revision']
    require(re.fullmatch('[0-9a-f]{40}', measured_revision), 'invalid measurement harness revision')
    if expected_revision:
        require(measured_revision == expected_revision, 'harness revision differs from independent expected revision')
    frozen = read(freeze)['files']
    require(set(manifest['harness']['files']) == HARNESS_FILES, 'expected exact six DeepSeek harness files')
    for name, sha in manifest['harness']['files'].items():
        expected = frozen['benchmarks/runtime/' + name]
        require(re.fullmatch('[0-9a-f]{64}', sha) and expected['sha256'] == sha, f'harness file not equal to pre-measurement reviewed freeze: {name}')
    for key, filename in (('adapter_sha256', 'deepseek-reference.py'), ('verification_reader_sha256', 'deepseek-read-log.mjs'), ('native_envelope_sha256', 'deepseek-sdk-envelope.json')):
        require(native[key] == manifest['harness']['files'][filename], f'identity cross-reference mismatch: {key}')

    current_head = None
    if not raw_only:
        require(digest(Path(manifest['rust']['path'])) == manifest['rust']['sha256'], 'Rust binary hash mismatch')
        for identity, pin in ((native, DSH_REV), (manifest['pi'], PI_REV)):
            checkout = Path(identity.get('checkout', identity.get('path')))
            require(git(checkout, 'rev-parse', 'HEAD') == pin, 'current checkout revision mismatch')
            require(git(checkout, 'diff', '--name-only', 'HEAD') == '', 'current checkout tracked files changed')
        require(git(native['checkout'], 'rev-parse', 'HEAD^{tree}') == native['tree'], 'current DeepSeek tree mismatch')
        for name, sha in native['files'].items():
            require(digest(Path(native['checkout']) / name) == sha, f'DeepSeek file identity mismatch: {name}')
        require(git(harness, 'rev-parse', '--verify', measured_revision + '^{commit}') == measured_revision,
                'measurement harness revision is not an available local commit')
        current_head = git(harness, 'rev-parse', 'HEAD')
        for name, sha in manifest['harness']['files'].items():
            relative = 'benchmarks/runtime/' + name
            require(git_blob_sha256(harness, measured_revision, relative) == sha,
                    f'measurement commit harness file hash mismatch: {name}')
            require(digest(harness / relative) == sha, f'current harness file changed: {name}')

    return {'mode': 'raw-only' if raw_only else 'strict', 'strict_identity_verified': not raw_only,
            'archive_identity_consistency_verified': True,
            'external_actual_binary_checked': not raw_only, 'external_actual_source_checked': not raw_only,
            'external_check_omission_reason': 'raw-only: no external binary, Git checkout, or dependency files accessed' if raw_only else None,
            'measurement_commit_verified': not raw_only, 'measurement_commit_files_hashed': 0 if raw_only else 6,
            'current_harness_head': current_head,
            'current_head_differs_from_measurement': None if raw_only else current_head != measured_revision,
            'current_harness_file_bytes_match': None if raw_only else True,
            'rust_binary_sha256': manifest['rust']['sha256'], 'harness_revision': measured_revision,
            'pi_revision': PI_REV, 'deepseek_revision': DSH_REV, 'deepseek_tree': DSH_TREE,
            'native_files_hashed': 0 if raw_only else len(native['files']), 'harness_files_hashed': 0 if raw_only else 6,
            'expected_binary_hash_compared_to_manifest': bool(expected_binary),
            'expected_revision_compared_to_manifest': bool(expected_revision)}


def aggregate(rows):
    groups = []
    for case in CASES:
        for engine in ENGINES:
            matched = [r for r in rows if r['case'] == case and r['engine'] == engine]
            values = {k: [r[k] for r in matched if r[k] is not None] for k in METRICS}
            groups.append({'case': case, 'engine': engine, 'samples': len(matched),
                           'available_samples': {k: len(v) for k, v in values.items()},
                           'medians': {k: statistics.median(v) if v else None for k, v in values.items()},
                           'ranges': {k: [min(v), max(v)] if v else None for k, v in values.items()},
                           'unavailable': [{'repetition': r['repetition'], 'reasons': r['unavailable_metrics']}
                                           for r in matched if r['unavailable_metrics']]})
    return groups


def audit(args):
    directory = args.dataset.resolve()
    manifest = read(directory / 'manifest.json')
    report = {'audit_version': 2, 'dataset': str(directory), 'accepted': False, 'errors': [],
              'mode': 'raw-only' if args.raw_only else 'strict', 'strict_identity_verified': False,
              'acceptance_scope': 'archived evidence and arithmetic only; external identity unchecked' if args.raw_only else 'archived evidence, arithmetic, and strict external identity',
              'limitations': LIMITS, 'durability_equivalence': False, 'rows': [], 'groups': [],
              'audit_script_sha256': digest(__file__), 'manifest_sha256': digest(directory / 'manifest.json')}
    try:
        report['identity'] = check_identity(manifest, args.harness_root, args.freeze,
                                             args.expected_rust_sha256, args.expected_harness_revision, args.raw_only)
        report['strict_identity_verified'] = report['identity']['strict_identity_verified']
    except Exception as error:
        report['errors'].append(f'identity: {error}')
    expected = {(r, c, e) for r in range(3) for c in CASES for e in ENGINES}
    for filename in ('records.json', 'metrics.json', 'product-metrics.json'):
        try:
            original_rows = read(directory / filename)
            indexed = index_rows(original_rows, ('repetition', 'case', 'engine'))
            require(set(indexed) == expected and len(indexed) == 36, f'{filename}: expected exactly 36 unique products')
            require(all(row['accepted'] is True and row.get('error') is None for row in indexed.values()), f'{filename}: unaccepted product present')
            if filename == 'records.json':
                rotation = [(r, c, e) for r in range(3) for c in CASES for e in (ENGINES[r:] + ENGINES[:r])]
                require([tuple(row[k] for k in ('repetition', 'case', 'engine')) for row in original_rows] == rotation, 'recorded rotation order mismatch')
        except Exception as error:
            report['errors'].append(f'{filename}: {error}')
    try:
        actual_processes = {p.name for p in directory.glob('*.process.json')}
        require(actual_processes == {f'{r}-{c}-{e}.process.json' for r in range(3) for c in CASES for e in ('rust', 'pi')}, 'expected exactly 24 Rust/Pi process results')
        require({str(p.relative_to(directory)) for p in directory.glob('*-deepseek/result.json')} ==
                {f'{r}-{c}-deepseek/result.json' for r in range(3) for c in CASES}, 'expected exactly 12 DeepSeek results')
        require({p.name for p in directory.glob('*-rust-pi-comparison.json')} ==
                {f'{r}-{c}-rust-pi-comparison.json' for r in range(3) for c in CASES}, 'expected exactly 12 Rust/Pi comparison files')
    except Exception as error:
        report['errors'].append(f'file completeness: {error}')
    comparison_count = 0
    for repetition in range(3):
        for case, (history, measured) in CASES.items():
            try:
                comparison = read(directory / f'{repetition}-{case}-rust-pi-comparison.json')
                require(comparison['case'] == case and comparison['workload_version'] == 3 and
                        comparison['model_time'] == '2026-10-19 00:00:00' and comparison['observer_mode'] == 'live-subscriber', 'comparison metadata mismatch')
                checked = comparison['checked']
                require(len(checked) == history + measured and [v['index'] for v in checked] == list(range(history + measured)), 'comparison request count/order mismatch')
                require(all(v['coreSemanticEqual'] is True and v['fullSemanticRequestEqual'] is False for v in checked), 'core oracle failed or full-request equivalence claim changed')
                require(all(v['rustToolNames'] == ['read', 'write', 'edit', 'glob', 'grep', 'bash', 'todo'] and
                            v['piToolNames'] == ['read', 'write', 'edit', 'bash'] for v in checked), 'native tool roster changed')
                comparison_count += 1
            except Exception as error:
                report['errors'].append(f'{repetition}/{case} Rust-Pi comparison: {error}')
            for engine in ENGINES:
                stem = directory / f'{repetition}-{case}-{engine}'
                path = stem / 'result.json' if engine == 'deepseek' else stem.with_suffix('.json')
                try:
                    data = read(path)
                    require(data.get('case', data.get('scenario')) == case and data['model_time'] == '2026-10-19 00:00:00', 'product case/model-time mismatch')
                    lifecycle = check_lifecycle(data, engine)
                    evidence = check_native(data, path, manifest, args.raw_only) if engine == 'deepseek' else check_existing(data, path, engine, manifest, args.raw_only)
                    row = derive(data, engine, manifest['clock_ticks'])
                    row.update(case=case, engine=engine, repetition=repetition, lifecycle=lifecycle,
                               evidence=evidence, raw_result_sha256=digest(path))
                    report['rows'].append(row)
                except Exception as error:
                    report['errors'].append(f'{repetition}/{case}/{engine}: {error}')
    report['valid_comparisons'] = comparison_count
    report['derived_products'] = len(report['rows'])
    report['groups'] = aggregate(report['rows'])
    for filename in ('metrics.json', 'product-metrics.json'):
        try:
            indexed = index_rows(read(directory / filename), ('repetition', 'case', 'engine'))
            for row in report['rows']:
                key = (row['repetition'], row['case'], row['engine'])
                original = indexed[key]
                for metric in METRICS:
                    equal_numbers(original[metric], row[metric], f'{filename}/{key}/{metric}')
                if row['engine'] == 'deepseek':
                    for side, totals in row['raw_window_totals'].items():
                        for metric, value in totals.items():
                            equal_numbers(original['raw_window_totals'][side][metric], value, f'{filename}/{key}/{side}/{metric}')
                    for metric, source in (('turn_cpu_ms_per_turn', 'cpu_ms'), ('turn_write_mib_per_turn', 'write_bytes')):
                        require(original['unavailable_metrics'].get(source) == row['unavailable_metrics'].get(metric), f'{filename}/{key}: null reason mismatch')
                    require(original['completion_boundary'] == 'SDK session/wait idle; does not await JSONL flush/fsync', 'producer labels SDK idle as durability barrier')
                else:
                    require(original['completion_boundary'] == 'native durable settlement plus live subscriber completion', 'producer native boundary changed')
        except Exception as error:
            report['errors'].append(f'producer metric comparison: {error}')
    try:
        producer = read(directory / 'summary.json')
        require(producer['durability_equivalence'] is False, 'producer summary claims durability equivalence')
        groups = index_rows(producer['groups'], ('case', 'engine'))
        require(len(groups) == 12 and set(groups) == {(c, e) for c in CASES for e in ENGINES}, 'producer summary groups incomplete')
        for row in report['groups']:
            original = groups[(row['case'], row['engine'])]
            require(original['samples'] == row['samples'] == 3, 'group does not contain three admitted products')
            require(original['available_samples'] == row['available_samples'], 'available sample count mismatch')
            for metric in METRICS:
                for field in ('medians', 'ranges'):
                    equal_numbers(original[field][metric], row[field][metric], f'summary/{row["case"]}/{row["engine"]}/{field}/{metric}')
    except Exception as error:
        report['errors'].append(f'producer summary comparison: {error}')
    report['accepted'] = not report['errors'] and comparison_count == 12 and len(report['rows']) == 36
    return report


class RecomputeTests(unittest.TestCase):
    def identity_fixture(self):
        sha, revision = 'a' * 64, 'b' * 40
        manifest = {'clock_ticks': 100, 'repetitions': 3, 'cases': list(CASES),
                    'order': 'rotate rust/pi/deepseek by repetition', 'deepseek_profile': 'sdk',
                    'durability_equivalence': False, 'cross_engine_lifecycle_ratios': 'intentionally absent',
                    'rust': {'path': '/external/binary', 'sha256': sha},
                    'pi': {'path': '/external/pi', 'revision': PI_REV},
                    'deepseek': {'checkout': '/external/deepseek', 'revision': DSH_REV, 'tree': DSH_TREE,
                                 'launch': 'supported source CLI via tsx ESM; no built JavaScript substitution',
                                 'node': 'v24.19.0', 'files': {name: sha for name in NATIVE_FILES | {'native/test.node'}},
                                 'adapter_sha256': sha, 'verification_reader_sha256': sha, 'native_envelope_sha256': sha},
                    'harness': {'revision': revision, 'files': {name: sha for name in HARNESS_FILES}}}
        freeze = {'files': {'benchmarks/runtime/' + name: {'sha256': sha} for name in HARNESS_FILES}}
        return manifest, freeze

    def fake_git(self, path, *args):
        if args[0] == 'diff':
            return ''
        if args[-1] == 'HEAD^{tree}':
            return DSH_TREE
        if '--verify' in args:
            return 'b' * 40
        return {'/external/pi': PI_REV, '/external/deepseek': DSH_REV, '/external/harness': 'c' * 40}[str(path)]

    def test_raw_only_never_accesses_external_identity(self):
        manifest, frozen = self.identity_fixture()
        with patch(__name__ + '.read', return_value=frozen), \
                patch(__name__ + '.digest', side_effect=AssertionError('external file access')), \
                patch('subprocess.check_output', side_effect=AssertionError('external Git access')):
            result = check_identity(manifest, Path('/external/harness'), Path('/archive/freeze'),
                                    'a' * 64, 'b' * 40, raw_only=True)
        self.assertFalse(result['strict_identity_verified'])
        self.assertFalse(result['external_actual_binary_checked'])
        self.assertFalse(result['external_actual_source_checked'])
        self.assertEqual(result['measurement_commit_files_hashed'], 0)
        self.assertIsNone(result['current_harness_head'])
        self.assertTrue(result['expected_revision_compared_to_manifest'])

    def test_strict_allows_new_head_with_identical_measured_files(self):
        manifest, frozen = self.identity_fixture()
        with patch(__name__ + '.read', return_value=frozen), \
                patch(__name__ + '.digest', return_value='a' * 64), \
                patch(__name__ + '.git', side_effect=self.fake_git), \
                patch(__name__ + '.git_blob_sha256', return_value='a' * 64) as blobs:
            result = check_identity(manifest, Path('/external/harness'), Path('/archive/freeze'), None, 'b' * 40)
        self.assertTrue(result['strict_identity_verified'])
        self.assertTrue(result['current_head_differs_from_measurement'])
        self.assertEqual(result['measurement_commit_files_hashed'], 6)
        self.assertEqual(blobs.call_count, 6)
        self.assertTrue(all(call.args[1] == 'b' * 40 for call in blobs.call_args_list))

    def test_strict_rejects_wrong_measured_commit_file(self):
        manifest, frozen = self.identity_fixture()
        with patch(__name__ + '.read', return_value=frozen), \
                patch(__name__ + '.digest', return_value='a' * 64), \
                patch(__name__ + '.git', side_effect=self.fake_git), \
                patch(__name__ + '.git_blob_sha256', return_value='d' * 64):
            with self.assertRaisesRegex(ValueError, 'measurement commit harness file hash mismatch'):
                check_identity(manifest, Path('/external/harness'), Path('/archive/freeze'), None, None)

    def test_strict_rejects_changed_current_harness_file(self):
        manifest, frozen = self.identity_fixture()
        def file_hash(path):
            return ('d' if str(path).startswith('/external/harness/') else 'a') * 64
        with patch(__name__ + '.read', return_value=frozen), \
                patch(__name__ + '.digest', side_effect=file_hash), \
                patch(__name__ + '.git', side_effect=self.fake_git), \
                patch(__name__ + '.git_blob_sha256', return_value='a' * 64):
            with self.assertRaisesRegex(ValueError, 'current harness file changed'):
                check_identity(manifest, Path('/external/harness'), Path('/archive/freeze'), None, None)

    def test_expected_revision_still_pins_raw_archive(self):
        manifest, frozen = self.identity_fixture()
        with patch(__name__ + '.read', return_value=frozen):
            with self.assertRaisesRegex(ValueError, 'independent expected revision'):
                check_identity(manifest, Path('/external/harness'), Path('/archive/freeze'), None, 'c' * 40, True)

    def test_git_blob_hash_keeps_trailing_newline(self):
        with patch('subprocess.check_output', return_value=b'content\n'):
            self.assertEqual(git_blob_sha256('/repo', 'b' * 40, 'file'), hashlib.sha256(b'content\n').hexdigest())

    def test_relocated_raw_archive_paths(self):
        archived = Path('/archive/0-large-pi.json')
        recorded = '/original/0-large-pi.json'
        self.assertEqual(recorded_output_path(recorded, archived, True), recorded)
        with self.assertRaisesRegex(ValueError, 'differs'):
            recorded_output_path(recorded, archived, False)
        with self.assertRaisesRegex(ValueError, 'filename mismatch'):
            recorded_output_path('/original/1-large-pi.json', archived, True)

    def point(self, cpu=10, write=4096, pid=100, start=9):
        fields = ['0'] * 50
        fields[0] = 'S'
        for n, value in ((14, cpu), (15, 5), (16, 2), (17, 1), (22, start)):
            fields[n - 3] = str(value)
        return {'stat': f'{pid} (odd (name)) ' + ' '.join(fields), 'io': f'write_bytes: {write}\n',
                'smaps_rollup': 'Pss: 2048 kB\nPss_Anon: 1024 kB\n'}

    def test_proc_fields(self):
        point = parse_process(self.point(), 100)
        self.assertEqual(point['identity'], (100, 9))
        self.assertEqual(point['values'], {'self_cpu_ms': 150, 'waited_cpu_ms': 30, 'write_bytes': 4096, 'pss_mib': 2, 'anon_pss_mib': 1})

    def test_missing_counter(self):
        raw = self.point(); raw['io'] = None
        point = snapshot({'100': raw}, 100, True)
        self.assertIsNone(point['values']['write_bytes'])
        self.assertIn('missing/invalid', point['unavailable']['write_bytes'])

    def test_changed_identity(self):
        a = snapshot({'100': self.point()}, 100, True)
        b = snapshot({'100': self.point(cpu=20, start=10)}, 100, True)
        value, reason = valid_delta([a, b], 'cpu_ms')
        self.assertIsNone(value); self.assertIn('PID/start-time set changed', reason)

    def test_internal_counter_reset(self):
        points = [snapshot({'100': self.point(cpu=n)}, 100, True) for n in (10, 5, 20)]
        value, reason = valid_delta(points, 'cpu_ms')
        self.assertIsNone(value); self.assertIn('decreased', reason)

    def test_negative_input(self):
        point = parse_process(self.point(write=-1), 100)
        self.assertIsNone(point['values']['write_bytes'])

    def test_null_not_zero(self):
        with self.assertRaises(ValueError):
            equal_numbers(0, None, 'null')

    def test_aggregate_null_and_range(self):
        rows = []
        for repetition, value in enumerate((3, None, 1)):
            rows.append({'case': 'large', 'engine': 'deepseek', 'repetition': repetition,
                         **{k: value for k in METRICS}, 'unavailable_metrics': {'cpu': 'missing'} if value is None else {}})
        group = next(g for g in aggregate(rows) if g['case'] == 'large' and g['engine'] == 'deepseek')
        self.assertEqual(group['available_samples'][METRICS[0]], 2)
        self.assertEqual(group['medians'][METRICS[0]], 2)
        self.assertEqual(group['ranges'][METRICS[0]], [1, 3])
        self.assertEqual(group['unavailable'][0]['reasons'], {'cpu': 'missing'})

    def test_duplicates_rejected(self):
        with self.assertRaises(ValueError):
            index_rows([{'x': 1}, {'x': 1}], ('x',))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dataset', type=Path, default=DEFAULT_ROOT / 'three-engine-final')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--harness-root', type=Path, default=DEFAULT_HARNESS)
    parser.add_argument('--freeze', type=Path, default=DEFAULT_ROOT / 'deepseek-setup/final-source-identity.json')
    parser.add_argument('--expected-rust-sha256')
    parser.add_argument('--expected-harness-revision')
    parser.add_argument('--raw-only', action='store_true',
                        help='check archived evidence/manifest/freeze only; skip all external binary, Git checkout and dependency-file checks; never verifies strict identity')
    parser.add_argument('--single', type=Path, help='derive one existing raw result; does not admit a formal cohort')
    parser.add_argument('--engine', choices=ENGINES)
    parser.add_argument('--ticks', type=int, default=100)
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(RecomputeTests))
        return 0 if result.wasSuccessful() else 1
    if args.single:
        require(args.engine is not None, '--single needs --engine')
        report = {'single_derive_only': True, 'raw_file': str(args.single), 'engine': args.engine,
                  'metrics': derive(read(args.single), args.engine, args.ticks), 'limitations': LIMITS}
    else:
        report = audit(args)
    rendered = json.dumps(report, indent=2, allow_nan=False) + '\n'
    if args.output:
        args.output.write_text(rendered)
        print(json.dumps({'output': str(args.output), 'accepted': report.get('accepted'),
                          'mode': report.get('mode'), 'strict_identity_verified': report.get('strict_identity_verified', False),
                          'derived_products': report.get('derived_products'), 'valid_comparisons': report.get('valid_comparisons'),
                          'errors': report.get('errors', [])}))
    else:
        sys.stdout.write(rendered)
    return 0 if report.get('accepted', args.single is not None) else 1


if __name__ == '__main__':
    raise SystemExit(main())
