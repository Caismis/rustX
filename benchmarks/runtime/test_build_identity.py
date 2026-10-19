"""A checkout label or an artifact path cannot certify a cached binary."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    'build_revision', Path(__file__).with_name('build-revision.py'))
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class BuildIdentityTests(unittest.TestCase):
    def artifacts(self):
        return [
            {'reason': 'compiler-artifact', 'fresh': False,
             'target': {'name': name, 'kind': [kind], 'src_path': '/repo/' + source}}
            for name, kind, source in (
                ('rustx', 'lib', 'src/lib.rs'),
                ('conformance', 'test', 'tests/conformance/main.rs'))
        ]

    def test_requires_fresh_library_and_harness(self):
        for cached in (0, 1):
            records = self.artifacts()
            records[cached]['fresh'] = True
            with self.assertRaisesRegex(ValueError, 'freshly compiled'):
                build.checked_artifacts(records, Path('/repo'))

    def test_rejects_different_checkout(self):
        for changed in (0, 1):
            records = self.artifacts()
            records[changed]['target']['src_path'] = '/other/source.rs'
            with self.assertRaisesRegex(ValueError, 'different checkout'):
                build.checked_artifacts(records, Path('/repo'))

    def test_accepts_both_fresh_current_artifacts(self):
        records = self.artifacts()
        artifacts, library, benchmark = build.checked_artifacts(records, Path('/repo'))
        self.assertEqual(artifacts, records)
        self.assertIs(library, records[0])
        self.assertIs(benchmark, records[1])
