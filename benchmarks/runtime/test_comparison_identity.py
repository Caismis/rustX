"""Reject accidental A/A before either product or provider is started."""
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class ComparisonIdentityTests(unittest.TestCase):
    def check_rejected(self, script, copied):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            before = root / "before"
            before.write_bytes(b"this executable must never be started\n")
            before.chmod(0o700)
            after = root / "after" if copied else before
            if copied:
                after.write_bytes(before.read_bytes())
                after.chmod(0o700)
            output = root / "results"
            result = subprocess.run(
                [sys.executable, str(ROOT / "benchmarks/runtime" / script),
                 "--before", str(before), "--after", str(after),
                 "--output", str(output), "--pairs", "3"],
                cwd=ROOT, capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIn("same executable; revision comparison is invalid", result.stderr)
            self.assertEqual(list(output.glob("*.json")), [])
            self.assertEqual(list(output.glob("*.log")), [])

    def test_runtime_same_path(self):
        self.check_rejected("compare-revisions.py", False)

    def test_runtime_identical_copy(self):
        self.check_rejected("compare-revisions.py", True)

    def test_append_same_path(self):
        self.check_rejected("compare-append.py", False)

    def test_append_identical_copy(self):
        self.check_rejected("compare-append.py", True)


if __name__ == "__main__":
    unittest.main()
