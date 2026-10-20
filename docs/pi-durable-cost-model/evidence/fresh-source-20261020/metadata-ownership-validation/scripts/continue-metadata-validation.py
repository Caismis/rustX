#!/usr/bin/env python3
"""Wait for the existing compile, then run serial correctness and root Clippy."""
from pathlib import Path
import datetime, hashlib, json, os, subprocess, sys, time

root = Path('/workspace/pr485-study')
out = root / 'evidence/metadata-ownership-candidate-20261020'
manifest = out / 'validation-continuation.json'
assert not manifest.exists()

def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()

record = {'start_utc': now(), 'pid': os.getpid(), 'state': 'waiting-for-existing-compile',
          'expected_cargo_pid': 39389, 'expected_cargo_start_ticks': 1641904,
          'source_commit': 'da49cecc037d29f90aa48047c1f2441a4531c83c',
          'script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
          'runs': [], 'no_performance': True}

def save():
    manifest.write_text(json.dumps(record, indent=2) + '\n')

save()
print(json.dumps({'state': record['state'], 'pid': record['pid']}), flush=True)
start = time.monotonic()
while not (out / 'build-final-validated.json').exists():
    assert time.monotonic() - start < 5400, 'existing build completion did not arrive'
    time.sleep(.5)
build = json.loads((out / 'build-final-validated.json').read_text())
if build['exit'] != 0 or build.get('guard_stop'):
    record.update(state='existing-compile-failed-no-tests-launched', end_utc=now())
    save()
    sys.exit(1)
for label, script in [('tests', 'test-metadata-ownership.py'), ('clippy', 'lint-metadata-ownership.py')]:
    record['state'] = 'running-' + label
    argv = [sys.executable, str(root / script)]
    run = {'label': label, 'argv': argv, 'start_utc': now(),
           'script_sha256': hashlib.sha256((root / script).read_bytes()).hexdigest()}
    record['runs'].append(run)
    save()
    with (out / ('continuation-' + label + '.stdout')).open('w') as stdout, (out / ('continuation-' + label + '.stderr')).open('w') as stderr:
        result = subprocess.run(argv, cwd=root, stdout=stdout, stderr=stderr)
    run.update(exit=result.returncode, end_utc=now())
    save()
    if result.returncode != 0:
        record.update(state='validation-driver-failed-output-retained', end_utc=now())
        save()
        sys.exit(result.returncode)
record.update(state='original-source-tests-and-lint-finished-review-results', end_utc=now())
save()
print(json.dumps({'state': record['state']}), flush=True)
