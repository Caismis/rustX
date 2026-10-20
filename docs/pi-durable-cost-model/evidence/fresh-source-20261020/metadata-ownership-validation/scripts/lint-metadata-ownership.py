#!/usr/bin/env python3
"""Check the exact root library-test crate against already compiled dependencies."""
from pathlib import Path
import datetime, hashlib, json, os, shlex, signal, subprocess, time

root = Path('/workspace/pr485-study')
repo = root / 'metadata-ownership-candidate'
out = root / 'evidence/metadata-ownership-candidate-20261020'
build_path = out / 'build-final-validated.json' if (out / 'build-final-validated.json').exists() else out / 'build-orchestration.json'
build = json.loads(build_path.read_text())
tests = json.loads((out / 'test-orchestration.json').read_text())
assert build['exit'] == 0 and tests['state'] in ['tests-passed', 'tests-failed']
assert not (out / 'lint-orchestration.json').exists()
for item in Path('/proc').iterdir():
    if item.name.isdigit():
        try:
            state = (item / 'stat').read_text().rsplit(')', 1)[1].split()[0]
            assert state == 'Z' or (item / 'comm').read_text().strip() not in ['cargo', 'rustc', 'clippy-driver']
        except OSError:
            pass

def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()

def free():
    value = os.statvfs(root)
    return value.f_bavail * value.f_frsize

def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

lines = [line.strip().removeprefix('Running `').removesuffix('`')
         for line in (out / 'cargo.stderr').read_text().splitlines()
         if 'Running `' in line and '--crate-name rustx ' in line]
assert len(lines) == 1
parts = shlex.split(lines[0])
index = next(i for i, value in enumerate(parts) if value.endswith('/rustc'))
env = os.environ | build['environment_overrides'] | dict(value.split('=', 1) for value in parts[:index])
for key in ['RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_ENCODED_RUSTFLAGS', 'CLIPPY_ARGS']:
    env.pop(key, None)
assert not env.get('LD_PRELOAD')
driver = str(Path(parts[index]).with_name('clippy-driver'))
args = parts[index + 1:]
args[args.index('--emit=dep-info,link')] = '--emit=metadata'
directory = out / 'clippy-root-artifacts'
directory.mkdir(exist_ok=False)
args[args.index('--out-dir') + 1] = str(directory)
argv = [driver, *args, '-D', 'warnings']
assert free() > 6 * 2**30
record = {'start_utc': now(), 'scope': 'Direct Clippy check of exact default-feature root library-test crate, reusing all already compiled externs; not CI all-targets/all-features or FULL',
          'argv': argv, 'cwd': str(repo), 'source_sha256': {name: sha(repo / name) for name in build['source_sha256']},
          'root_environment_from_cargo_verbose': True, 'guard_floor_bytes': 5 * 2**30,
          'soft_stop_available_bytes': 6 * 2**30, 'before_available_bytes': free(),
          'outer_script_sha256': sha(__file__)}
manifest = out / 'lint-orchestration.json'
manifest.write_text(json.dumps(record, indent=2) + '\n')
with (out / 'clippy.stdout.jsonl').open('w') as stdout, (out / 'clippy.stderr').open('w') as stderr:
    process = subprocess.Popen(argv, cwd=repo, env=env, stdout=stdout, stderr=stderr, start_new_session=True)
    start = time.monotonic()
    minimum = free()
    stop = None
    while process.poll() is None:
        available = free()
        minimum = min(minimum, available)
        if stop is None and (available <= 6 * 2**30 or time.monotonic() - start > 600):
            record['guard_stop'] = {'utc': now(), 'available_bytes': available}
            os.killpg(process.pid, signal.SIGTERM)
            stop = time.monotonic()
        elif stop is not None and time.monotonic() - stop > 10:
            os.killpg(process.pid, signal.SIGKILL)
            stop = float('inf')
        time.sleep(.5)
record.update(exit=process.returncode, end_utc=now(), minimum_observed_available_bytes=minimum, after_available_bytes=free())
manifest.write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps({'exit': record['exit'], 'minimum_available_bytes': minimum}), flush=True)
