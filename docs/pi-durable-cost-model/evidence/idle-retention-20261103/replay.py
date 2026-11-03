"""Authenticate and recompute archived measurements without running any product."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('--directory', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
assert not args.output.exists()
load = lambda path: json.loads(path.read_text())
sha = lambda body: hashlib.sha256(body).hexdigest()
manifest = load(args.directory / 'manifest.json')

with tempfile.TemporaryDirectory(prefix='rustx-idle-retention-replay-') as tmp:
    root = Path(tmp)
    seen = set()
    for record in manifest['archives']:
        archive_path = args.directory / record['name']
        assert archive_path.stat().st_size == record['bytes']
        assert sha(archive_path.read_bytes()) == record['sha256']
        expected = [v for v in manifest['members'] if v['archive'] == record['name']]
        with tarfile.open(archive_path, 'r:xz') as archive:
            members = archive.getmembers()
            assert [v.name for v in members] == [v['path'] for v in expected]
            for member, identity in zip(members, expected):
                rel = Path(member.name)
                assert member.isfile() and not rel.is_absolute() and '..' not in rel.parts
                assert member.name not in seen
                seen.add(member.name)
                body = archive.extractfile(member).read()
                assert len(body) == identity['bytes'] and sha(body) == identity['sha256']
                target = root / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(body)

    def run(directory, script, *arguments):
        result = subprocess.run(
            [sys.executable, str(directory / script), *arguments],
            cwd=directory, capture_output=True, text=True,
            env=os.environ | {'PYTHONDONTWRITEBYTECODE': '1'},
        )
        assert result.returncode == 0, (str(directory), script, result.stdout, result.stderr)

    for directory in [root, root / 'refinement']:
        old = load(directory / 'native-audit.json')
        run(directory, 'audit-native.py', '--raw-only')
        new = load(directory / 'native-audit.json')
        assert old.pop('raw_only') is False and new.pop('raw_only') is True
        assert old == new
        scripts = [
            ('audit-cache.py', 'cache-audit.json'),
            ('audit-functional.py', 'functional-audit.json'),
            ('audit-source.py', 'source-audit.json'),
            ('audit-host.py', 'host-audit.json'),
        ]
        if directory == root:
            scripts += [('audit-heap.py', 'heap-audit.json'), ('audit-trim.py', 'trim-audit.json')]
        scripts += [('summarize.py', 'summary.json')]
        for script, output in scripts:
            old = load(directory / output)
            run(directory, script)
            assert load(directory / output) == old, output

    old_table = (root / 'idle-retention-tables.md').read_bytes()
    run(root, 'make-tables.py')
    assert (root / 'idle-retention-tables.md').read_bytes() == old_table
    proof = load(root / 'prearchive-proof.json')
    source = load(root / 'refinement/source-audit.json')
    assert proof['accepted'] and proof['source_inputs'] == source['source_inputs'] == 973
    assert proof['source_head'] == source['head'] and len(proof['executables']) == 3
    assert proof['new_pi_deepseek_products'] == 0
    for row in proof['wrappers']:
        assert sha((root / row['path']).read_bytes()) == row['sha256']
    assert sha((root / 'heap-tool-identity.json').read_bytes()) == proof['heap_tool_identity_sha256']
    assert sha((root / 'allocator-control-identity.json').read_bytes()) == proof['allocator_control_identity_sha256']
    assert sha((root / 'pi-memory-reference.json').read_bytes()) == proof['prior_pi_reference_sha256']
    assert sha((root / 'heap-sample.cc').read_bytes()) == load(root / 'heap-tool-identity.json')['helper']['source_sha256']
    assert sha((root / 'allocator-control.c').read_bytes()) == load(root / 'allocator-control-identity.json')['source_sha256']
    native = [v for directory in [root, root / 'refinement']
              for v in load(directory / 'native-audit.json')['products']]
    result = {
        'accepted': True,
        'archives': manifest['archives'],
        'verified_members': len(seen),
        'native_products_reparsed': len(native),
        'native_consistent_images': sum(v['sessions'] for v in native),
        'functional_products': sum(len(load(directory / 'functional-audit.json')['products'])
                                   for directory in [root, root / 'refinement']),
        'heap_profiles': len(load(root / 'heap-audit.json')['products']),
        'controlled_allocator_products': len(load(root / 'trim-audit.json')['products']),
        'tests': source['tests'],
        'new_pi_deepseek_products': 0,
        'archived_tables_sha256': sha(old_table),
        'tables_sha256': sha(old_table.rstrip(b'\n') + b'\n'),
        'tables_export': 'Raw archived table reproduction is byte exact; publication normalizes terminal newlines to exactly one.',
        'tables_reproduced': True,
        'scope': 'Authenticates every archive/member and reparses native PID counters, complete wire histories, consistent SQLite images/integrity/FKs/cancellation, cache rotations and Source reads, functional reopen checks, sampled heap denominators, allocator-return counters, source/check/build maps, host records and all tables. Executes only archived Python arithmetic/audits: no product, compiler, git, symbolizer or network. Deliberately excluded binaries/libraries are covered by the stored prearchive byte proofs, not freshly verified.',
    }
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))
