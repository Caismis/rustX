import unittest
from run import PHASES
from summarize import lifecycle_totals


def process(ticks, writes):
    fields = ['0'] * 50
    fields[11], fields[12] = str(ticks), '0'
    return {'stat': '1 (fixture with spaces) ' + ' '.join(fields),
            'io': f'write_bytes: {writes}\nread_bytes: 0\ncancelled_write_bytes: 0\n',
            'smaps_rollup': None}


class LifecycleAccounting(unittest.TestCase):
    def test_deferred_post_turn_work_is_in_phase_and_whole_totals(self):
        points = [{'phase': name, 'elapsed_us': i * 100,
                   'process': process(i * 2, i * 4096)} for i, name in enumerate(PHASES)]
        totals = lifecycle_totals(points, 100)
        self.assertEqual(totals['phases']['final_writer_close']['write_bytes'], 4096)
        self.assertEqual(totals['phases']['final_writer_close']['cpu_us'], 20000)
        self.assertEqual(totals['whole_measured_lifecycle']['write_bytes'], 9 * 4096)
        self.assertEqual(totals['post_turn']['write_bytes'], 7 * 4096)
        self.assertEqual(sum(p['cpu_us'] for p in totals['phases'].values()),
                         totals['whole_measured_lifecycle']['cpu_us'])

    def test_incomplete_window_cannot_be_labeled_whole(self):
        with self.assertRaises(AssertionError):
            lifecycle_totals([], 100)

    def test_unavailable_counters_are_null_not_zero(self):
        points = [{'phase': name, 'elapsed_us': i,
                   'process': dict(stat=None, io=None, smaps_rollup=None)} for i, name in enumerate(PHASES)]
        self.assertIsNone(lifecycle_totals(points, 100)['whole_measured_lifecycle']['cpu_us'])
        self.assertIsNone(lifecycle_totals(points, 100)['post_turn']['write_bytes'])


if __name__ == '__main__':
    unittest.main()
