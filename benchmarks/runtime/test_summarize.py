import unittest
from run import PHASES
from summarize import lifecycle_totals


def process(ticks, writes):
    fields = ['0'] * 50
    fields[11], fields[12] = str(ticks), '0'
    return {'stat': '1 (fixture with spaces) ' + ' '.join(fields),
            'io': f'write_bytes: {writes}\nrchar: {writes * 2}\nwchar: {writes * 3}\nread_bytes: 0\ncancelled_write_bytes: 0\n',
            'smaps_rollup': None}


class LifecycleAccounting(unittest.TestCase):
    def test_deferred_post_turn_work_is_in_phase_and_whole_totals(self):
        points = [{'phase': name, 'elapsed_us': i * 100,
                   'process': process(i * 2, i * 4096)} for i, name in enumerate(PHASES)]
        totals = lifecycle_totals(points, 100)
        self.assertEqual(totals['phases']['final_writer_close']['write_bytes'], 4096)
        self.assertEqual(totals['phases']['final_writer_close']['cpu_us'], 20000)
        self.assertEqual(totals['product_lifecycle']['write_bytes'], 8 * 4096)
        self.assertEqual(totals['product_post_turn']['write_bytes'], 6 * 4096)
        self.assertEqual(sum(p['cpu_us'] for p in totals['phases'].values()),
                         totals['product_lifecycle']['cpu_us'])

    def test_provider_reap_io_is_separate_from_product_cleanup(self):
        points = [{'phase': name, 'elapsed_us': i * 100,
                   'process': process(i * 2, i * 4096)} for i, name in enumerate(PHASES)]
        points[-1]['process'] = process(18, 8 * 4096 + 299008)
        totals = lifecycle_totals(points, 100)
        self.assertEqual(totals['product_lifecycle']['write_bytes'], 8 * 4096)
        self.assertEqual(totals['product_post_turn']['write_bytes'], 6 * 4096)
        self.assertEqual(totals['product_lifecycle']['cpu_us'], 160000)
        self.assertEqual(totals['product_lifecycle']['wall_us'], 800)
        self.assertEqual(totals['product_post_turn']['wall_us'], 600)
        self.assertNotIn('provider_teardown', totals['phases'])
        teardown = totals['provider_teardown_including_reaped_child_io']
        self.assertEqual(teardown['write_bytes'], 299008)
        self.assertEqual(teardown['rchar'], 299008 * 2)
        self.assertEqual(teardown['wchar'], 299008 * 3)
        self.assertEqual(totals['product_lifecycle']['rchar'], 8 * 4096 * 2)
        self.assertEqual(totals['product_post_turn']['wchar'], 6 * 4096 * 3)
        self.assertEqual(teardown['cpu_us'], 20000)
        self.assertEqual(teardown['wall_us'], 100)
        # The complete window remains reconstructible without assigning the
        # reaped provider's lifetime I/O to product maintenance.
        self.assertEqual(totals['product_lifecycle']['write_bytes'] + teardown['write_bytes'],
                         8 * 4096 + 299008)

    def test_incomplete_window_cannot_be_labeled_whole(self):
        with self.assertRaises(AssertionError):
            lifecycle_totals([], 100)

    def test_unavailable_counters_are_null_not_zero(self):
        points = [{'phase': name, 'elapsed_us': i,
                   'process': dict(stat=None, io=None, smaps_rollup=None)} for i, name in enumerate(PHASES)]
        self.assertIsNone(lifecycle_totals(points, 100)['product_lifecycle']['cpu_us'])
        self.assertIsNone(lifecycle_totals(points, 100)['product_post_turn']['write_bytes'])


if __name__ == '__main__':
    unittest.main()
