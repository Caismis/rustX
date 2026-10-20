"""Fresh-host provenance fixtures: bytes only; never build or run products."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
import ownership_adapter as adapter


def fixture(root, label):
    build = root / ('evidence/build-' + label)
    build.mkdir(parents=True, exist_ok=True)
    repo, target = root / ('repo-' + label), root / 'target'
    head, tree = adapter.SOURCES[label]
    identity = {'head': head, 'production_tree': tree, 'verified': True, 'repo': str(repo),
        'harness': {'fixture': 'fixed'}, 'source_sha256': dict(adapter.LOCKED_INPUTS),
        'rustc': 'rustc 1.98.1 (48a229cea 2026-09-01)\ncommit-hash: 48a229ceaefd4985c50990b14116b6d856af0985\nhost: x86_64-unknown-linux-gnu\nLLVM version: 22.1.8',
        'cargo': 'cargo 1.98.1 (797e8a9bc 2026-08-05)',
        'build_environment': adapter.PINNED_ENV | {'CARGO_TARGET_DIR': str(target), 'RUSTC': '/pinned/bin/rustc', 'LIBCLANG_PATH': '/pinned/lib', 'BINDGEN_EXTRA_CLANG_ARGS': 'fixed'},
        'root_artifacts': [], 'shared_libraries': {}, 'linker_comment': 'Linker: LLD 22.1.8'}
    artifacts = []
    for name, kind, test, source in [('rustx', ['lib'], False, 'src/lib.rs'),
        ('conformance', ['test'], True, 'tests/conformance/main.rs'), ('rustx', ['bin'], False, 'src/main.rs'),
        ('bash-supervisor', ['bin'], False, 'src/bin/bash_supervisor.rs'),
        ('interactive-supervisor', ['bin'], False, 'src/bin/interactive_supervisor.rs')]:
        file = build / ('ordinary.rlib' if kind == ['lib'] else name)
        file.write_bytes((label + ':' + str(kind) + ':' + name).encode())
        dest = target / 'release' / (file.name if kind == ['bin'] else 'deps/' + file.name)
        identity['root_artifacts'].append({'name': name, 'kind': kind, 'fresh': False, 'source_path': str(repo / source)})
        artifacts.append({'name': name, 'kind': kind, 'target': str(dest), 'preserved': str(file), 'sha256': adapter.sha(file), 'features': [],
            'profile': {'opt_level': '3', 'debuginfo': 0, 'test': test, 'debug_assertions': False, 'overflow_checks': False}})
    identity.update(binary=str(build / 'conformance'), sha256=adapter.sha(build / 'conformance'), library_sha256=adapter.sha(build / 'ordinary.rlib'))
    preserved = {'source_head': head, 'artifacts': artifacts}
    (build / 'identity.json').write_text(json.dumps(identity))
    (build / 'preserved-artifacts.json').write_text(json.dumps(preserved))
    return identity, preserved


class FreshHostTests(unittest.TestCase):
    def test_symmetric_mapping_and_malformed_baseline_rejections(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            baseline, bp = fixture(root, 'baseline')
            candidate, cp = fixture(root, 'ownership')
            adapter.validate_product(baseline, bp, 'baseline')
            adapter.validate_product(candidate, cp, 'ownership')
            adapter.validate_pair(baseline, candidate)
            adapter.validate_candidate(candidate, cp, baseline)
            mutations = [lambda i,p: i.update(head='wrong'), lambda i,p: i.update(production_tree=adapter.TREE),
                lambda i,p: i.update(root_artifacts=[]), lambda i,p: p.update(artifacts=[]),
                lambda i,p: i.update(library_sha256='wrong'), lambda i,p: i.update(sha256='wrong'),
                lambda i,p: i['source_sha256'].update({'Cargo.lock':'wrong'}),
                lambda i,p: i.update(rustc='wrong'), lambda i,p: i.update(linker_comment='wrong'), lambda i,p: p['artifacts'][0]['profile'].update(test=True),
                lambda i,p: p['artifacts'][0]['profile'].update(opt_level='0'),
                lambda i,p: p['artifacts'][0].update(features=['issue-387-profile']),
                lambda i,p: p['artifacts'][2].update(sha256='wrong'),
                lambda i,p: p['artifacts'][2].update(target='/wrong/rustx'),
                lambda i,p: i.update(shared_libraries={'missing.so':'wrong'})]
            for change in mutations:
                with self.subTest(mutation=mutations.index(change)):
                    i,p=copy.deepcopy(baseline),copy.deepcopy(bp)
                    change(i,p)
                    with self.assertRaises(ValueError): adapter.validate_product(i,p,'baseline')
            (root / 'evidence/build-baseline/rustx').write_bytes(b'corrupted helper')
            with self.assertRaises(ValueError): adapter.validate_product(baseline,bp,'baseline')

    def test_baseline_then_candidate_build_entry_without_compilation(self):
        from unittest.mock import patch
        import time
        import types
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inputs = {}
            for label in adapter.SOURCES:
                repo = root / ('source-' + label)
                repo.mkdir()
                for name in [*adapter.LOCKED_INPUTS, 'src/runtime_client/projection.rs']:
                    file = repo / name
                    file.parent.mkdir(parents=True, exist_ok=True)
                    file.write_bytes(b'mock source')
                inputs[str(repo)] = label
            model, _ = fixture(root / 'fixture', 'baseline')
            def output(argv, cwd=None, **kwargs):
                if argv[0] == 'git':
                    label = inputs[str(cwd)]
                    if argv[1:] == ['rev-parse', 'HEAD']: return adapter.SOURCES[label][0]
                    if argv[1:] == ['rev-parse', 'HEAD:src']: return adapter.SOURCES[label][1]
                    if argv[1] in ('status', 'ls-files'): return ''
                if argv[0] == 'rustc': return model['rustc']
                if argv[0] == 'cargo': return model['cargo']
                if argv[0] == 'readelf': return 'Linker: LLD 22.1.8'
                raise AssertionError(argv)
            real_sha = adapter.sha
            def digest(file):
                file = Path(file)
                if str(file.parent) in inputs and file.name in adapter.LOCKED_INPUTS:
                    return adapter.LOCKED_INPUTS[file.name]
                return real_sha(file)
            calls = []
            def fake_build(argv, cwd, env, stdout, stderr, **kwargs):
                calls.append((inputs[str(cwd)], argv))
                self.assertIn('--no-run', argv)
                self.assertNotIn('--lib', argv)
                i,p = fixture(root / 'products', inputs[str(cwd)])
                for item, row in zip(p['artifacts'], i['root_artifacts']):
                    target = Path(env['CARGO_TARGET_DIR']) / 'release' / ('deps' if item['kind'] != ['bin'] else '') / Path(item['preserved']).name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(Path(item['preserved']).read_bytes())
                    stdout.write(json.dumps({'reason':'compiler-artifact', 'target': {'name':row['name'], 'kind':row['kind'], 'src_path':str(Path(cwd)/Path(row['source_path']).relative_to(i['repo']))},
                        'fresh':False,'filenames':[str(target)],'features':[], 'profile':item['profile']})+'\n')
                return types.SimpleNamespace(pid=999999999, wait=lambda:0, poll=lambda:0)
            point={'free':32*adapter.GIB, 'monotonic':time.monotonic(), 'oom':0,'oom_kill':0,
                   'anon_shmem':0,'memory_limit':16*adapter.GIB,'full_avg10':0,'psi_available':True,'memory_current':0}
            with patch.object(adapter.subprocess,'check_output',side_effect=output), patch.object(adapter.subprocess,'Popen',side_effect=fake_build), patch.object(adapter,'sha',side_effect=digest), patch.object(adapter,'resources',return_value=point):
                for label in adapter.SOURCES:
                    adapter.watch_build(root/('source-'+label), root/('build-'+label), root, label,
                        root/'toolchain',root/'libclang',root/'build-baseline' if label=='ownership' else None)
            self.assertEqual([x[0] for x in calls],['baseline','ownership'])
            for label in adapter.SOURCES:
                i=json.loads((root/('build-'+label)/'identity.json').read_text())
                self.assertTrue(i['verified'])
                self.assertEqual(i['head'],adapter.SOURCES[label][0])

    def test_build_growth_cap_and_protected_reserve(self):
        from unittest.mock import patch
        import time
        point={'free':32*adapter.GIB, 'monotonic':time.monotonic(), 'oom':0,'oom_kill':0,
               'anon_shmem':0,'memory_limit':16*adapter.GIB,'full_avg10':0,'psi_available':True,'memory_current':0}
        with tempfile.TemporaryDirectory() as tmp, patch.object(adapter,'resources',return_value=point):
            guard=adapter.Guard(tmp,Path(tmp)/'out','build')
            self.assertEqual(guard.reason(point,2*adapter.GIB+1),'allocated-disk-budget')
            self.assertEqual(guard.reason(point | {'free':18*adapter.GIB},0),'build-protected-cohort-reserve-18GiB')

if __name__ == '__main__': unittest.main()
