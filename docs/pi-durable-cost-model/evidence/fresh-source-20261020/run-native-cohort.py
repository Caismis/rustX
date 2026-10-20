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
import traceback
from ownership_adapter import Guard, accepted, cohort_arguments, comparison_runner, expected_plan, parse_ownership_args, validate_candidate

root = Path('/workspace/pr485-study')
candidate_build = os.environ.get('RUSTX_BENCH_CANDIDATE_BUILD')
ownership_options = parse_ownership_args(sys.argv[1:], root) if candidate_build else None
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
builds = [(label, root / 'evidence' / ('build-' + label)) for label in ('baseline', 'geometry', 'scoped')]
if candidate_build:
    builds.append(('ownership', Path(candidate_build).resolve()))
for label, build in builds:
    identity = json.loads((build / 'identity.json').read_text())
    preserved = json.loads((build / 'preserved-artifacts.json').read_text())
    assert identity['verified'] is True
    assert sha(Path(identity['binary'])) == identity['sha256']
    if label == 'ownership':
        validate_candidate(identity, preserved, build_identities['baseline'])
        assert str(Path(identity['binary']).resolve()) not in identities
    build_identities[label] = identity
    identities[str(Path(identity['binary']).resolve())] = (label, preserved)
if ownership_options is not None:
    assert identities[str(ownership_options.before)][0] == 'baseline' and identities[str(ownership_options.after)][0] == 'ownership', 'ownership mode requires exact baseline/candidate pair'

original_run = controller.run
original_popen, original_subprocess_run = subprocess.Popen, subprocess.run
guard = None
plan = expected_plan(ownership_options) if ownership_options is not None else []
attempts = []
output_arg = str(ownership_options.output) if ownership_options is not None else sys.argv[sys.argv.index('--output') + 1]
Path(output_arg).parent.mkdir(parents=True, exist_ok=True)
restoration_log = Path(output_arg).with_name(Path(output_arg).name + '-helper-restoration.jsonl')
interval_log = Path(output_arg).with_name(Path(output_arg).name + '-product-intervals.jsonl')
cohort_path = Path(output_arg).with_name(Path(output_arg).name + '-cohort-manifest.json')
assert not restoration_log.exists()
assert not interval_log.exists()
assert not cohort_path.exists()
protected = [runtime / name for name in ('compare-revisions.py', 'controller.py', 'observer_oracle.py')]
if candidate_build:
    protected.extend([Path(__file__).resolve(), Path(__file__).with_name('ownership_adapter.py').resolve()])
    frozen_driver = Path(__file__).with_name('ownership-driver-compare-revisions.py')
    assert sha(runtime / 'compare-revisions.py') == sha(frozen_driver), 'runtime driver differs from tested frozen workload'
    protected.append(frozen_driver)
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
    if guard is not None:
        guard.check()
    label, preserved = identities[str(Path(argv[0]).resolve())]
    if guard is not None:
        actual = {'label': label, 'case': case, 'observer_mode': env.get('RUSTX_BENCH_OBSERVER_MODE'), 'stem': str(stem.resolve())}
        if len(attempts) >= len(plan) or actual != plan[len(attempts)]:
            raise RuntimeError('unexpected product identity/case/order/output; no product started')
        attempts.append(actual)
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
    if guard is not None:
        guard.product(True)
    entered = time.monotonic_ns()
    result = original_run(argv, stem, case, env, project)
    ended = time.monotonic_ns()
    if guard is not None:
        guard.product(False)
    with interval_log.open('a') as f:
        f.write(json.dumps({'label': label, 'stem': str(stem), 'case': case,
            'start_utc': started_utc,
            'end_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'controller_entry_monotonic_ns': entered,
            'controller_return_monotonic_ns': ended,
            'accepted': result['accepted']}) + '\n')
    if guard is not None:
        guard.check()
        accepted(result)
    return result

controller.run = guarded_run
sys.argv[0] = str(runtime / 'compare-revisions.py')
os.chdir(repo)
try:
    if candidate_build:
        guard = Guard(root, Path(output_arg + '-watchdog'), 'cohort', Path(output_arg)).start()
        def owned_popen(*args, **kwargs):
            guard.check()
            kwargs['start_new_session'] = True
            process = original_popen(*args, **kwargs)
            guard.register(process)
            return process
        subprocess.Popen = owned_popen
        subprocess.run = comparison_runner(original_subprocess_run, Path(output_arg + '-watchdog'))
        cohort['ownership_adapter'] = {'candidate_build': str(Path(candidate_build).resolve()),
            'effective_entry_reserve_bytes': 16 * 2**30, 'first_failure_stops': True,
            'pair_order': ['AB', 'BA', 'AB'], 'order_imbalance': 'baseline first twice; candidate first once; exploratory n=3, no acceptance/significance claim',
            'watchdog': output_arg + '-watchdog', 'cohort_cap_seconds': 1800,
            'expected_product_count': 18,
            'expected_plan': plan,
            'canonical_blocks': cohort_arguments(ownership_options, runtime / 'compare-revisions.py'),
            'incomplete_policy': 'retain incomplete cohort; no replacement or automatic retry'}
        for block_argv in cohort_arguments(ownership_options, runtime / 'compare-revisions.py'):
            guard.check()
            sys.argv = block_argv
            try:
                runpy.run_path(sys.argv[0], run_name='__main__')
            except SystemExit as result:
                if result.code not in (0, None):
                    raise
    else:
        runpy.run_path(sys.argv[0], run_name='__main__')
except Exception:
    cohort['adapter_error'] = traceback.format_exc()
    raise
finally:
    subprocess.Popen, subprocess.run = original_popen, original_subprocess_run
    if candidate_build:
        controller.run = original_run
    guard_error = None
    if guard is not None:
        try:
            guard.close()
        except RuntimeError as error:
            cohort['watchdog_failure'] = str(error)
            guard_error = error
    assert {str(p): sha(p) for p in protected} == hashes, 'published benchmark sources changed'
    cohort['end_utc'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    manifest = Path(output_arg) / 'manifest.json'
    if manifest.exists():
        cohort['published_manifest'] = json.loads(manifest.read_text())
    if candidate_build:
        cohort['published_manifests'] = {name: json.loads(path.read_text())
            for name in ('live-main', 'late-control')
            if (path := Path(output_arg) / name / 'manifest.json').exists()}
    if interval_log.exists():
        cohort['products'] = [json.loads(line) for line in interval_log.read_text().splitlines()]
    if candidate_build:
        cohort['attempted_plan'] = attempts
        completed = [{key: product[key] for key in ('label', 'case', 'stem')} for product in cohort.get('products', [])]
        expected_completed = [{key: item[key] for key in ('label', 'case', 'stem')} for item in plan]
        cohort['complete_18_products'] = attempts == plan and completed == expected_completed and all(product['accepted'] for product in cohort.get('products', [])) and not guard_error and 'adapter_error' not in cohort
    cohort_path.write_text(json.dumps(cohort, indent=2) + '\n')
    if guard_error is not None:
        raise guard_error
    if candidate_build and not cohort['complete_18_products'] and 'adapter_error' not in cohort:
        raise RuntimeError('incomplete or mismatched final product plan; retained without replacement')
