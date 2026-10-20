#!/usr/bin/env python3
"""Retain native artifacts and compiler input evidence before shared-target clean."""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys

root = Path('/workspace/pr485-study')
repo = root / 'repo'
target = root / 'target'
label = sys.argv[1]
build = root / 'evidence' / ('build-' + label)
assert json.loads((build / 'identity.json').read_text())['verified'] is True

def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

records = [json.loads(line) for line in (build / 'build.stdout').read_text().splitlines()
           if line.startswith('{')]
artifacts = [r for r in records if r.get('reason') == 'compiler-artifact']
owned = [r for r in artifacts if Path(r['target']['src_path']).is_relative_to(repo)]
for r in owned:
    assert r['profile']['opt_level'] == '3' and r['profile']['debuginfo'] == 0
    assert r['profile']['debug_assertions'] is False and r['profile']['overflow_checks'] is False
    assert r['features'] == [], r['features']
directory = build / 'artifacts'
directory.mkdir(exist_ok=False)
manifest = {'label': label, 'source_head': json.loads((build / 'identity.json').read_text())['head'],
            'artifacts': [], 'input_depfiles': [], 'compiler_inputs': [], 'external_graph': []}
for row in owned:
    for filename in row['filenames']:
        source = Path(filename)
        if not source.is_file():
            continue
        relative = source.relative_to(target)
        destination = directory / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        manifest['artifacts'].append({'target': str(source), 'preserved': str(destination),
            'name': row['target']['name'], 'kind': row['target']['kind'],
            'bytes': source.stat().st_size, 'sha256': sha(destination),
            'profile': row['profile'], 'features': row['features']})
        assert sha(source) == manifest['artifacts'][-1]['sha256']
pool = root / 'evidence' / 'common-runtime-libraries'
pool.mkdir(exist_ok=True)
for source in sorted((target / 'release/deps').glob('*.so')):
    digest = sha(source)
    destination = pool / (digest + '-' + source.name)
    if not destination.exists():
        shutil.copy2(source, destination)
    assert sha(destination) == digest
    manifest['artifacts'].append({'target': str(source), 'preserved': str(destination),
        'name': source.name, 'kind': ['shared-library'], 'bytes': source.stat().st_size,
        'sha256': digest})
inputs = set()
for source in sorted((target / 'release').rglob('*.d')):
    destination = build / 'compiler-depfiles' / source.relative_to(target)
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)
    manifest['input_depfiles'].append({'source': str(source), 'preserved': str(destination),
                                     'sha256': sha(destination)})
    for line in source.read_text().replace('\\\n', ' ').splitlines():
        if line.startswith('#') or ': ' not in line:
            continue
        for token in re.findall(r'(?:\\.|[^\s])+', line.split(': ', 1)[1]):
            path = Path(re.sub(r'\\(.)', r'\1', token))
            if path.is_absolute():
                inputs.add(path)
for path in sorted(inputs):
    manifest['compiler_inputs'].append({'path': str(path), 'exists': path.is_file(),
        **({'bytes': path.stat().st_size, 'sha256': sha(path)} if path.is_file() else {})})
for r in artifacts:
    if r not in owned:
        manifest['external_graph'].append({k: r[k] for k in
            ('package_id', 'target', 'profile', 'features', 'filenames')})
manifest['external_graph'].sort(key=lambda r: (r['package_id'], r['target']['name'], r['target']['kind']))
ldd = {}
for artifact in manifest['artifacts']:
    if artifact['kind'] in (['bin'], ['test']):
        result = subprocess.run(['ldd', artifact['preserved']], capture_output=True, text=True)
        ldd[artifact['target']] = {'exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}
(build / 'runtime-linkage.json').write_text(json.dumps(ldd, indent=2) + '\n')
(build / 'preserved-artifacts.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps({'label': label, 'artifacts': len(manifest['artifacts']),
    'depfiles': len(manifest['input_depfiles']), 'compiler_inputs': len(manifest['compiler_inputs']),
    'missing_inputs': sum(not x['exists'] for x in manifest['compiler_inputs']),
    'external_artifacts': len(manifest['external_graph'])}, indent=2))
