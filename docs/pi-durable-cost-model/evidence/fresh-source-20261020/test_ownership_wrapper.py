"""Run the complete wrapper against tiny fake build metadata, never products.

Only its fixed workspace literal is redirected to an isolated fixture. Parser,
mapping, schedule, per-sample checks, watchdog wiring and final manifest run as
written. Every external command is mocked or forbidden.
"""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import types
import unittest
from unittest.mock import patch

import ownership_adapter as adapter


class FullWrapperTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.products = self.root / 'evidence/products'
        self.products.mkdir(parents=True)
        self.output = self.products / 'cohort'
        self.here = Path(__file__).parent
        self.wrapper = self.here / 'run-native-cohort.py'
        self.runtime = self.root / 'repo/benchmarks/runtime'
        self.runtime.mkdir(parents=True)
        for name in ['controller.py', 'observer_oracle.py']:
            (self.runtime / name).write_text('# mock module content; never imported\n')
        driver = (self.here / 'ownership-driver-compare-revisions.py').read_bytes()
        (self.runtime / 'compare-revisions.py').write_bytes(driver)
        self.binaries = {}
        for label in ['baseline', 'geometry', 'scoped', 'ownership']:
            build = self.root / ('evidence/build-' + label)
            build.mkdir()
            binary = build / 'conformance'
            binary.write_bytes(label.encode())
            self.binaries[label] = binary
            identity = {'head': adapter.BASE if label == 'baseline' else adapter.HEAD,
                        'production_tree': adapter.TREE, 'verified': True, 'harness': {'fixture': 'fixed'},
                        'binary': str(binary), 'sha256': adapter.sha(binary), 'root_artifacts': []}
            artifacts = []
            if label == 'ownership':
                library = build / 'ordinary.rlib'
                library.write_bytes(b'ordinary library fixture')
                identity.update(library_sha256=adapter.sha(library), root_artifacts=[
                    {'name': 'rustx', 'kind': ['lib'], 'fresh': False},
                    {'name': 'conformance', 'kind': ['test'], 'fresh': False}])
                for name, kind, test, file in [('rustx', ['lib'], False, library), ('conformance', ['test'], True, binary)]:
                    artifacts.append({'name': name, 'kind': kind, 'target': str(file), 'preserved': str(file),
                                      'sha256': adapter.sha(file), 'features': [],
                                      'profile': {'opt_level': '3', 'debuginfo': 0, 'test': test,
                                                  'debug_assertions': False, 'overflow_checks': False}})
            (build / 'identity.json').write_text(json.dumps(identity))
            (build / 'preserved-artifacts.json').write_text(json.dumps({'source_head': adapter.HEAD, 'artifacts': artifacts}))
        self.calls = []

    def argv(self):
        return ['--before', str(self.binaries['baseline']), '--after', str(self.binaries['ownership']), '--output', str(self.output), '--pairs', '3']

    def run_wrapper(self, argv, output=None, corrupt_interval=False):
        output = output or self.output
        calls, spawns = [], []
        def original_run(argv, stem, case, env, project):
            calls.append({'program': argv[0], 'case': case, 'stem': str(stem), 'mode': env['RUSTX_BENCH_OBSERVER_MODE']})
            if corrupt_interval and len(calls) == 2:
                log = output.with_name(output.name + '-product-intervals.jsonl')
                value = json.loads(log.read_text())
                value['label'] = 'geometry'
                log.write_text(json.dumps(value) + '\n')
            stem.with_suffix('.process.json').write_text('{"accepted":true}')
            return {'accepted': True}
        controller = types.SimpleNamespace(CASES=['short-small', 'short-large'], TEST='mock-only',
                                           OBSERVER_MODES=['native-workflow', 'no-live-subscriber', 'live-subscriber'], run=original_run)
        def forbidden(*args, **kwargs):
            spawns.append(args)
            raise AssertionError('test must never start an external product/command')
        def output_command(argv, **kwargs):
            if argv == ['node', '--version']: return 'v24.19.0\n'
            if argv == ['git', 'rev-parse', 'HEAD']: return 'mock-controller-source\n'
            raise AssertionError('unexpected check_output: ' + repr(argv))
        def point(_):
            return {'monotonic': time.monotonic(), 'free': 32 * adapter.GIB, 'memory_limit': 16 * adapter.GIB,
                    'memory_current': adapter.GIB, 'anon_shmem': adapter.GIB, 'full_avg10': 0., 'oom': 0, 'oom_kill': 0}
        real_iterdir = Path.iterdir
        def iterdir(path):
            return iter(()) if str(path) == '/proc' else real_iterdir(path)
        source = self.wrapper.read_text()
        literal = "root = Path('/workspace/pr485-study')"
        self.assertEqual(source.count(literal), 1)
        code = compile(source.replace(literal, 'root = Path(' + repr(str(self.root)) + ')'), str(self.wrapper), 'exec')
        cwd, paths, error = os.getcwd(), list(sys.path), None
        try:
            with patch.dict(os.environ, {'RUSTX_BENCH_CANDIDATE_BUILD': str(self.binaries['ownership'].parent)}), \
                 patch.dict(sys.modules, {'controller': controller}), patch.object(sys, 'argv', [str(self.wrapper), *argv]), \
                 patch.object(subprocess, 'Popen', forbidden), patch.object(subprocess, 'check_output', output_command), \
                 patch.object(subprocess, 'run', lambda argv, **kwargs: subprocess.CompletedProcess(argv, 0, '', '')), \
                 patch.object(adapter, 'resources', point), patch.object(os, 'statvfs', lambda _: types.SimpleNamespace(f_bavail=32*adapter.GIB, f_frsize=1)), \
                 patch.object(Path, 'iterdir', iterdir), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                try:
                    exec(code, {'__name__': '__main__', '__file__': str(self.wrapper)})
                except BaseException as caught:
                    error = caught
        finally:
            os.chdir(cwd)
            sys.path[:] = paths
        self.assertFalse(spawns)
        self.calls = calls
        return error

    def test_mixed_equals_duplicates_rejected_before_any_sample(self):
        base = self.argv()
        for suffix in [
            ['--after=' + str(self.binaries['geometry'])], ['--before=' + str(self.binaries['geometry'])],
            ['--output=' + str(self.products / 'unmonitored')], ['--pairs=3'], ['--pairs', '4']]:
            with self.subTest(suffix=suffix):
                error = self.run_wrapper(base + suffix)
                self.assertIsInstance(error, SystemExit)
                self.assertEqual(error.code, 2)
                self.assertFalse(self.calls)
                self.assertFalse(self.output.exists())

    def test_abbreviations_offsets_and_unplanned_options_rejected(self):
        base = self.argv()
        for argv in [base + ['--pair-offset=1'], base + ['--case=tool'], base + ['--observer-mode=native-workflow'],
                     ['--bef', *base[1:]], [*base[:-2], '--pai=3']]:
            with self.subTest(argv=argv):
                error = self.run_wrapper(argv)
                self.assertIsInstance(error, SystemExit)
                self.assertEqual(error.code, 2)
                self.assertFalse(self.calls)

    def test_equals_forms_are_canonical_and_outputs_are_monitored(self):
        output = self.products / 'equals-output'
        argv = ['--before=' + str(self.binaries['baseline']), '--after=' + str(self.binaries['ownership']),
                '--output=' + str(output), '--pairs=3']
        self.assertIsNone(self.run_wrapper(argv, output=output))
        manifest = json.loads(output.with_name(output.name + '-cohort-manifest.json').read_text())
        config = json.loads((output.with_name(output.name + '-watchdog') / 'watchdog-config.json').read_text())
        self.assertEqual(config['evidence_directory'], str(output))
        self.assertTrue(manifest['complete_18_products'])
        self.assertEqual(len(self.calls), 18)
        expected = []
        for mode, cases in [('live-subscriber', ['short-small', 'short-large']), ('no-live-subscriber', ['short-large'])]:
            for pair in range(3):
                for case in cases:
                    for label in (['baseline', 'ownership'] if pair % 2 == 0 else ['ownership', 'baseline']):
                        expected.append((str(self.binaries[label]), case, mode))
        self.assertEqual([(call['program'], call['case'], call['mode']) for call in self.calls], expected)
        for block in manifest['ownership_adapter']['canonical_blocks']:
            for option in ['--before', '--after', '--output', '--pairs']:
                self.assertEqual(block.count(option), 1)
            self.assertEqual(block[block.index('--after') + 1], str(self.binaries['ownership']))
            self.assertTrue(Path(block[block.index('--output') + 1]).is_relative_to(output))

    def test_wrong_registered_artifact_is_rejected(self):
        argv = self.argv(); argv[3] = str(self.binaries['geometry'])
        error = self.run_wrapper(argv)
        self.assertIsInstance(error, AssertionError)
        self.assertIn('exact baseline/candidate pair', str(error))
        self.assertFalse(self.calls)

    def test_wrong_ordinary_artifact_profile_is_rejected(self):
        path = self.binaries['ownership'].parent / 'preserved-artifacts.json'
        value = json.loads(path.read_text()); value['artifacts'][0]['profile']['test'] = True
        path.write_text(json.dumps(value))
        self.assertIsInstance(self.run_wrapper(self.argv()), ValueError)
        self.assertFalse(self.calls)

    def test_output_symlink_cannot_escape_monitored_products_root(self):
        outside = self.root / 'outside'; outside.mkdir()
        alias = self.products / 'escaped'; alias.symlink_to(outside, target_is_directory=True)
        argv = self.argv(); argv[5] = str(alias / 'cohort')
        error = self.run_wrapper(argv)
        self.assertIsInstance(error, SystemExit)
        self.assertFalse(self.calls)
        self.assertFalse((outside / 'cohort').exists())

    def test_final_manifest_checks_labels_not_only_count(self):
        error = self.run_wrapper(self.argv(), corrupt_interval=True)
        self.assertIsInstance(error, RuntimeError)
        manifest = json.loads(self.output.with_name(self.output.name + '-cohort-manifest.json').read_text())
        self.assertEqual(len(manifest['products']), 18)
        self.assertFalse(manifest['complete_18_products'])
        self.assertEqual(manifest['products'][0]['label'], 'geometry')


if __name__ == '__main__':
    unittest.main()
