#!/usr/bin/env python3
"""Independent F raw WAL audit, adapted from the independently reviewed D parser."""
import ast, collections, hashlib, json, re, struct, subprocess
from pathlib import Path
ROOT = Path(__file__).resolve().parent
REPO = Path('/home/capi/.codex/worktrees/pr485-pi-performance/rustX')
PI = Path('/home/capi/Documents/codes/pi-rustx-benchmark')
SHA = '7ca19d99e1a241e5e33627819fe837a81788d26609072d8c41a1a0ab805ef109'
PI_REV = '90d863f7c1183b965f099855715d6e88c7b8a52c'
def read(path):
    return json.loads(path.read_text())

def sha(path):
    with path.open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()

def git(path, *args):
    return subprocess.check_output(['git', '-C', str(path), *args], text=True).strip()

build=read(ROOT/'candidate-f-build/identity.json')
assert build['head']=='2800be25a650305d5daf9f543ffbf7d8f62ea79b'
assert build['production_tree']==git(REPO,'rev-parse',build['head']+':src')=='a86302ed48b415de91cac782bd156dc935b8e517'
assert build['verified'] and build['build_exit']==0 and build['sha256']==sha(Path(build['binary']))==SHA
for filename, expected in build['harness'].items(): assert sha(REPO/filename)==expected
assert git(PI,'rev-parse','HEAD')==PI_REV and not git(PI,'diff','--name-only','HEAD')
identity=read(ROOT/'wal-identity-f.json')
assert identity['rust_sha256']==SHA and identity['rust_source']==build['head'] and identity['pi_revision']==PI_REV
report={'verified':True,'identity':identity,'per_product_before_identity_verified':True,'products':[],'comparisons':[],'wal':[]}
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

stored_wal = read(ROOT/'wal-summary-f.json')
for case in ['short-small', 'short-large']:
    comparison = read(ROOT/f'wal-f-{case}-comparison.json')
    assert len(comparison['checked']) == (40 if case == 'short-small' else 96)
    assert all(p['coreSemanticEqual'] and not p['fullSemanticRequestEqual'] for p in comparison['checked'])
    report['comparisons'].append({'case': case, 'checked': len(comparison['checked']), 'core_all_equal': True, 'full_request_all_equal': False})
    for engine in ['rust', 'pi']:
        name = f'{case}-{engine}'
        directory = ROOT/('wal-f-'+name)
        frozen = read(directory/'identity-before.json')
        assert frozen['binary_sha256'] == SHA and frozen['pi_revision'] == PI_REV
        assert frozen['build'] == build
        assert frozen['driver_sha256'] == sha(ROOT/'pi-http-wal-f.mjs')
        assert frozen['runner_sha256'] == sha(ROOT/'run-wal-verified.py')
        assert frozen['argv'] == read(directory/'sample.process.json')['argv']
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

records=read(ROOT/'wal-records-f.json')
assert len(records)==4 and all(p['accepted'] and not p['diagnostic_error'] for p in records)
assert all(a['end_ns']<=b['start_ns'] for a,b in zip(records,records[1:]))
prior=read(ROOT/'diagnostic-audit.json')
report['D_comparison']=[]
for row in report['wal']:
 old=next(x for x in prior['wal'] if x['case_engine']==row['case_engine'])
 report['D_comparison'].append({'case_engine':row['case_engine'],'D_frames':old['frame_headers'],'F_frames':row['frame_headers'],'frame_delta':row['frame_headers']-old['frame_headers'],'frame_ratio':row['frame_headers']/old['frame_headers'],'D_commit_markers':old['write_commit_markers'],'F_commit_markers':row['write_commit_markers']})
report['scope']='Frame headers/body writes paired; exact producer summary equality and original source/build/driver identities checked. Native oracle records checked, not reimplemented. Endpoint dbstat ownership cannot exclude intermediate page reuse; WAL bytes are not device traffic or fsync counts.'
(ROOT/'wal-f-audit.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({'verified':True,'accepted_products':len(report['products']),'wal':report['wal'],'D_comparison':report['D_comparison']},indent=2))
