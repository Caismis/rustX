#!/usr/bin/env python3
"""Read existing diagnostic data only; do not import producer analyzers or run products."""
import ast
import collections
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess

ROOT = Path(__file__).resolve().parent
REPO = Path('/home/capi/.codex/worktrees/pr485-pi-performance/rustX')
PI = Path('/home/capi/Documents/codes/pi-rustx-benchmark')
SHA = '8ec26a4a08bde085f46ee40159d3870f7ead675bec39f42cb4f3e55df1d1d233'
PI_REV = '90d863f7c1183b965f099855715d6e88c7b8a52c'

def read(path):
    return json.loads(path.read_text())

def sha(path):
    with path.open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()

def git(path, *args):
    return subprocess.check_output(['git', '-C', str(path), *args], text=True).strip()

plan = read(ROOT/'plan.json')
assert plan['baseline_sha256'] == SHA
assert sha(Path(plan['baseline_binary'])) == SHA
build = read(Path(plan['baseline_binary']).parent/'identity.json')
assert build['sha256'] == SHA and build['verified'] and build['build_exit'] == 0
assert build['head'] == plan['baseline_production_source']
assert not git(REPO, 'diff', '--name-only', build['head'], plan['baseline_head'], '--', 'src', 'Cargo.toml', 'Cargo.lock')
input_hashes = {name: sha(REPO/name) for name in plan['input_sha256']}
assert input_hashes == plan['input_sha256']
assert git(PI, 'rev-parse', 'HEAD') == PI_REV
assert not git(PI, 'diff', '--name-only', 'HEAD')
identity = read(ROOT/'wal-identity.json')
assert identity['rust_sha256'] == SHA and identity['pi_revision'] == PI_REV
report = {'identity': {'rust_sha256': SHA, 'production_revision': build['head'],
    'baseline_head_same_production': True, 'plan_inputs_match_current_files': True,
    'pi_revision': PI_REV, 'pi_tracked_changes': [],
    'pi_status_porcelain': git(PI, 'status', '--porcelain'),
    'identity_scope': 'Current binary rehash and stored build identity; profile per-product manifests. WAL has only post-run global identity, not per-product binary/driver freezes.'},
    'products': [], 'cpu': [], 'wal': [], 'comparisons': []}

def product(directory):
    proc, sample = read(directory/'sample.process.json'), read(directory/'sample.json')
    assert proc['accepted'] and proc['product_exit'] == proc['provider_exit'] == 0
    assert sample['oracle'] == 'passed' and sample['workload_version'] == 3
    assert sample['model_time'] == '2026-10-19 00:00:00'
    case = 'short-small' if 'small' in directory.name else 'short-large'
    assert proc['case'] == case
    assert sample['history'] == (8 if case == 'short-small' else 64)
    expected = sample['history'] + 32
    assert sample['measured'] == 32 and len(sample['samples']) == expected
    provider = proc['provider_report']
    assert provider['ok'] and provider['complete'] and not provider['failures']
    assert all(provider[k] == expected for k in ['stepsTotal', 'stepsMatched', 'stepsSettled', 'requestCount'])
    assert provider['stepStates'] == ['settled'] * expected
    wire = sample.get('wire_requests', sample.get('requests'))
    assert wire == provider['requests'] and len(wire) == expected
    assert [p['turn'] for p in sample['samples']] == list(range(expected))
    assert [p['measured'] for p in sample['samples']] == [i >= sample['history'] for i in range(expected)]
    phases = [p['phase'] for p in sample['lifecycle']]
    assert phases[-1] == 'after_cleanup' and len(phases) == len(set(phases))
    assert phases.index('after_turns') < phases.index('after_late_snapshot') < phases.index('after_verification') < phases.index('after_final_writer_close') < phases.index('after_reopen_verification')
    elapsed = [p['elapsed_us'] for p in sample['lifecycle']]
    assert elapsed == sorted(elapsed)
    assert all(p['process'][name] is not None for p in sample['lifecycle'] for name in ['stat', 'io', 'smaps_rollup'])
    if 'measurement_version' in sample:
        assert sample['verified_live_requests'] == sample['verified_reopened_requests'] == expected
        assert sample['live_requests'] == sample['reopened_requests'] and len(sample['live_requests']) == expected
        assert sample['late_snapshot']['messages'] == sample['canonical']
    else:
        assert sample['revision'] == PI_REV
        assert [p['messages'] for p in sample['captures']] == sample['reopenedRequests']
        assert len(sample['captures']) == expected
        assert sample['lateSnapshot']['entries'] == sample['entries']
    report['products'].append({'directory': directory.name, 'accepted': True, 'requests': expected,
        'measured_turns': 32, 'observer_mode': proc['observer_mode'], 'oracle_record': 'passed',
        'live_reopen_equal': True, 'phases': phases})

commands = read(ROOT/'profile-commands.json')
assert len(commands) == 8 and all(p['status'] == 'complete' and p['exit'] == 0 for p in commands)
assert all(a['end_utc'] <= b['start_utc'] for a, b in zip(commands, commands[1:]))
cpu_stored = {p['name']: p for p in read(ROOT/'cpu-counts.json')}
for directory in sorted(ROOT.glob('profile-d-*')):
    if not directory.is_dir():
        continue
    manifest = read(directory/'manifest.json')
    assert manifest['binary_sha256'] == SHA
    assert manifest['case'] == ('short-small' if 'small' in directory.name else 'short-large')
    assert manifest['observer_mode'] == read(directory/'sample.process.json')['observer_mode']
    product(directory)
    if manifest['mode'] != 'cpu':
        continue
    raw = directory/'before_measured_turns.cpu'
    data = raw.read_bytes()
    header = struct.unpack_from('<5Q', data)
    assert header[:3] == (0, 3, 0) and header[3] == 4000
    offset, total, records = 40, 0, 0
    while True:
        count, depth = struct.unpack_from('<QQ', data, offset)
        offset += 16
        assert depth <= 4096
        pcs = struct.unpack_from('<' + 'Q'*depth, data, offset)
        offset += depth*8
        if count == 0 and pcs == (0,):
            break
        assert count > 0 and depth > 0
        records += 1
        total += count
    assert b' r-xp ' in data[offset:]
    collapsed = []
    for line in (directory/'steady-collapsed.txt').read_text().splitlines():
        stack, count = line.rsplit(' ', 1)
        collapsed.append((stack, int(count)))
    assert sum(n for _, n in collapsed) == total
    stored = cpu_stored[directory.name]
    assert stored['samples'] == total and stored['interval_us'] == header[3] and stored['raw_sha256'] == sha(raw)
    cumulative = (directory/'steady-cumulative.txt').read_text().splitlines()
    assert cumulative[0] == f'Total: {total} samples'
    selected = {}
    for symbol in ['PublicationDraft>::encode', 'CheckpointSeed>::checkpoint_unfitted', 'index_request_read_layouts', 'sqlite3LockAndPrepare']:
        inclusive = sum(n for stack, n in collapsed if symbol in stack)
        matches = [row for row in stored['rows'] if symbol in row['symbol']]
        assert len(matches) == 1 and matches[0]['inclusive'] == inclusive
        selected[symbol] = inclusive
    report['cpu'].append({'directory': directory.name, 'denominator_samples': total,
        'requested_interval_us': header[3], 'raw_records': records, 'raw_sha256': sha(raw),
        'selected_inclusive_samples': selected,
        'phase': 'before_measured_turns -> after_turns', 'includes_driver_and_profiler_boundary_work': True})

stored_wal = read(ROOT/'wal-summary-d.json')
for case in ['short-small', 'short-large']:
    comparison = read(ROOT/f'wal-d-{case}-comparison.json')
    assert len(comparison['checked']) == (40 if case == 'short-small' else 96)
    assert all(p['coreSemanticEqual'] and not p['fullSemanticRequestEqual'] for p in comparison['checked'])
    report['comparisons'].append({'case': case, 'checked': len(comparison['checked']), 'core_all_equal': True, 'full_request_all_equal': False})
    for engine in ['rust', 'pi']:
        name = f'{case}-{engine}'
        directory = ROOT/('wal-d-'+name)
        product(directory)
        before = read(directory/'before_measured_turns-database.json')[0]
        after = read(directory/'after_turns-database.json')[0]
        owners = collections.defaultdict(set)
        for snapshot in [before, after]:
            for table, page, *_ in snapshot['pages']:
                owners[page].add(table)
        pending, phase, boundaries = {}, 'startup', []
        page_sizes, write_sizes, page_counts = set(), collections.Counter(), collections.Counter()
        commits, transaction_frames, current_frames = collections.Counter(), [], 0
        data_frame_counts, header_frame_counts = collections.Counter(), collections.Counter()
        for line in (directory/'calls.strace').open():
            match = re.match(r'([0-9]+)\s+(.*)', line)
            if not match:
                continue
            pid, call = match.groups()
            if '<unfinished ...>' in call:
                assert pid not in pending
                pending[pid] = call.split('<unfinished ...>')[0]
                continue
            resumed = re.match(r'<\.\.\. \w+ resumed>(.*)', call)
            if resumed:
                call = pending.pop(pid) + resumed[1]
            if 'UNIX-STREAM' in call:
                for boundary in ['before_measured_turns', 'after_turns']:
                    if boundary in call:
                        assert not any('pwrite64' in x and '-wal>' in x for x in pending.values())
                        boundaries.append(boundary)
                        phase = 'steady' if boundary == 'before_measured_turns' else 'after_turns'
            if not call.startswith('pwrite64(') or '-wal>' not in call:
                continue
            match = re.fullmatch(r'pwrite64\(\d+<([^>]+-wal)>, (.*), (\d+), (\d+)\) = (\d+)', call)
            assert match, call
            filename, quoted, size, offset, returned = match.groups()
            size, offset, returned = int(size), int(offset), int(returned)
            assert returned == size and size in [24, 32, 4096]
            if phase == 'steady':
                assert filename == before['path']+'-wal'
                write_sizes[size] += 1
            if size == 32:
                assert offset == 0
                header = ast.literal_eval('b'+quoted)
                page_sizes.add(struct.unpack('>I', header[8:12])[0])
                continue
            if size == 4096:
                assert (offset-56) % 4120 == 0
                if phase == 'steady':
                    data_frame_counts[offset-24] += 1
                continue
            assert (offset-32) % 4120 == 0
            header = ast.literal_eval('b'+quoted)
            assert len(header) == 24
            page, database_size = struct.unpack('>II', header[:8])
            if phase == 'steady':
                page_counts[page] += 1
                header_frame_counts[offset] += 1
                current_frames += 1
            if database_size:
                commits[phase] += 1
                if phase == 'steady':
                    transaction_frames.append(current_frames)
                    current_frames = 0
        assert not pending and current_frames == 0
        assert boundaries == ['before_measured_turns', 'after_turns']
        assert page_sizes == {4096} and data_frame_counts == header_frame_counts
        phases = read(directory/'phases.json')
        assert all(a['monotonic_ns'] <= b['monotonic_ns'] for a, b in zip(phases, phases[1:]))
        assert [p['phase'] for p in phases].count('before_measured_turns') == 1
        by_owner = collections.Counter()
        for page, count in page_counts.items():
            by_owner['|'.join(sorted(owners[page])) or '<unmapped>'] += count
        delta = {table: count-before['counts'].get(table, 0) for table, count in after['counts'].items() if count != before['counts'].get(table, 0)}
        old = stored_wal[name]
        frames = sum(page_counts.values())
        assert old['wal_write_commits'] == commits['steady']
        assert old['wal_frames'] == frames and old['wal_frame_bytes'] == frames*4120
        assert old['commits_by_phase'] == dict(commits)
        assert old['wal_frames_by_snapshot_page_owner'] == dict(by_owner)
        assert old['transaction_page_counts'] == transaction_frames and old['net_row_count_changes'] == delta
        report['wal'].append({'case_engine': name, 'write_commit_markers': commits['steady'],
            'measured_turns': 32, 'commit_markers_per_turn': commits['steady']/32,
            'frame_headers': frames, 'frame_body_writes': sum(data_frame_counts.values()),
            'page_size': 4096, 'frame_bytes': frames*4120, 'all_successful_write_sizes': dict(write_sizes),
            'wal_header_writes': write_sizes[32],
            'steady_wal_pwrite_bytes': sum(size*count for size, count in write_sizes.items()),
            'unmapped_owner_frames': by_owner['<unmapped>'],
            'mixed_owner_frames': sum(n for key, n in by_owner.items() if '|' in key),
            'all_producer_wal_summary_fields_equal': True, 'raw_sha256': sha(directory/'calls.strace')})

failed = ROOT/'wal-d-short-small-pi-initial-failed'
failure = read(failed/'sample.process.json')
assert not failure['accepted'] and not (failed/'sample.json').exists()
assert 'ERR_MODULE_NOT_FOUND' in (failed/'sample.log').read_text()
assert 'pi-observer-fixture.mjs' in (failed/'sample.log').read_text()
initial = read(ROOT/'wal-records-initial.json')
resumed = read(ROOT/'wal-records.json')
assert len(initial) == 2 and initial[0]['accepted'] and not initial[1]['accepted']
assert len(resumed) == 3 and all(p['accepted'] and not p['diagnostic_error'] for p in resumed)
report['failed_attempt_retained'] = {'directory': failed.name, 'accepted': False,
    'cause': 'ERR_MODULE_NOT_FOUND: relative pi-observer-fixture.mjs after adapter relocation',
    'raw_files': sorted(p.name for p in failed.iterdir())}
report['counts'] = {'profiles_and_controls_accepted': 8, 'wal_products_accepted': 4,
    'cross_engine_comparisons': 2, 'failed_wal_attempts_retained': 1}
report['semantic_scope'] = 'Existing oracle records, captured request equality, phase and persisted/reopened evidence checked. This script does not reimplement every native oracle or test power-loss durability.'
(ROOT/'diagnostic-audit.json').write_text(json.dumps(report, indent=2)+'\n')
print(json.dumps({'counts': report['counts'], 'cpu': report['cpu'], 'wal': report['wal']}, indent=2))
