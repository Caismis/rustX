#!/usr/bin/env python3
"""Portable sequential postprocessing; run only after measured products stop."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('evidence_root', type=Path)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=False)
scripts = Path(__file__).resolve().parent
fixture = scripts / 'frozen-fixture'
files = []
for name in ('browser-baseline-vs-geometry-v2', 'browser-baseline-vs-scoped'):
    directory = a.evidence_root / 'products' / name
    manifest = json.loads((directory / 'cohort-manifest.json').read_text())
    for run in manifest['runs']:
        if run.get('accepted_by_producer'):
            files.append(str(directory / f"{run['pair']}-{run['case']}-{run['label']}" / 'sample.json'))
assert files
commands = [
    ['node', str(scripts / 'replay-browser-proofs.mjs'), str(fixture / 'drain.mjs'), str(a.output / 'wire-proof-replay.json'), *files],
    ['node', str(scripts / 'verify-browser-requests.mjs'), str(scripts / 'pi-oracle.mjs'), str(fixture / 'expected_tools.json'), str(a.output / 'requests-verification.json'), *files],
]
for command in commands:
    subprocess.run(command, check=True)
hashes = {name: hashlib.file_digest((scripts / name).open('rb'), 'sha256').hexdigest()
          for name in ('replay-browser-proofs.mjs', 'verify-browser-requests.mjs', 'pi-oracle.mjs')}
(a.output / 'verifier-identities.json').write_text(json.dumps(hashes, indent=2) + '\n')
