#!/usr/bin/env python3
"""Apply explicit compile AND link flags to every pinned OCCT translation unit."""
import json
import pathlib
import sys

source = pathlib.Path(sys.argv[1])
optimization, lto = sys.argv[2:4]
if optimization not in ('Oz', 'O2', 'O3') or lto not in ('on', 'off'):
    raise SystemExit('Expected Oz/O2/O3 and on/off LTO')
cmake = source / 'CMakeLists.txt'
text = cmake.read_text()
old = '\ttarget_compile_options (OcctImportJS PUBLIC "$<$<CONFIG:Release>:-Oz>")'
if text.count(old) != 1:
    raise SystemExit('Pinned upstream compile flag changed')
flags = f'-{optimization}' + (' -flto' if lto == 'on' else '')
text = text.replace(old, f'\ttarget_compile_options (OcctImportJS PUBLIC {flags})\n\ttarget_link_options (OcctImportJS PUBLIC {flags})')
cmake.write_text(text)
(source / 'rustx-build-config.json').write_text(json.dumps({
    'optimization': optimization, 'lto': lto == 'on',
    'compileFlags': flags.split(), 'linkFlags': flags.split(),
    'scope': 'OcctImportJS target: importer and all OCCT sources',
}, indent=2) + '\n')
