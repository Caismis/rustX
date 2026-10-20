#!/usr/bin/env python3
"""Run unchanged compare-revisions.py, restoring owned helpers before its timer."""
import hashlib
import datetime
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import time

root = Path('/workspace/pr485-study')
repo = root / 'repo'
runtime = repo / 'benchmarks/runtime'
os.environ['TMPDIR'] = str(root / 'tmp')
assert not os.environ.get('LD_PRELOAD'), 'instrumentation preload is not allowed'
assert os.sysconf('SC_CLK_TCK') == 100
sys.path.insert(0, str(runtime))
import controller

def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

identities = {}
build_identities = {}
for label in ('baseline', 'geometry', 'scoped'):
    build = root / 'evidence' / ('build-' + label)
    identity = json.loads((build / 'identity.json').read_text())
    preserved = json.loads((build / 'preserved-artifacts.json').read_text())
    assert identity['verified'] is True
    assert sha(Path(identity['binary'])) == identity['sha256']
    build_identities[label] = identity
    identities[str(Path(identity['binary']).resolve())] = (label, preserved)

original_run = controller.run
output_arg = sys.argv[sys.argv.index('--output') + 1]
Path(output_arg).parent.mkdir(parents=True, exist_ok=True)
restoration_log = Path(output_arg).with_name(Path(output_arg).name + '-helper-restoration.jsonl')
interval_log = Path(output_arg).with_name(Path(output_arg).name + '-product-intervals.jsonl')
cohort_path = Path(output_arg).with_name(Path(output_arg).name + '-cohort-manifest.json')
assert not restoration_log.exists()
assert not interval_log.exists()
assert not cohort_path.exists()
protected = [runtime / name for name in ('compare-revisions.py', 'controller.py', 'observer_oracle.py')]
hashes = {str(p): sha(p) for p in protected}
node_version = subprocess.check_output(['node', '--version'], text=True).strip()
assert node_version == 'v24.19.0'
cohort = {'start_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'argv': list(sys.argv), 'published_source_sha256': hashes, 'build_identities': build_identities,
    'environment': {'node': node_version, 'python': sys.version, 'TMPDIR': os.environ['TMPDIR'],
        'TZ': os.environ.get('TZ'), 'LD_PRELOAD': os.environ.get('LD_PRELOAD'),
        'clock_ticks': os.sysconf('SC_CLK_TCK'),
        'cpu_max': Path('/sys/fs/cgroup/cpu.max').read_text().strip(),
        'memory_max': Path('/sys/fs/cgroup/memory.max').read_text().strip()},
    'measurement_definition': 'Unchanged published compare-revisions.py/controller.run; helper restore and all extra manifest I/O occur outside controller.run.',
    'helper_restoration_log': str(restoration_log), 'product_interval_log': str(interval_log)}
cohort_path.write_text(json.dumps(cohort, indent=2) + '\n')

def guarded_run(argv, stem, case, env, project):
    label, preserved = identities[str(Path(argv[0]).resolve())]
    stat = os.statvfs(root)
    assert stat.f_bavail * stat.f_frsize >= 16 * 2**30, 'less than 16 GiB usable reserve'
    active = []
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            comm = (entry / 'comm').read_text().strip()
            if comm in ('cargo', 'rustc', 'rust-lld', 'cc1', 'cc1plus', 'xz', 'gzip', 'perf', 'strace',
                        'conformance', 'inspector-host', 'fake-provider'):
                active.append({'pid': int(entry.name), 'comm': comm})
        except (OSError, ProcessLookupError):
            pass
    assert not active, active
    before = time.monotonic_ns()
    restored = []
    for item in preserved['artifacts']:
        if item['kind'] not in (['bin'], ['test'], ['shared-library']):
            continue
        source, target = Path(item['preserved']), Path(item['target'])
        assert sha(source) == item['sha256']
        changed = not target.exists() or sha(target) != item['sha256']
        if changed:
            target.parent.mkdir(parents=True, exist_ok=True)
            staging = target.with_name(target.name + '.restore')
            shutil.copy2(source, staging)
            staging.replace(target)
        assert sha(target) == item['sha256']
        restored.append({'target': str(target), 'sha256': item['sha256'], 'copied': changed})
    assert sha(Path(argv[0])) == json.loads((Path(argv[0]).parent / 'identity.json').read_text())['sha256']
    with restoration_log.open('a') as f:
        f.write(json.dumps({'label': label, 'stem': str(stem), 'case': case,
            'outside_controller_run': True, 'before_monotonic_ns': before,
            'after_monotonic_ns': time.monotonic_ns(), 'TMPDIR': os.environ['TMPDIR'],
            'idle_process_check_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'forbidden_or_timed_processes': active, 'artifacts': restored}) + '\n')
    started_utc = datetime.datetime.now(datetime.timezone.utc).isoformat()
    entered = time.monotonic_ns()
    result = original_run(argv, stem, case, env, project)
    ended = time.monotonic_ns()
    with interval_log.open('a') as f:
        f.write(json.dumps({'label': label, 'stem': str(stem), 'case': case,
            'start_utc': started_utc,
            'end_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'controller_entry_monotonic_ns': entered,
            'controller_return_monotonic_ns': ended,
            'accepted': result['accepted']}) + '\n')
    return result

controller.run = guarded_run
sys.argv[0] = str(runtime / 'compare-revisions.py')
os.chdir(repo)
try:
    runpy.run_path(sys.argv[0], run_name='__main__')
finally:
    assert {str(p): sha(p) for p in protected} == hashes, 'published benchmark sources changed'
    cohort['end_utc'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    manifest = Path(output_arg) / 'manifest.json'
    if manifest.exists():
        cohort['published_manifest'] = json.loads(manifest.read_text())
    if interval_log.exists():
        cohort['products'] = [json.loads(line) for line in interval_log.read_text().splitlines()]
    cohort_path.write_text(json.dumps(cohort, indent=2) + '\n')
