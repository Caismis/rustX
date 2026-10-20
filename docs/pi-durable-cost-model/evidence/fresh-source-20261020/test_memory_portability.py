"""Read-only signal fixtures; no Cargo or product process is launched."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import ownership_adapter as adapter


class MemoryPortabilityTests(unittest.TestCase):
    def signals(self, root, omit=None, pressure=None):
        group = root / 'cgroup'
        group.mkdir()
        files = {'memory.max': str(16 * adapter.GIB), 'memory.current': str(adapter.GIB),
                 'memory.stat': 'anon 1024\nshmem 0\n', 'memory.events': 'oom 0\noom_kill 0\n'}
        if pressure is not None:
            files['memory.pressure'] = pressure
        for name, value in files.items():
            if name != omit:
                (group / name).write_text(value)
        return group

    def test_missing_psi_is_null_and_usage_events_remain_enforced(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            point = adapter.resources(root, self.signals(root))
            self.assertFalse(point['psi_available'])
            self.assertIsNone(point['full_avg10'])
            point['free'] = 32 * adapter.GIB
            with patch.object(adapter, 'resources', return_value=point):
                guard = adapter.Guard(root, root / 'log', 'build').start()
                guard.close()
            config = json.loads((root / 'log/watchdog-config.json').read_text())
            self.assertFalse(config['psi_available_at_entry'])
            self.assertIsNone(config['initial']['full_avg10'])
            high = point | {'memory_current': 14 * adapter.GIB}
            self.assertIsNone(guard.reason(high, 0))
            self.assertEqual(guard.reason(high | {'monotonic': high['monotonic'] + 3.1}, 0),
                             'memory-current-within-2GiB-of-limit-for-3s')
            self.assertEqual(guard.reason(point | {'oom': 1}, 0), 'new-cgroup-oom-event')
            self.assertEqual(guard.reason(point | {'oom_kill': 1}, 0), 'new-cgroup-oom-event')

    def test_missing_required_signals_fail_before_output_or_child(self):
        real_resources = adapter.resources
        for missing in ['memory.max', 'memory.current', 'memory.stat', 'memory.events']:
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                group = self.signals(root, omit=missing)
                with patch.object(adapter, 'resources', side_effect=lambda _: real_resources(root, group)), patch.object(adapter.subprocess, 'Popen') as popen:
                    with self.assertRaises(FileNotFoundError):
                        adapter.Guard(root, root / 'out', 'build')
                    popen.assert_not_called()
                    self.assertFalse((root / 'out').exists())

    def test_required_signal_loss_during_monitoring_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            point = adapter.resources(root, self.signals(root)) | {'free': 32 * adapter.GIB}
            with patch.object(adapter, 'resources', side_effect=[point, FileNotFoundError('memory.current')]):
                guard = adapter.Guard(root, root / 'out', 'build').start()
                guard.thread.join(timeout=5)
                self.assertFalse(guard.thread.is_alive())
                with self.assertRaises(RuntimeError): guard.close()
            self.assertEqual(guard.failure['reason'], 'watchdog-observation-error')
            self.assertTrue((root / 'out/watchdog-stop.json').is_file())

    def test_malformed_required_signals_fail_closed(self):
        for name, value in [('memory.max', 'max'), ('memory.current', 'bad'),
                            ('memory.events', 'oom 0\n'), ('memory.stat', 'anon 0\n')]:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp); group=self.signals(root)
                (group/name).write_text(value)
                with self.assertRaises((KeyError, ValueError)):
                    adapter.resources(root, group)

    def test_available_psi_still_enforced_and_malformed_is_not_unavailable(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); group=self.signals(root, pressure='some avg10=0\nfull avg10=11.0\n')
            point=adapter.resources(root,group) | {'free':32*adapter.GIB}
            self.assertTrue(point['psi_available'])
            with patch.object(adapter,'resources',return_value=point):
                guard=adapter.Guard(root,root/'out','build')
            self.assertIsNone(guard.reason(point,0))
            self.assertEqual(guard.reason(point | {'monotonic':point['monotonic']+5.1},0),
                             'memory-full-avg10-at-least-10-percent-for-5s')
            (group/'memory.pressure').write_text('some avg10=0\n')
            with self.assertRaises(KeyError): adapter.resources(root,group)

if __name__ == '__main__': unittest.main()
