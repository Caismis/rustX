#!/usr/bin/env python3
"""Reject a mixed optimization/LTO build using generated effective commands."""
import hashlib
import json
import pathlib
import shlex
import sys

root = pathlib.Path(sys.argv[1])
config = json.loads((root / 'rustx-build-config.json').read_text())
compile_bytes = (root / 'build/compile_commands.json').read_bytes()
commands = json.loads(compile_bytes)
expected = '-' + config['optimization']
for item in commands:
    args = shlex.split(item['command'])
    optimizations = [arg for arg in args if arg.startswith('-O')]
    if optimizations != [expected] or ('-flto' in args) != config['lto']:
        raise SystemExit(f"Mixed flags in {item['file']}: {optimizations}")
lines = (root / 'effective-commands.txt').read_text().splitlines()
links = [line for line in lines if 'Release/occt-import-js.js' in line and ' -o ' in line and ' -c ' not in line]
if len(links) != 1:
    raise SystemExit('Expected one effective link command')
link_args = shlex.split(links[0])
if [arg for arg in link_args if arg.startswith('-O')] != [expected] or ('-flto' in link_args) != config['lto']:
    raise SystemExit('Link flags do not match compile flags')
modified = ['importer-brep.cpp', 'importer-brep.hpp', 'importer-iges.cpp', 'importer-step.cpp', 'importer-utils.cpp', 'importer-utils.hpp', 'importer.hpp', 'js-interface.cpp', 'native-profile.hpp']
source_hashes = {name: hashlib.sha256((root / 'occt-import-js/src' / name).read_bytes()).hexdigest() for name in modified}
record = {**config, 'modifiedSourcesSha256': source_hashes, 'translationUnits': len(commands),
          'occtTranslationUnits': sum('/occt/src/' in item['file'] for item in commands),
          'compileCommandsSha256': hashlib.sha256(compile_bytes).hexdigest(),
          'effectiveLinkCommandSha256': hashlib.sha256(links[0].encode()).hexdigest(),
          'effectiveOptimizationFlags': [expected], 'effectiveCompileLto': config['lto'], 'effectiveLinkLto': config['lto']}
(root / 'verified-build.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record))
