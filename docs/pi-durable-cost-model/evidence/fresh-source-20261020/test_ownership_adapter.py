"""Metadata/fail-fast/watchdog checks. Never launch Cargo or native measurements."""
import copy
import errno
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import threading
import time
import types
import unittest
from unittest.mock import patch

import ownership_adapter as adapter


class AdapterTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.binary = self.root / 'conformance'
        self.library = self.root / 'ordinary.rlib'
        self.binary.write_bytes(b'fixture executable identity, never executed')
        self.library.write_bytes(b'fixture ordinary library identity')
        self.baseline = {'head': adapter.BASE, 'harness': {'fixture': 'unchanged'}}
        self.identity = {'head': adapter.HEAD, 'production_tree': adapter.TREE, 'verified': True,
                         'harness': self.baseline['harness'], 'binary': str(self.binary),
                         'sha256': adapter.sha(self.binary), 'library_sha256': adapter.sha(self.library),
                         'root_artifacts': [{'name': 'rustx', 'kind': ['lib'], 'fresh': False},
                                            {'name': 'conformance', 'kind': ['test'], 'fresh': False}]}
        self.preserved = {'source_head': adapter.HEAD, 'artifacts': [
            {'name': 'rustx', 'kind': ['lib'], 'preserved': str(self.library), 'sha256': adapter.sha(self.library),
             'profile': {'opt_level': '3', 'debuginfo': 0, 'test': False, 'debug_assertions': False, 'overflow_checks': False}, 'features': []},
            {'name': 'conformance', 'kind': ['test'], 'preserved': str(self.binary), 'sha256': adapter.sha(self.binary),
             'profile': {'opt_level': '3', 'debuginfo': 0, 'test': True, 'debug_assertions': False, 'overflow_checks': False}, 'features': []}]}

    def test_exact_ordinary_product_mapping_and_rejections(self):
        adapter.validate_candidate(self.identity, self.preserved, self.baseline)
        mutations = [lambda i, p: i.update(head='wrong'), lambda i, p: i.update(production_tree='old-test-tree'),
                     lambda i, p: i['root_artifacts'][0].update(fresh=True),
                     lambda i, p: p['artifacts'][0]['profile'].update(test=True),
                     lambda i, p: p['artifacts'][0]['profile'].update(opt_level='0'),
                     lambda i, p: p['artifacts'][0]['profile'].update(debug_assertions=True),
                     lambda i, p: p['artifacts'][0].update(features=['issue-387-profile']),
                     lambda i, p: i.update(harness={'fixture': 'changed'}),
                     lambda i, p: i.update(sha256='wrong')]
        for change in mutations:
            with self.subTest(change=mutations.index(change)):
                identity, preserved = copy.deepcopy(self.identity), copy.deepcopy(self.preserved)
                change(identity, preserved)
                with self.assertRaises(ValueError):
                    adapter.validate_candidate(identity, preserved, self.baseline)

    def compare_script(self, fail_product=False, fail_comparison=False):
        before, after, out = self.root / 'before', self.root / 'after', self.root / 'cohort'
        before.write_bytes(b'base'); after.write_bytes(b'candidate')
        calls = []
        def fake_run(argv, stem, case, env, project):
            calls.append(str(stem))
            result = {'accepted': not fail_product}
            stem.with_suffix('.process.json').write_text(json.dumps(result))
            adapter.accepted(result)
            return result
        controller = types.SimpleNamespace(CASES=['short-large'], TEST='fixture', OBSERVER_MODES=['live-subscriber'], run=fake_run)
        compare = lambda *args, **kwargs: subprocess.CompletedProcess(args[0], 7 if fail_comparison else 0, 'retained stdout', 'retained stderr')
        script = Path(__file__).with_name('ownership-driver-compare-revisions.py')
        argv = [str(script), '--before', str(before), '--after', str(after), '--output', str(out), '--pairs', '3', '--observer-mode', 'live-subscriber']
        with patch.dict(sys.modules, {'controller': controller}), patch.object(sys, 'argv', argv), \
             patch.object(subprocess, 'run', adapter.comparison_runner(compare, out)):
            with self.assertRaises(RuntimeError):
                runpy.run_path(str(script), run_name='__main__')
        return calls, out

    def test_first_product_oracle_failure_stops_before_second_product(self):
        calls, out = self.compare_script(fail_product=True)
        self.assertEqual(len(calls), 1)
        self.assertEqual(json.loads(next(out.glob('*.process.json')).read_text()), {'accepted': False})

    def test_first_pair_oracle_failure_stops_before_next_pair(self):
        calls, out = self.compare_script(fail_comparison=True)
        self.assertEqual(len(calls), 2)
        failure = json.loads((out / 'first-comparison-failure.json').read_text())
        self.assertEqual((failure['returncode'], failure['stderr']), (7, 'retained stderr'))

    def test_allocated_count_deduplicates_hardlinks(self):
        one = self.root / 'data'; one.write_bytes(b'x' * 8192)
        os.link(one, self.root / 'alias')
        self.assertEqual(adapter.allocated([self.root, self.root]), sum(p.stat().st_blocks * 512 for p in [self.binary, self.library, one]))

    def test_schedule_is_fixed_and_bounded(self):
        argv = ['--before', 'base', '--after', 'candidate', '--output', str(self.root / 'evidence/products/cohort'), '--pairs', '3']
        options = adapter.parse_ownership_args(argv, self.root)
        blocks = adapter.cohort_arguments(options, 'compare.py')
        self.assertEqual(len(blocks), 2)
        self.assertEqual([block[block.index('--observer-mode') + 1] for block in blocks], ['live-subscriber', 'no-live-subscriber'])
        self.assertEqual([block.count('--case') for block in blocks], [2, 1])
        self.assertEqual(argv[-1], '3')
        with self.assertRaises(SystemExit): adapter.parse_ownership_args(argv + ['--case', 'stream'], self.root)
        with self.assertRaises(SystemExit): adapter.parse_ownership_args(argv[:-1] + ['4'], self.root)

    def point(self):
        return {'monotonic': time.monotonic(), 'free': 32 * adapter.GIB,
                'memory_limit': 16 * adapter.GIB, 'memory_current': adapter.GIB,
                'anon_shmem': adapter.GIB, 'full_avg10': 0., 'psi_available': True, 'oom': 0, 'oom_kill': 0}

    def test_entry_gates_do_not_start_a_child_or_create_output(self):
        with patch.object(adapter, 'resources', return_value=self.point() | {'free': 9 * adapter.GIB}):
            for kind in ['build', 'cohort']:
                with self.assertRaises(RuntimeError):
                    adapter.Guard(self.root, self.root / kind, kind)
                self.assertFalse((self.root / kind).exists())
        self.assertEqual((adapter.BUILD_ENTRY, adapter.COHORT_ENTRY), (18 * adapter.GIB, 16 * adapter.GIB))

    def test_memory_disk_and_timeout_criteria(self):
        with patch.object(adapter, 'resources', side_effect=lambda _: self.point()):
            guard = adapter.Guard(self.root, self.root / 'log', 'cohort')
        point = self.point()
        self.assertEqual(guard.reason(point | {'free': 6 * adapter.GIB}, 0), 'disk-soft-stop-6GiB')
        self.assertEqual(guard.reason(point, adapter.GIB + 1), 'allocated-disk-budget')
        self.assertEqual(guard.reason(point, 0, .75 * adapter.GIB + 1), 'evidence-or-temporary-allocation-budget')
        self.assertEqual(guard.reason(point | {'oom_kill': 1}, 0), 'new-cgroup-oom-event')
        memory = point | {'anon_shmem': 14 * adapter.GIB}
        self.assertIsNone(guard.reason(memory, 0))
        self.assertIn('anon-plus-shmem', guard.reason(memory | {'monotonic': memory['monotonic'] + 3.1}, 0))
        guard.memory_since = None
        pressure = point | {'full_avg10': 10.}
        self.assertIsNone(guard.reason(pressure, 0))
        self.assertIn('memory-full', guard.reason(pressure | {'monotonic': pressure['monotonic'] + 5.1}, 0))
        self.assertEqual(guard.reason(point | {'monotonic': guard.deadline + 1}, 0), 'cohort-30m-timeout')
        guard.product_deadline = point['monotonic'] - 1
        self.assertEqual(guard.reason(point, 0), 'product-180s-timeout')

    def test_timeout_kills_only_registered_process_group_and_retains_stop(self):
        with patch.object(adapter, 'resources', side_effect=lambda _: self.point()):
            guard = adapter.Guard(self.root, self.root / 'watchdog', 'cohort').start()
            owned = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'], start_new_session=True)
            unrelated = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'], start_new_session=True)
            try:
                guard.register(owned)
                guard.product_deadline = time.monotonic() + .01
                owned.wait(timeout=6)
                self.assertIsNone(unrelated.poll())
                with self.assertRaises(RuntimeError):
                    guard.close()
                self.assertEqual(json.loads((guard.output / 'watchdog-stop.json').read_text())['reason'], 'product-180s-timeout')
            finally:
                if owned.poll() is None: owned.kill()
                if unrelated.poll() is None: unrelated.terminate()
                owned.wait(); unrelated.wait()

    def write_failure_guard(self, monitor):
        gate = threading.Event()
        real_open, real_write = Path.open, Path.write_text
        failures = []
        class BrokenLog:
            def __enter__(self): return self
            def __exit__(self, *args): return False
            def write(self, value):
                gate.wait(4)
                raise OSError(errno.ENOSPC, 'injected resource log full')
            def flush(self): pass
        def opened(path, *args, **kwargs):
            if monitor and path.name == 'watchdog-resources.jsonl': return BrokenLog()
            return real_open(path, *args, **kwargs)
        def written(path, *args, **kwargs):
            if path.name in ('watchdog-stop.json', 'owned-process-status.json'):
                raise OSError(errno.ENOSPC, 'injected diagnostic full')
            return real_write(path, *args, **kwargs)
        with patch.object(adapter, 'resources', side_effect=lambda _: self.point()), \
             patch.object(Path, 'open', opened), patch.object(Path, 'write_text', written), \
             patch.object(threading, 'excepthook', lambda args: failures.append(args)):
            guard = adapter.Guard(self.root, self.root / 'failed-writes', 'cohort').start()
            owned = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'], start_new_session=True)
            unrelated = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'], start_new_session=True)
            try:
                guard.register(owned)
                if monitor:
                    gate.set()
                    owned.wait(timeout=6)
                with self.assertRaises(RuntimeError): guard.close()
                owned.wait(timeout=6)
                self.assertIsNone(unrelated.poll())
                self.assertTrue(guard.diagnostic_errors)
                self.assertFalse(failures, 'logging failures must not escape monitoring')
                self.assertEqual(guard.failure['reason'], 'watchdog-observation-error' if monitor else 'owner-exit-with-live-child')
            finally:
                gate.set()
                guard.done.set()
                guard.thread.join(timeout=6)
                if owned.poll() is None: owned.kill()
                if unrelated.poll() is None: unrelated.terminate()
                owned.wait(); unrelated.wait()

    def test_monitor_log_and_stop_diagnostic_failure_still_stop_owned_child(self):
        self.write_failure_guard(monitor=True)

    def test_close_stop_diagnostic_failure_still_stops_owned_child(self):
        self.write_failure_guard(monitor=False)


if __name__ == '__main__':
    unittest.main()
