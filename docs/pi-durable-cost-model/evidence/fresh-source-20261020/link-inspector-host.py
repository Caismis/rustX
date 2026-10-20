#!/usr/bin/env python3
"""Record untimed original host-builder execution and required system libraries."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

root = Path('/workspace/pr485-study')
label = sys.argv[1]
build = root / 'evidence' / ('build-' + label)
fixture = root / 'inspector-fixture'
identity = json.loads((build / 'identity.json').read_text())
assert identity['verified'] is True
assert subprocess.check_output(['git', '-C', str(root / 'repo'), 'rev-parse', 'HEAD'], text=True).strip() == identity['head']

def sha(path):
    with Path(path).open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

expected = json.loads((root / 'evidence/handoff-fixture-identity.json').read_text())
def hashes():
    return {p.name: sha(p) for p in fixture.iterdir() if p.is_file()}

assert hashes() == expected
env = os.environ | {'PATH': '/workspace/.cargo/bin:' + os.environ['PATH'],
    'CARGO_HOME': '/workspace/.cargo', 'RUSTUP_HOME': '/workspace/.rustup',
    'RUSTUP_TOOLCHAIN': '1.98.1', 'TMPDIR': str(root / 'tmp')}
cmd = [sys.executable, str(fixture / 'build-host.py'), '--build', str(build), '--output', str(build / 'host')]
record = {'start_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'argv': cmd,
    'fixture_sha256': expected, 'environment': {k: env[k] for k in
    ['PATH', 'CARGO_HOME', 'RUSTUP_HOME', 'RUSTUP_TOOLCHAIN', 'TMPDIR']}}
path = root / 'evidence' / ('build-' + label + '-host-orchestration.json')
assert not path.exists()
path.write_text(json.dumps(record, indent=2) + '\n')
result = subprocess.run(cmd, cwd=root / 'repo', env=env)
record.update(exit=result.returncode, end_utc=datetime.datetime.now(datetime.timezone.utc).isoformat())
path.write_text(json.dumps(record, indent=2) + '\n')
assert hashes() == expected
result.check_returncode()
host = json.loads((build / 'host/identity.json').read_text())
assert sha(build / 'host/inspector-host') == host['binary_sha256']
assert host['extern']['rustx']['sha256'] == identity['library_sha256']
linkage = json.loads((build / 'runtime-linkage.json').read_text())
binary = build / 'host/inspector-host'
ldd = subprocess.run(['ldd', str(binary)], capture_output=True, text=True, check=True)
linkage[str(binary)] = {'exit': ldd.returncode, 'stdout': ldd.stdout, 'stderr': ldd.stderr}
pool = root / 'evidence/common-system-libraries'
pool.mkdir(exist_ok=True)
libraries = []
for item in linkage.values():
    for filename in re.findall(r'(?:=>\s+|^\s*)(/\S+)\s+\(', item['stdout'], re.M):
        source = Path(filename).resolve()
        digest = sha(source)
        dest = pool / (digest + '-' + source.name)
        if not dest.exists():
            shutil.copy2(source, dest)
        assert sha(dest) == digest
        entry = {'path': filename, 'resolved': str(source), 'preserved': str(dest),
                 'sha256': digest, 'bytes': source.stat().st_size}
        if entry not in libraries:
            libraries.append(entry)
(build / 'system-linkage-libraries.json').write_text(json.dumps({'libraries': libraries, 'executables': linkage}, indent=2) + '\n')
print(json.dumps({'label': label, 'host_sha256': host['binary_sha256'],
                  'library_sha256': identity['library_sha256'], 'system_libraries': len(libraries)}))
