#!/usr/bin/env python3
"""Deterministic regressions for test-support/rust_test_shards.py."""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
TOOL = HERE / "rust_test_shards.py"

FAKE_HARNESS = """#!/usr/bin/env python3
import sys
NAMES = ["alpha::one", "alpha::two", "beta::slow", "gamma::should_panic_test"]
args = sys.argv[1:]
names = []
skip_next = False
for a in args:
    if skip_next:
        skip_next = False
        continue
    if a == "--format":
        skip_next = True
        continue
    if not a.startswith("-"):
        names.append(a)
exact = "--exact" in args
if "--list" in args:
    if "--ignored" in args:
        print("zeta::ignored_test: test")
    else:
        # Stable libtest marks #[ignore]d tests identically in terse
        # --list output: the runnable set is all names minus --ignored.
        for n in NAMES + ["zeta::ignored_test"]:
            if not names or any(f in n for f in names):
                print(f"{n}: test")
    sys.exit(0)
selected = [n for n in NAMES if n in names] if exact else NAMES
for n in selected:
    mark = "FAILED" if n == "beta::slow" and "FAIL_SLOW" in __import__("os").environ else "ok"
    if n == "gamma::should_panic_test":
        # libtest's real marker for #[should_panic] tests.
        print(f"test {n} - should panic ... {mark}")
    else:
        print(f"test {n} ... {mark}")
sys.exit(0)
"""

WRITING_FAIL_HARNESS = FAKE_HARNESS.replace(
    'mark = "FAILED" if n == "beta::slow" and "FAIL_SLOW" in __import__("os").environ else "ok"',
    'mark = "FAILED" if n == "beta::slow" else "ok"',
)


def write_harness(directory: Path, name: str, body: str = FAKE_HARNESS) -> Path:
    path = directory / name
    path.write_text(body)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


class ShardToolCase(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="rts-"))
        self.prepared = self.tmp / "prepared"
        self.prepared.mkdir()
        write_harness(self.prepared, "lib-harness")
        self.spec = self.tmp / "spec.json"
        self.spec.write_text(json.dumps([
            {"name": "lib", "harness": "lib-harness", "libtest_args": []},
        ]))
        self.root = self.tmp / "repo"
        self.root.mkdir()
        (self.root / "Cargo.lock").write_text("lock")
        self.manifest = self.tmp / "manifest.json"
        self.manifest.write_text(json.dumps({
            "git_head": "current",
            "runner_os": __import__("platform").system(),
            "runner_arch": __import__("platform").machine(),
            "cargo_lock_sha256": self._lock_sha(),
            "features": "all-features",
            "profile": "test",
        }))
        self.inventory_dir = self.tmp / "inventory"

    def _lock_sha(self) -> str:
        import hashlib
        return hashlib.sha256(b"lock").hexdigest()

    def tool(self, *argv: str, cwd: Path | None = None) -> subprocess.CompletedProcess:
        # PATH without ~/.cargo/bin keeps rustc off the tool's PATH so the
        # manifest's optional toolchain checks are skipped deterministically.
        env = dict(os.environ, RUSTX_SOURCE_HEAD="current", PATH="/usr/bin:/bin")
        return subprocess.run(
            [sys.executable, str(TOOL), *argv],
            cwd=cwd or self.tmp,
            capture_output=True, text=True,
            env=env,
        )

    def make_inventory(self, shards: int = 2) -> dict:
        result = self.tool(
            "inventory",
            "--manifest", str(self.manifest),
            "--prepared", str(self.prepared),
            "--spec", str(self.spec),
            "--shards", str(shards),
            "--out", str(self.inventory_dir),
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads((self.inventory_dir / "inventory.json").read_text())

    def run_shard(self, shard: int, runs: Path) -> subprocess.CompletedProcess:
        return self.tool(
            "run",
            "--manifest", str(self.manifest),
            "--prepared", str(self.prepared),
            "--inventory", str(self.inventory_dir / "inventory.json"),
            "--spec", str(self.spec), "--features", "all-features",
            "--profile", "test", "--root", str(self.root),
            "--shard", str(shard),
            "--report", str(runs / f"shard-{shard}" / "report.json"),
        )

    def verify(self, runs: Path) -> subprocess.CompletedProcess:
        return self.tool(
            "verify",
            "--inventory", str(self.inventory_dir / "inventory.json"),
            "--runs", str(runs),
        )

    def test_happy_path_all_shards_cover_each_test_once(self) -> None:
        self.make_inventory(shards=2)
        runs = self.tmp / "runs"
        for shard in range(2):
            self.assertEqual(self.run_shard(shard, runs).returncode, 0)
        result = self.verify(runs)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("4 tests", result.stdout)

    def test_wrong_artifact_identity_is_rejected(self) -> None:
        self.make_inventory()
        manifest = json.loads(self.manifest.read_text())
        manifest["git_head"] = "another-commit"
        self.manifest.write_text(json.dumps(manifest))
        result = self.run_shard(0, self.tmp / "runs")
        self.assertEqual(result.returncode, 1)
        self.assertIn("different commit", result.stderr)

    def test_foreign_lockfile_is_rejected(self) -> None:
        self.make_inventory()
        (self.root / "Cargo.lock").write_text("changed")
        result = self.run_shard(0, self.tmp / "runs")
        self.assertEqual(result.returncode, 1)
        self.assertIn("Cargo.lock changed", result.stderr)

    def test_feature_config_mismatch_is_rejected(self) -> None:
        self.make_inventory()
        manifest = json.loads(self.manifest.read_text())
        manifest["features"] = "default-features"
        self.manifest.write_text(json.dumps(manifest))
        result = self.run_shard(0, self.tmp / "runs")
        self.assertEqual(result.returncode, 1)
        self.assertIn("different features", result.stderr)

    def test_cross_platform_artifact_is_rejected(self) -> None:
        # A manifest minted on macOS must be refused on Linux (and vv).
        self.make_inventory()
        manifest = json.loads(self.manifest.read_text())
        manifest["runner_os"] = (
            "Darwin" if __import__("platform").system() == "Linux" else "Linux"
        )
        self.manifest.write_text(json.dumps(manifest))
        result = self.run_shard(0, self.tmp / "runs")
        self.assertEqual(result.returncode, 1)
        self.assertIn("different OS", result.stderr)

    def test_package_selection_change_is_rejected(self) -> None:
        # A spec that names a harness absent from the packed artifact must
        # fail inventory discovery rather than silently covering nothing.
        self.spec.write_text(json.dumps([
            {"name": "lib", "harness": "lib-harness", "libtest_args": []},
            {"name": "removed", "harness": "test:removed-target",
             "libtest_args": []},
        ]))
        result = self.tool(
            "inventory",
            "--manifest", str(self.manifest),
            "--prepared", str(self.prepared),
            "--spec", str(self.spec),
            "--shards", "2",
            "--out", str(self.inventory_dir),
        )
        self.assertEqual(result.returncode, 1)

    def test_tampered_harness_is_rejected(self) -> None:
        self.make_inventory()
        write_harness(self.prepared, "lib-harness", WRITING_FAIL_HARNESS)
        result = self.run_shard(0, self.tmp / "runs")
        self.assertEqual(result.returncode, 1)
        self.assertIn("sha256 mismatch", result.stderr)

    def test_missing_shard_report_fails_verify(self) -> None:
        self.make_inventory(shards=2)
        runs = self.tmp / "runs"
        self.assertEqual(self.run_shard(0, runs).returncode, 0)
        result = self.verify(runs)
        self.assertEqual(result.returncode, 1)
        self.assertIn("no report from shard(s) ['1']", result.stderr)
        self.assertIn("test never executed", result.stderr)

    def test_duplicate_execution_fails_verify(self) -> None:
        self.make_inventory(shards=1)
        runs = self.tmp / "runs"
        self.run_shard(0, runs)
        report = json.loads((runs / "shard-0" / "report.json").read_text())
        report["results"]["lib-harness::alpha::one"] = "ok"
        report["shard"] = 1
        (runs / "shard-1").mkdir()
        (runs / "shard-1" / "report.json").write_text(json.dumps(report))
        result = self.verify(runs)
        self.assertEqual(result.returncode, 1)
        self.assertIn("executed 2 times", result.stderr)

    def test_incomplete_execution_fails_verify(self) -> None:
        self.make_inventory(shards=1)
        runs = self.tmp / "runs"
        self.run_shard(0, runs)
        report = json.loads((runs / "shard-0" / "report.json").read_text())
        del report["results"]["lib-harness::beta::slow"]
        (runs / "shard-0" / "report.json").write_text(json.dumps(report))
        result = self.verify(runs)
        self.assertEqual(result.returncode, 1)
        self.assertIn("never executed", result.stderr)

    def test_shard_failure_is_reported_and_recorded(self) -> None:
        self.make_inventory(shards=1)
        runs = self.tmp / "runs"
        env = dict(os.environ, FAIL_SLOW="1", RUSTX_SOURCE_HEAD="current",
                   PATH="/usr/bin:/bin")
        result = subprocess.run(
            [sys.executable, str(TOOL), "run",
             "--manifest", str(self.manifest),
             "--prepared", str(self.prepared),
             "--inventory", str(self.inventory_dir / "inventory.json"),
             "--spec", str(self.spec), "--features", "all-features",
            "--profile", "test", "--root", str(self.root),
             "--shard", "0",
             "--report", str(runs / "shard-0" / "report.json")],
            capture_output=True, text=True, env=env,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("FAILED lib-harness::beta::slow", result.stderr)
        verify = self.verify(runs)
        self.assertEqual(verify.returncode, 1)
        self.assertIn("beta::slow -> FAILED", verify.stderr)

    def test_run_rejects_unexecuted_test_names(self) -> None:
        self.make_inventory(shards=1)
        runs = self.tmp / "runs"
        report_path = runs / "shard-0" / "report.json"
        self.run_shard(0, runs)
        # The shard itself must fail when the harness drops a test: swap in
        # a harness that lists three but executes two.
        harness = self.prepared / "lib-harness"
        harness.write_text(harness.read_text().replace(
            'selected = [n for n in NAMES if n in names] if exact else NAMES',
            'selected = [n for n in NAMES if n in names and n != "alpha::two"]'
            ' if exact else NAMES',
        ))
        # Keep sha256 consistent by regenerating the inventory binding.
        self.inventory_dir = self.tmp / "inventory2"
        self.make_inventory(shards=1)
        result = self.tool(
            "run",
            "--manifest", str(self.manifest),
            "--prepared", str(self.prepared),
            "--inventory", str(self.inventory_dir / "inventory.json"),
            "--spec", str(self.spec), "--features", "all-features",
            "--profile", "test", "--root", str(self.root),
            "--shard", "0",
            "--report", str(report_path),
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("did not execute", result.stderr)

    def test_missing_harness_fails(self) -> None:
        self.make_inventory(shards=1)
        (self.prepared / "lib-harness").unlink()
        result = self.run_shard(0, self.tmp / "runs")
        self.assertEqual(result.returncode, 1)
        self.assertIn("missing harness", result.stderr)


if __name__ == "__main__":
    unittest.main()
