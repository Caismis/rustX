"""Authenticate and recompute this cohort offline without running any product."""
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
with tempfile.TemporaryDirectory(prefix='rustx-observation-ownership-replay-') as tmp:
    root = Path(tmp)
    seen = set()
    for record in manifest['archives']:
        path = args.directory / record['name']
        assert path.stat().st_size == record['bytes'] and sha(path.read_bytes()) == record['sha256']
        expected = [v for v in manifest['members'] if v['archive'] == record['name']]
        with tarfile.open(path, 'r:xz') as archive:
            members = archive.getmembers()
            assert [v.name for v in members] == [v['path'] for v in expected]
            for member, identity in zip(members, expected):
                rel = Path(member.name)
                assert member.isfile() and not rel.is_absolute() and '..' not in rel.parts and member.name not in seen
                seen.add(member.name)
                body = archive.extractfile(member).read()
                assert len(body) == identity['bytes'] and sha(body) == identity['sha256']
                target = root / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(body)

    def run(script, *arguments):
        result = subprocess.run([sys.executable, str(root / script), *arguments], cwd=root,
                                capture_output=True, text=True,
                                env=os.environ | {'PYTHONDONTWRITEBYTECODE': '1'})
        assert result.returncode == 0, (script, result.stdout, result.stderr)

    old = load(root / 'native-audit.json')
    run('audit-native.py', '--raw-only')
    new = load(root / 'native-audit.json')
    assert old.pop('raw_only') is False and new.pop('raw_only') is True and old == new
    for script, output in [('audit-requests.py', 'requests-audit.json'),
                           ('audit-cache.py', 'cache-audit.json'),
                           ('audit-functional.py', 'functional-audit.json'),
                           ('audit-source.py', 'source-audit.json'),
                           ('audit-host.py', 'host-audit.json'),
                           ('audit-census.py', 'census-audit.json'),
                           ('audit-heap.py', 'heap-audit.json'),
                           ('summarize.py', 'summary.json')]:
        old = load(root / output)
        run(script)
        assert load(root / output) == old, output
    table = (root / 'observation-ownership-tables.md').read_bytes()
    run('make-tables.py')
    assert (root / 'observation-ownership-tables.md').read_bytes() == table
    assert table.endswith(b'\n') and not table.endswith(b'\n\n')
    proof = load(root / 'prearchive-proof.json')
    source = load(root / 'source-audit.json')
    assert proof['accepted'] and proof['source_inputs'] == source['source_inputs'] == 974
    assert proof['source_head'] == source['head'] and proof['source_map_sha256'] == source['source_map_sha256']
    assert len(proof['executables']) == 8 and proof['new_pi_deepseek_products'] == 0
    for row in proof['wrappers']:
        assert sha((root / row['path']).read_bytes()) == row['sha256']
    for filename, key in [('profiler-identity.json', 'profiler_identity_sha256'),
                          ('heap-sample.cc', 'heap_helper_source_sha256'),
                          ('prior-pi-memory-reference.json', 'prior_pi_reference_sha256'),
                          ('source.patch', 'source_patch_sha256')]:
        assert sha((root / filename).read_bytes()) == proof[key]
    symbols = load(root / 'symbol-commands.json')
    assert len(symbols) == proof['symbol_commands']
    for row in symbols:
        assert sha((root / row['output']).read_bytes()) == row['output_sha256']
        profile = root / row['directory'] / Path(row['command'][-1]).name
        assert sha(profile.read_bytes()) == row['profile_sha256']
    products = load(root / 'native-audit.json')['products']
    result = {'accepted': True, 'archives': manifest['archives'], 'verified_members': len(seen),
              'native_products_reparsed': len(products),
              'native_consistent_images': sum(v['sessions'] for v in products),
              'functional_products': len(load(root / 'functional-audit.json')['products']),
              'matching_full_http_pairs': len(load(root / 'requests-audit.json')['pairs']),
              'native_owner_cohorts': len(load(root / 'census-audit.json')['products']),
              'heap_profiles': len(load(root / 'heap-audit.json')['products']),
              'tests': source['tests'], 'new_pi_deepseek_products': 0,
              'tables_sha256': sha(table), 'tables_reproduced': True,
              'scope': 'Every archive/member authenticated. Reparsed native PID counters/full wire histories/SQLite integrity/FKs/cancellation, exact full provider bodies, cold/warm/Inspector/Source, functional live/reopen, source/check/build maps and target Serde features, shared-host observations, actual native owner/vector census and complete heap profile denominators, all summary/table arithmetic. Only archived Python audits run: no product/compiler/git/symbolizer/network. Excluded executable/library bytes use stored prearchive proofs, not a fresh offline verification.'}
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))
