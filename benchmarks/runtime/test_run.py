"""Acceptance regressions: successful process exit must never imply a sample."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
import run


class MeasurementAcceptance(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name) / "sample.json"
        self.sample = {"case": "cancel", "measurement_version": 2, "oracle_version": 2,
                       "oracle": "passed", "schema": 69, "protocol": 1,
                       "initial_process": dict(stat=None, io=None, smaps_rollup=None),
                       "before_close": [], "closed_footprint": [], "checkpoint_footprint": [], "history": 0, "measured": 1, "requests": 1,
                       "verified_live_requests": 1, "verified_reopened_requests": 1,
                       "samples": [{"measured": True, "turn": 0, "settled_us": 1, "accepted_us": 0, "first_consumed_publication_us": None, "notification_records": 1,
                                    "footprint": [], "process": dict(stat=None, io=None, smaps_rollup=None)}],
                       "lifecycle": [{"phase": p, "elapsed_us": i,
                                      "process": dict(stat=None, io=None, smaps_rollup=None)}
                                     for i, p in enumerate(run.PHASES)]}
        self.sample.update({key: 1 for key in ("open_us", "shutdown_us", "close_us",
                                              "final_writer_close_us", "reopen_us", "cleanup_us")})
        self.log = f"test {run.TEST} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;\n"
        self.write(self.sample)

    def write(self, sample):
        self.path.write_text(json.dumps(sample))

    def validate(self, log=None):
        return run.validate_measurement(0, self.log if log is None else log, self.path, "cancel")

    def test_valid_single_measurement_is_accepted(self):
        self.assertEqual(self.validate(), [])

    def test_exit_zero_with_zero_matching_tests_is_rejected(self):
        self.assertTrue(self.validate("running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored;\n"))

    def test_exit_zero_with_missing_sample_is_rejected(self):
        self.path.unlink()
        self.assertTrue(self.validate())

    def test_exit_zero_with_invalid_json_is_rejected(self):
        self.path.write_text("not JSON")
        self.assertTrue(self.validate())

    def test_exit_zero_with_invalid_or_incomplete_oracle_is_rejected(self):
        for key, value in [("oracle", "failed"), ("oracle_version", 1),
                           ("verified_reopened_requests", 0), ("lifecycle", []),
                           ("samples", []), ("case", "tool")]:
            with self.subTest(key=key):
                sample = copy.deepcopy(self.sample)
                sample[key] = value
                self.write(sample)
                self.assertTrue(self.validate())


if __name__ == "__main__":
    unittest.main()
