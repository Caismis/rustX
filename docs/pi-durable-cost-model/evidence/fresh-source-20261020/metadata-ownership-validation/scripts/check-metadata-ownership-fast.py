#!/usr/bin/env python3
"""Root-only O0 correctness harness against preserved O3 dependency rlibs."""
from pathlib import Path
import datetime, hashlib, json, os, shlex, signal, subprocess, sys, time

root = Path('/workspace/pr485-study')
repo = root / 'metadata-ownership-candidate'
original = root / 'evidence/metadata-ownership-candidate-20261020'
label = sys.argv[1]
assert label in ['fixture-admission', 'trace-expectation']
out = root / 'evidence/metadata-ownership-correctness-20261020' / label
out.mkdir(parents=True, exist_ok=False)

def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()

def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def point():
    value = os.statvfs(root)
    tmp = os.statvfs('/tmp')
    return {'utc': now(), 'available_bytes': value.f_bavail * value.f_frsize,
            'tmp_available_bytes': tmp.f_bavail * tmp.f_frsize,
            'memory_current': int(Path('/sys/fs/cgroup/memory.current').read_text())}

for item in Path('/proc').iterdir():
    if item.name.isdigit():
        try:
            state = (item / 'stat').read_text().rsplit(')', 1)[1].split()[0]
            assert state == 'Z' or (item / 'comm').read_text().strip() not in ['cargo', 'rustc', 'clippy-driver', 'conformance', 'inspector-host']
        except OSError:
            pass
build = json.loads((original / 'build-final-validated.json').read_text())
assert build['exit'] == 0
assert sha(repo / 'src/runtime_client/projection.rs') == build['source_sha256']['src/runtime_client/projection.rs']
old_binary = root / 'target/release/deps/rustx-7a147e7c97deabb9'
assert sha(old_binary) == json.loads((original / 'test-orchestration.json').read_text())['sha256']
lines = [line.strip().removeprefix('Running `').removesuffix('`') for line in (original / 'cargo.stderr').read_text().splitlines() if 'Running `' in line and '--crate-name rustx ' in line]
assert len(lines) == 1
parts = shlex.split(lines[0])
index = next(i for i, value in enumerate(parts) if value.endswith('/rustc'))
env = os.environ | build['environment_overrides'] | dict(value.split('=', 1) for value in parts[:index])
for key in ['CARGO_MAKEFLAGS', 'MAKEFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_ENCODED_RUSTFLAGS', 'CLIPPY_ARGS']:
    env.pop(key, None)
env.update(CARGO_INCREMENTAL='1', RUSTFLAGS='', RUST_BACKTRACE='1')
assert not env.get('LD_PRELOAD')
args = parts[index + 1:]
args[args.index('opt-level=3')] = 'opt-level=0'
args.remove('-Copt-level=3')
artifacts = out / 'root-artifacts'
artifacts.mkdir()
args[args.index('--out-dir') + 1] = str(artifacts)
incremental = Path('/tmp/pr485-metadata-correctness-incremental')
incremental.mkdir(exist_ok=True)
args += ['-Clto=off', '-Cdebug-assertions=off', '-Coverflow-checks=off', '-Ccodegen-units=256', '-Cincremental=' + str(incremental)]
argv = [parts[index], *args]
affinity = sorted(os.sched_getaffinity(0))[:2]
record = {'label': label, 'start_utc': now(), 'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
          'src_tree': subprocess.check_output(['git', 'rev-parse', 'HEAD:src'], cwd=repo, text=True).strip(),
          'source_sha256': {name: sha(repo / name) for name in build['source_sha256']},
          'production_file_identical_to_original_O3_build': True,
          'scope': 'Correctness only; root O0/debuginfo0/lto-off, release assertion/overflow behavior, cached O3 dependency rlibs. Never use for performance.',
          'argv': argv, 'environment_overrides': {'CARGO_INCREMENTAL': '1', 'RUSTFLAGS': '', 'RUST_BACKTRACE': '1'},
          'base_root_environment': str(original / 'build-orchestration.json'), 'rustc_environment_from_cargo_verbose': True,
          'affinity': affinity, 'guard_floor_bytes': 5 * 2**30, 'soft_stop_available_bytes': 6 * 2**30,
          'original_optimized_test_binary': str(old_binary), 'original_optimized_test_binary_sha256': sha(old_binary),
          'incremental_directory': str(incremental), 'before': point(), 'runs': [], 'outer_script_sha256': sha(__file__)}
manifest = out / 'validation.json'

def save():
    manifest.write_text(json.dumps(record, indent=2) + '\n')

def run(label, command, bound=1800, compiler=False):
    assert point()['available_bytes'] > 6 * 2**30
    entry = {'label': label, 'argv': command, 'start_utc': now(), 'before': point()}
    record['runs'].append(entry)
    record['state'] = 'running-' + label
    save()
    print(json.dumps({'label': label, 'state': record['state']}), flush=True)
    with (out / (label + '.stdout')).open('w') as stdout, (out / (label + '.stderr')).open('w') as stderr, (out / (label + '.resources.jsonl')).open('w') as resources:
        process = subprocess.Popen(command, cwd=repo, env=env, stdout=stdout, stderr=stderr, start_new_session=True,
                                   preexec_fn=(lambda: os.sched_setaffinity(0, set(affinity))) if compiler else None)
        entry['pid'] = process.pid
        save()
        start = time.monotonic()
        minimum = point()['available_bytes']
        stop = None
        while process.poll() is None:
            value = point()
            minimum = min(minimum, value['available_bytes'])
            resources.write(json.dumps(value) + '\n')
            resources.flush()
            if stop is None and (value['available_bytes'] <= 6 * 2**30 or value['tmp_available_bytes'] < 2 * 2**30 or time.monotonic() - start > bound):
                entry['guard_stop'] = value
                os.killpg(process.pid, signal.SIGTERM)
                stop = time.monotonic()
            elif stop is not None and time.monotonic() - stop > 10:
                os.killpg(process.pid, signal.SIGKILL)
                stop = float('inf')
            time.sleep(.5)
    entry.update(exit=process.returncode, end_utc=now(), minimum_observed_available_bytes=minimum, after=point())
    save()
    print(json.dumps({'label': label, 'exit': entry['exit']}), flush=True)
    return entry['exit']

save()
if run('build', argv, compiler=True) != 0:
    record.update(state='build-failed', end_utc=now())
    save()
    sys.exit(1)
binary = artifacts / 'rustx-7a147e7c97deabb9'
record.update(binary=str(binary), binary_sha256=sha(binary))
listing = subprocess.check_output([str(binary), '--list'], cwd=repo, env=env, text=True)
(out / 'libtests-list.txt').write_text(listing)
names = [line.removesuffix(': test') for line in listing.splitlines() if line.endswith(': test')]
new = 'runtime_client::projection::tests::metadata_ownership_tests::'
specs = [('ownership', new, []), ('projection', 'runtime_client::projection::tests::', [new, '::scale::']),
         ('checkpoint', 'runtime_client::checkpoint::', ['::scale::']), ('settlement', 'runtime_client::host::tests::settlement_tests::', [])]
for label, filter, skips in specs:
    selected = [name for name in names if filter in name and not any(skip in name for skip in skips)]
    assert selected and (label != 'ownership' or len(selected) == 8)
    command = [str(binary), filter, '--test-threads=1', '--nocapture']
    for skip in skips:
        command += ['--skip', skip]
    result = run(label, command, bound=900)
    record['runs'][-1]['discovered_matching_tests'] = len(selected)
    if result:
        record.setdefault('failed_runs', []).append(label)
    save()
if not record.get('failed_runs'):
    lint_args = [arg for arg in args if not arg.startswith('-Cincremental=')]
    lint_args[lint_args.index('--emit=dep-info,link')] = '--emit=metadata'
    lint_artifacts = out / 'clippy-artifacts'
    lint_artifacts.mkdir()
    lint_args[lint_args.index('--out-dir') + 1] = str(lint_artifacts)
    if run('clippy', [str(Path(parts[index]).with_name('clippy-driver')), *lint_args, '-D', 'warnings'], bound=900, compiler=True):
        record.setdefault('failed_runs', []).append('clippy')
record.update(state='checks-failed' if record.get('failed_runs') else 'checks-passed', end_utc=now(), after=point())
for name, digest in record['source_sha256'].items():
    assert sha(repo / name) == digest
assert sha(old_binary) == record['original_optimized_test_binary_sha256']
save()
print(json.dumps({'state': record['state'], 'failed_runs': record.get('failed_runs', [])}), flush=True)
