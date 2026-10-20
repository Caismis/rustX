#!/usr/bin/env python3
"""Wait for the existing geometry cohort; launch scoped once, sequentially."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

root = Path('/workspace/pr485-study')
driver = root / 'run-browser-cohort.py'
geometry = root / 'evidence/products/browser-baseline-vs-geometry-v2/cohort-manifest.json'
output = root / 'evidence/products/browser-baseline-vs-scoped'
record = root / 'evidence/browser-sequential-continuation.json'


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


matches = []
for path in Path('/proc').iterdir():
    if not path.name.isdigit():
        continue
    try:
        args = (path / 'cmdline').read_bytes().split(b'\0')
        if len(args) > 3 and args[1] == str(driver).encode() and args[2:4] == [b'--candidate', b'geometry']:
            matches.append(int(path.name))
    except OSError:
        pass
assert len(matches) == 1, matches
pid = matches[0]
started = (Path('/proc') / str(pid) / 'stat').read_text().rsplit(')', 1)[1].split()[19]
state = {'start_utc': now(), 'wait_existing_geometry_pid': pid, 'wait_pid_start_ticks': started,
         'driver_sha256': hashlib.file_digest(driver.open('rb'), 'sha256').hexdigest(),
         'scoped_output': str(output), 'relaunch_existing_runner': False}
record.write_text(json.dumps(state, indent=2) + '\n')
while True:
    try:
        current = (Path('/proc') / str(pid) / 'stat').read_text().rsplit(')', 1)[1].split()
    except FileNotFoundError:
        break
    if current[19] != started or current[0] == 'Z':
        break
    time.sleep(5)
manifest = json.loads(geometry.read_text())
assert manifest.get('completed_products') == 18 and not manifest.get('stopped_on_failure'), 'Inspect geometry failure before continuing'
assert all(run.get('accepted_by_producer') for run in manifest['runs'])
assert not output.exists(), 'Never overwrite or relaunch an existing scoped cohort'
assert hashlib.file_digest(driver.open('rb'), 'sha256').hexdigest() == state['driver_sha256']
argv = ['python3', str(driver), '--candidate', 'scoped', '--output', str(output)]
state.update(geometry_end_utc=manifest['end_utc'], scoped_start_utc=now(), scoped_argv=argv)
record.write_text(json.dumps(state, indent=2) + '\n')
with (root / 'evidence/browser-scoped.stdout').open('w') as stdout, (root / 'evidence/browser-scoped.stderr').open('w') as stderr:
    result = subprocess.run(argv, cwd=root, stdout=stdout, stderr=stderr)
state.update(scoped_end_utc=now(), scoped_exit=result.returncode)
record.write_text(json.dumps(state, indent=2) + '\n')
raise SystemExit(result.returncode)
