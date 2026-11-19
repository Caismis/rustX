#!/usr/bin/env python3
"""Unit and real-Cargo end-to-end tests for ``prepare_native_tests.py``."""

from __future__ import annotations

import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import prepare_native_tests as prep  # noqa: E402


REPO_ROOT = Path(__file__).resolve().parents[1]


def write_executable(path: Path, contents: bytes = b"#!/bin/sh\necho ok\n") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(contents)
    path.chmod(0o755)
    return path


def artifact_record(
    *,
    pkg_id: str,
    manifest_path: Path,
    name: str,
    kind: list[str],
    src_path: Path,
    executable: Path | None,
    profile_test: bool,
    features: list[str] | None = None,
    opt_level: str = "0",
    debuginfo: object = 2,
    debug_assertions: bool = True,
    overflow_checks: bool = True,
    filenames: list[str] | None = None,
    fresh: bool = False,
) -> dict:
    return {
        "reason": "compiler-artifact",
        "package_id": pkg_id,
        "manifest_path": str(manifest_path),
        "target": {
            "kind": kind,
            "name": name,
            "src_path": str(src_path),
        },
        "profile": {
            "opt_level": opt_level,
            "debuginfo": debuginfo,
            "debug_assertions": debug_assertions,
            "overflow_checks": overflow_checks,
            "test": profile_test,
        },
        "features": features or [],
        "filenames": filenames if filenames is not None else ([str(executable)] if executable else []),
        "executable": str(executable) if executable is not None else None,
        "fresh": fresh,
    }


def make_fixture_package(root: Path, *, toolchain_toml: str | None = None) -> None:
    if toolchain_toml:
        (root / "rust-toolchain.toml").write_text(toolchain_toml, encoding="utf-8")
    (root / "Cargo.toml").write_text(
        """[package]
name = "prep_fixture"
version = "0.1.0"
edition = "2024"

[workspace]

[features]
feat-a = []

[profile.test]
debug = "line-tables-only"
codegen-units = 256

[[bin]]
name = "companion"
path = "src/bin/companion.rs"
""",
        encoding="utf-8",
    )
    (root / "src/bin").mkdir(parents=True, exist_ok=True)
    (root / "src/lib.rs").write_text(
        """pub fn companion_output() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let sibling = exe
        .parent()
        .ok_or_else(|| "missing exe parent".to_owned())?
        .join(format!("companion{}", std::env::consts::EXE_SUFFIX));
    let out = std::process::Command::new(&sibling)
        .output()
        .map_err(|e| format!("failed to exec {}: {e}", sibling.display()))?;
    if !out.status.success() {
        return Err(format!("companion exited with {:?}", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn lib_runs_companion_sibling() {
        let output = crate::companion_output().expect("sibling companion runs");
        assert!(output.starts_with("COMPANION_OK:"), "{output}");
        println!("OBSERVED={output}");
    }
}
""",
        encoding="utf-8",
    )
    (root / "src/bin/companion.rs").write_text(
        """fn main() {
    let feat = if cfg!(feature = "feat-a") { "feat-a" } else { "default" };
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    println!("COMPANION_OK:{feat}:{profile}");
}
""",
        encoding="utf-8",
    )
    (root / "tests").mkdir(exist_ok=True)
    (root / "tests/integration.rs").write_text(
        """#[test]
fn integration_runs_companion_sibling() {
    let output = prep_fixture::companion_output().expect("sibling companion runs");
    assert!(output.starts_with("COMPANION_OK:"), "{output}");
    println!("OBSERVED={output}");
}
""",
        encoding="utf-8",
    )


class CommandParsingTests(unittest.TestCase):
    def test_rejects_missing_no_run_or_wrong_command(self) -> None:
        for argv in [
            ["prepare_native_tests.py", "cargo", "build", "--bins"],
            ["prepare_native_tests.py", "--", "cargo", "test", "--lib"],
            ["prepare_native_tests.py", "--", "cargo", "test", "--no-run", "--workspace"],
            [
                "prepare_native_tests.py",
                "--",
                "cargo",
                "test",
                "--no-run",
                "--message-format=short",
            ],
            ["prepare_native_tests.py", "--", "cargo", "test", "--no-run", "--doc"],
            ["prepare_native_tests.py", "--", "cargo", "test", "--no-run", "-j4"],
            ["prepare_native_tests.py", "--", "cargo", "test", "--no-run", "-Ffeat-a"],
            ["prepare_native_tests.py", "--", "cargo", "test", "--no-run", "--keep-going"],
        ]:
            with self.subTest(argv=argv):
                with self.assertRaises(prep.PreparationError):
                    prep.split_cargo_invocation(argv)

    def test_injects_test_profile_only_when_no_profile_selected(self) -> None:
        _, bin_default, _, _ = prep.split_cargo_invocation(
            ["prepare_native_tests.py", "--", "cargo", "test", "--lib", "--locked", "--no-run"]
        )
        self.assertEqual(
            bin_default,
            ["cargo", "build", "--bins", "--message-format=json", "--locked", "--profile=test"],
        )

        _, bin_explicit, _, _ = prep.split_cargo_invocation(
            [
                "prepare_native_tests.py",
                "--",
                "cargo",
                "test",
                "--profile",
                "dev",
                "--lib",
                "--no-run",
            ]
        )
        self.assertEqual(
            bin_explicit,
            ["cargo", "build", "--bins", "--message-format=json", "--profile", "dev"],
        )

    def test_forwards_build_context_flags_to_fallback_bin_build(self) -> None:
        test_cmd, bin_cmd, metadata_flags, pkg = prep.split_cargo_invocation(
            [
                "prepare_native_tests.py",
                "--",
                "cargo",
                "test",
                "--locked",
                "--release",
                "--all-features",
                "--target",
                "x86_64-unknown-linux-gnu",
                "--target-dir",
                "target/custom",
                "--lib",
                "--no-run",
                "--",
                "unit::",
            ]
        )
        self.assertEqual(pkg, None)
        self.assertEqual(metadata_flags, ["--locked"])
        self.assertIn("--message-format=json", test_cmd)
        self.assertEqual(
            bin_cmd,
            [
                "cargo",
                "build",
                "--bins",
                "--message-format=json",
                "--locked",
                "--release",
                "--all-features",
                "--target",
                "x86_64-unknown-linux-gnu",
                "--target-dir",
                "target/custom",
            ],
        )


class ValidationAndStagingTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.manifest = self.root / "Cargo.toml"
        self.manifest.write_text("[package]\nname='p'\n", encoding="utf-8")
        self.lib_src = self.root / "src/lib.rs"
        self.bin_src = self.root / "src/bin/comp.rs"
        self.test_src = self.root / "tests/int.rs"
        self.pkg_id = f"path+file://{self.root}#p@0.1.0"
        self.bin_targets = {"comp": self.bin_src}
        self.all_targets = {
            (("lib",), "p"): self.lib_src,
            (("bin",), "comp"): self.bin_src,
            (("test",), "int"): self.test_src,
        }

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_stages_binaries_and_replaces_stale_sibling(self) -> None:
        bin_exe = write_executable(self.root / "target/debug/comp", b"current-comp-bytes")
        lib_out = self.root / "target/debug/build/p/aaa/out"
        test_out = self.root / "target/debug/build/p/bbb/out"
        ordinary_lib = self.root / "target/debug/build/p/ccc/out/libp.rlib"
        ordinary_lib.parent.mkdir(parents=True, exist_ok=True)
        ordinary_lib.write_bytes(b"ordinary-lib")
        lib_exe = write_executable(lib_out / "p-aaa", b"lib-harness")
        test_exe = write_executable(test_out / "int-bbb", b"test-harness")
        stale = write_executable(test_out / "comp", b"stale-old-comp")

        records = [
            artifact_record(
                pkg_id=self.pkg_id,
                manifest_path=self.manifest,
                name="p",
                kind=["lib"],
                src_path=self.lib_src,
                executable=None,
                profile_test=False,
                filenames=[str(ordinary_lib)],
                fresh=True,
            ),
            artifact_record(
                pkg_id=self.pkg_id,
                manifest_path=self.manifest,
                name="comp",
                kind=["bin"],
                src_path=self.bin_src,
                executable=bin_exe,
                profile_test=False,
            ),
            artifact_record(
                pkg_id=self.pkg_id,
                manifest_path=self.manifest,
                name="p",
                kind=["lib"],
                src_path=self.lib_src,
                executable=lib_exe,
                profile_test=True,
            ),
            artifact_record(
                pkg_id=self.pkg_id,
                manifest_path=self.manifest,
                name="int",
                kind=["test"],
                src_path=self.test_src,
                executable=test_exe,
                profile_test=True,
            ),
            {"reason": "build-finished", "success": True},
        ]

        with (
            mock.patch.object(
                prep,
                "load_package_metadata",
                return_value=(
                    self.pkg_id,
                    "p",
                    self.manifest,
                    self.bin_targets,
                    self.all_targets,
                ),
            ),
            mock.patch.object(prep, "run_cargo_json", return_value=records),
        ):
            bins, harnesses = prep.prepare(
                ["prepare_native_tests.py", "--", "cargo", "test", "--no-run", "--lib", "--test", "int"],
                cwd=self.root,
            )

        self.assertEqual(bins, {"comp": bin_exe})
        self.assertEqual(harnesses, [lib_exe, test_exe])
        self.assertEqual((lib_out / "comp").read_bytes(), b"current-comp-bytes")
        self.assertEqual(stale.read_bytes(), b"current-comp-bytes")

    def test_rejects_foreign_checkout_incomplete_bins_and_mismatches(self) -> None:
        bin_exe = write_executable(self.root / "target/debug/comp")
        test_exe = write_executable(self.root / "target/debug/build/p/bbb/out/int-bbb")

        # 1. Wrong src_path checkout
        bad_src = [
            artifact_record(
                pkg_id=self.pkg_id,
                manifest_path=self.manifest,
                name="comp",
                kind=["bin"],
                src_path=Path("/other/checkout/src/bin/comp.rs"),
                executable=bin_exe,
                profile_test=False,
            ),
            {"reason": "build-finished", "success": True},
        ]
        with self.assertRaises(prep.PreparationError):
            prep.collect_package_artifacts(bad_src, self.pkg_id, self.manifest, self.all_targets)

        # 2. Incomplete binary set & missing ordinary library
        libs, bins, harnesses = prep.collect_package_artifacts(
            [
                artifact_record(
                    pkg_id=self.pkg_id,
                    manifest_path=self.manifest,
                    name="int",
                    kind=["test"],
                    src_path=self.test_src,
                    executable=test_exe,
                    profile_test=True,
                )
            ],
            self.pkg_id,
            self.manifest,
            self.all_targets,
        )
        self.assertEqual(libs, [])
        with self.assertRaises(prep.PreparationError):
            prep.validate_coherence({"comp": self.bin_src, "other": self.bin_src}, bins, harnesses)

        # 3. Feature mismatch
        bin_row = artifact_record(
            pkg_id=self.pkg_id,
            manifest_path=self.manifest,
            name="comp",
            kind=["bin"],
            src_path=self.bin_src,
            executable=bin_exe,
            profile_test=False,
            debuginfo="line-tables-only",
            features=["f1"],
        )
        test_row = artifact_record(
            pkg_id=self.pkg_id,
            manifest_path=self.manifest,
            name="int",
            kind=["test"],
            src_path=self.test_src,
            executable=test_exe,
            profile_test=True,
            debuginfo="line-tables-only",
            features=["f2"],
        )
        with self.assertRaises(prep.PreparationError):
            prep.validate_coherence(self.bin_targets, {"comp": bin_row}, [test_row])

        # 4. Profile & debuginfo mismatch (including ordinary lib vs bin vs test)
        release_test_row = artifact_record(
            pkg_id=self.pkg_id,
            manifest_path=self.manifest,
            name="int",
            kind=["test"],
            src_path=self.test_src,
            executable=test_exe,
            profile_test=True,
            opt_level="3",
            debug_assertions=False,
        )
        with self.assertRaises(prep.PreparationError):
            prep.validate_coherence(self.bin_targets, {"comp": bin_row}, [release_test_row])

        dev_bin_row = artifact_record(
            pkg_id=self.pkg_id,
            manifest_path=self.manifest,
            name="comp",
            kind=["bin"],
            src_path=self.bin_src,
            executable=bin_exe,
            profile_test=False,
            debuginfo=2,
        )
        line_tables_test_row = artifact_record(
            pkg_id=self.pkg_id,
            manifest_path=self.manifest,
            name="int",
            kind=["test"],
            src_path=self.test_src,
            executable=test_exe,
            profile_test=True,
            debuginfo="line-tables-only",
        )
        with self.assertRaises(prep.PreparationError):
            prep.validate_coherence(
                self.bin_targets, {"comp": dev_bin_row}, [line_tables_test_row]
            )
        with self.assertRaises(prep.PreparationError):
            prep.validate_coherence(
                self.bin_targets,
                {"comp": bin_row},
                [line_tables_test_row],
                libraries=[],
                has_lib=True,
            )

    def test_failed_cargo_run_stages_nothing(self) -> None:
        bin_exe = write_executable(self.root / "target/debug/comp")
        test_out = self.root / "target/debug/build/p/bbb/out"
        test_exe = write_executable(test_out / "int-bbb")
        emitted_before_error = json.dumps(
            artifact_record(
                pkg_id=self.pkg_id,
                manifest_path=self.manifest,
                name="comp",
                kind=["bin"],
                src_path=self.bin_src,
                executable=bin_exe,
                profile_test=False,
            )
        )
        emitted_diag = json.dumps(
            {
                "reason": "compiler-message",
                "package_id": self.pkg_id,
                "message": {"level": "error", "rendered": "error[E0425]: first-party fail\n"},
            }
        )
        upstream_diag = json.dumps(
            {
                "reason": "compiler-message",
                "package_id": "registry+https://example.invalid#upstream@0.1.0",
                "message": {"level": "warning", "rendered": "warning: upstream context note\n"},
            }
        )
        emitted_test = json.dumps(
            artifact_record(
                pkg_id=self.pkg_id,
                manifest_path=self.manifest,
                name="int",
                kind=["test"],
                src_path=self.test_src,
                executable=test_exe,
                profile_test=True,
            )
        )
        fake_proc = mock.Mock(
            returncode=101,
            stdout=f"{emitted_before_error}\n{emitted_diag}\n{upstream_diag}\n{emitted_test}\n"
            + json.dumps({"reason": "build-finished", "success": False}),
        )
        stderr_buf = io.StringIO()
        with (
            mock.patch.object(subprocess, "run", return_value=fake_proc),
            mock.patch.object(sys, "stderr", stderr_buf),
        ):
            with self.assertRaises(prep.PreparationError):
                prep.run_cargo_json(["cargo", "test", "--no-run"], self.root)
        self.assertIn("error[E0425]: first-party fail", stderr_buf.getvalue())
        self.assertIn("warning: upstream context note", stderr_buf.getvalue())
        self.assertFalse((test_out / "comp").exists())

    def test_relays_upstream_and_first_party_diagnostics_on_success(self) -> None:
        upstream_warn = json.dumps(
            {
                "reason": "compiler-message",
                "package_id": "registry+https://example.invalid#upstream@0.1.0",
                "message": {"level": "warning", "rendered": "warning: upstream dep unused\n"},
            }
        )
        first_party_warn = json.dumps(
            {
                "reason": "compiler-message",
                "package_id": self.pkg_id,
                "message": {"level": "warning", "rendered": "warning: first-party unused\n"},
            }
        )
        empty_rendered = json.dumps(
            {
                "reason": "compiler-message",
                "package_id": "registry+https://example.invalid#upstream@0.1.0",
                "message": {"level": "note", "rendered": ""},
            }
        )
        fake_proc = mock.Mock(
            returncode=0,
            stdout=f"{upstream_warn}\n{first_party_warn}\n{empty_rendered}\n"
            + json.dumps({"reason": "build-finished", "success": True}),
        )
        stderr_buf = io.StringIO()
        with (
            mock.patch.object(subprocess, "run", return_value=fake_proc),
            mock.patch.object(sys, "stderr", stderr_buf),
        ):
            records = prep.run_cargo_json(["cargo", "test", "--no-run"], self.root)
        self.assertEqual(len(records), 4)
        self.assertEqual(
            stderr_buf.getvalue(),
            "warning: upstream dep unused\nwarning: first-party unused\n",
        )


class RealCargoIntegrationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        toolchain = REPO_ROOT / "rust-toolchain.toml"
        make_fixture_package(
            self.root,
            toolchain_toml=toolchain.read_text(encoding="utf-8") if toolchain.exists() else None,
        )
        rustc_vv = subprocess.check_output(["rustc", "-vV"], cwd=REPO_ROOT, text=True)
        host_line = next(line for line in rustc_vv.splitlines() if line.startswith("host: "))
        self.host_triple = host_line.split(": ", 1)[1].strip()

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_custom_target_build_dir_explicit_target_and_features_end_to_end(self) -> None:
        (self.root / ".cargo").mkdir()
        custom_target = self.root / "custom-target"
        custom_build = self.root / "custom-build-dir"
        (self.root / ".cargo/config.toml").write_text(
            f'[build]\nbuild-dir = "{custom_build}"\n',
            encoding="utf-8",
        )
        env = os.environ | {"CARGO_TARGET_DIR": str(custom_target)}
        base_args = [
            "cargo",
            "test",
            "--test",
            "integration",
            "--features",
            "feat-a",
            "--target",
            self.host_triple,
        ]

        # 1. Without preparation, Cargo builds under custom-build-dir without
        # staging `companion` beside the harness, so executing fails clearly.
        subprocess.run([*base_args, "--no-run"], cwd=self.root, env=env, check=True)
        unprepared = subprocess.run(
            [*base_args, "--", "--nocapture"],
            cwd=self.root,
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(unprepared.returncode, 0)
        self.assertIn("failed to exec", unprepared.stdout + unprepared.stderr)

        # 2. With `prepare_native_tests.py`, the exact feature/target binary is
        # staged beside the emitted harness and executes cleanly.
        with mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": str(custom_target)}):
            binaries, harnesses = prep.prepare(
                ["prepare_native_tests.py", "--", *base_args, "--no-run"],
                cwd=self.root,
            )
        self.assertIn("companion", binaries)
        self.assertTrue(str(binaries["companion"]).startswith(str(custom_target)))
        self.assertTrue(str(harnesses[0]).startswith(str(custom_build)))
        prepared = subprocess.run(
            [*base_args, "--", "--nocapture"],
            cwd=self.root,
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(prepared.returncode, 0, prepared.stdout + prepared.stderr)
        self.assertIn("OBSERVED=COMPANION_OK:feat-a:debug", prepared.stdout)

    def test_lib_only_release_profile_builds_matching_bins_and_executes(self) -> None:
        custom_target = self.root / "rel-target"
        base_args = [
            "cargo",
            "test",
            "--lib",
            "--release",
            "--target-dir",
            str(custom_target),
        ]
        subprocess.run([*base_args, "--no-run"], cwd=self.root, check=True)
        unprepared = subprocess.run(
            [*base_args, "--", "--nocapture"],
            cwd=self.root,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(unprepared.returncode, 0)

        binaries, harnesses = prep.prepare(
            ["prepare_native_tests.py", "--", *base_args, "--no-run"],
            cwd=self.root,
        )
        self.assertIn("companion", binaries)
        self.assertEqual(len(harnesses), 1)
        prepared = subprocess.run(
            [*base_args, "--", "--nocapture"],
            cwd=self.root,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(prepared.returncode, 0, prepared.stdout + prepared.stderr)
        self.assertIn("OBSERVED=COMPANION_OK:default:release", prepared.stdout)

    def test_lib_only_default_test_profile_injects_test_profile_and_reuses_fresh_lib(self) -> None:
        custom_target = self.root / "test-profile-target"
        base_args = [
            "cargo",
            "test",
            "--lib",
            "--target-dir",
            str(custom_target),
        ]
        binaries, harnesses = prep.prepare(
            ["prepare_native_tests.py", "--", *base_args, "--no-run"],
            cwd=self.root,
        )
        self.assertIn("companion", binaries)
        self.assertEqual(len(harnesses), 1)
        prepared = subprocess.run(
            [*base_args, "--", "--nocapture"],
            cwd=self.root,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(prepared.returncode, 0, prepared.stdout + prepared.stderr)
        self.assertIn("OBSERVED=COMPANION_OK:default:debug", prepared.stdout)

        # Second invocation sees fresh=True on ordinary library without rebuilding.
        bin_records = prep.run_cargo_json(
            ["cargo", "build", "--bins", "--profile=test", "--message-format=json", "--target-dir", str(custom_target)],
            self.root,
        )
        lib_rows = [
            r
            for r in bin_records
            if r.get("reason") == "compiler-artifact"
            and r.get("target", {}).get("name") == "prep_fixture"
            and r.get("profile", {}).get("test") is False
        ]
        self.assertEqual(len(lib_rows), 1)
        self.assertIs(lib_rows[0]["fresh"], True)
        self.assertEqual(lib_rows[0]["profile"]["debuginfo"], "line-tables-only")

    def test_real_cargo_relays_upstream_warning_on_success_and_error_on_failure(self) -> None:
        with tempfile.TemporaryDirectory() as upstream_tmp:
            upstream_dir = Path(upstream_tmp).resolve()
            (upstream_dir / "src").mkdir(parents=True)
            (upstream_dir / "Cargo.toml").write_text(
                """[package]
name = "upstream_dep"
version = "0.1.0"
edition = "2024"
""",
                encoding="utf-8",
            )
            (upstream_dir / "src/lib.rs").write_text(
                """pub fn ok() -> &'static str {
    "ok"
}

fn upstream_unused_warning_witness() {}
""",
                encoding="utf-8",
            )
            cargo_toml = self.root / "Cargo.toml"
            cargo_toml.write_text(
                cargo_toml.read_text(encoding="utf-8")
                + f'\n[dependencies]\nupstream_dep = {{ path = "{upstream_dir}" }}\n',
                encoding="utf-8",
            )
            lib_rs = self.root / "src/lib.rs"
            lib_rs.write_text(
                lib_rs.read_text(encoding="utf-8")
                + "\npub fn use_upstream() -> &'static str {\n    upstream_dep::ok()\n}\n",
                encoding="utf-8",
            )

            custom_target = self.root / "diag-target"
            cmd = [
                "prepare_native_tests.py",
                "--",
                "cargo",
                "test",
                "--lib",
                "--target-dir",
                str(custom_target),
                "--no-run",
            ]
            stderr_ok = io.StringIO()
            with mock.patch.object(sys, "stderr", stderr_ok):
                binaries, harnesses = prep.prepare(cmd, cwd=self.root)
            self.assertIn("companion", binaries)
            self.assertEqual(len(harnesses), 1)
            self.assertIn("upstream_unused_warning_witness", stderr_ok.getvalue())

            # A failing compilation relays the compiler error and fails preparation.
            (upstream_dir / "src/lib.rs").write_text(
                "pub fn ok() -> &'static str { missing_symbol_in_upstream }\n",
                encoding="utf-8",
            )
            stderr_fail = io.StringIO()
            with mock.patch.object(sys, "stderr", stderr_fail):
                with self.assertRaises(prep.PreparationError):
                    prep.prepare(cmd, cwd=self.root)
            self.assertIn("missing_symbol_in_upstream", stderr_fail.getvalue())


class ClientBinCommandTests(unittest.TestCase):
    def test_replaces_package_and_drops_root_feature_selectors(self) -> None:
        cmd = prep.client_bin_command(
            [
                "cargo",
                "build",
                "--bins",
                "--message-format=json",
                "-p",
                "rustx",
                "--all-features",
                "--features",
                "prod",
                "-F",
                "extra",
                "--features=inline",
                "--no-default-features",
                "--locked",
                "--target",
                "x86_64-unknown-linux-gnu",
                "--target-dir",
                "target/custom",
                "--profile=test",
            ]
        )
        self.assertEqual(
            cmd,
            [
                "cargo",
                "build",
                "--bins",
                "--message-format=json",
                "-p",
                "rustx-execution-client",
                "--locked",
                "--target",
                "x86_64-unknown-linux-gnu",
                "--target-dir",
                "target/custom",
                "--profile=test",
            ],
        )

    def test_appends_package_selection_when_absent(self) -> None:
        cmd = prep.client_bin_command(
            ["cargo", "build", "--bins", "--message-format=json", "--locked"]
        )
        self.assertEqual(
            cmd,
            [
                "cargo",
                "build",
                "--bins",
                "--message-format=json",
                "--locked",
                "-p",
                "rustx-execution-client",
            ],
        )

    def test_rewrites_equals_spelled_package(self) -> None:
        cmd = prep.client_bin_command(
            ["cargo", "build", "--bins", "--package=rustx", "--locked"]
        )
        self.assertEqual(
            cmd,
            ["cargo", "build", "--bins", "--package=rustx-execution-client", "--locked"],
        )


if __name__ == "__main__":
    unittest.main()
