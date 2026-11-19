#!/usr/bin/env python3
"""Prepare native test harnesses using Cargo compiler-artifact JSON.

Runs a compile-only ``cargo test --no-run`` invocation with
``--message-format=json``, validates build completion and checkout/feature/
profile coherence, builds the package binaries with matching flags when the
test selector omitted integration tests (for example ``--lib`` alone), and
physically stages each current package binary beside every emitted test harness
executable so tests and production share one executable-sibling contract.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys


VALUE_FLAGS_FOR_METADATA = ("--manifest-path",)
BOOL_FLAGS_FOR_METADATA = ("--locked", "--frozen", "--offline")
TEST_TARGET_BOOL_FLAGS = (
    "--lib",
    "--bins",
    "--examples",
    "--tests",
    "--benches",
    "--all-targets",
    "--no-fail-fast",
    "--timings",
)
TEST_TARGET_VALUE_FLAGS = ("--bin", "--example", "--test", "--bench")
BUILD_VALUE_FLAGS = (
    "--manifest-path",
    "-p",
    "--package",
    "--target",
    "--target-dir",
    "--profile",
    "--features",
    "-F",
    "--config",
    "-j",
    "--jobs",
    "--color",
)
BUILD_BOOL_FLAGS = (
    "--locked",
    "--frozen",
    "--offline",
    "--release",
    "-r",
    "--all-features",
    "--no-default-features",
    "-q",
    "--quiet",
    "-v",
    "--verbose",
)
PROFILE_SELECTING_FLAGS = ("--release", "-r", "--profile")


class PreparationError(RuntimeError):
    """Raised when native test preparation fails closed."""


def sha256_file(path: Path) -> str:
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def split_cargo_invocation(
    argv: list[str],
) -> tuple[list[str], list[str], list[str], str | None]:
    """Parse ``[--] cargo test ...`` and return commands + metadata flags + package."""
    args = argv[1:]
    if "--" in args:
        idx = args.index("--")
        if idx != 0:
            raise PreparationError(
                "usage: prepare_native_tests.py -- cargo test <args...>"
            )
        cmd = args[1:]
    else:
        cmd = args
    if len(cmd) < 2 or Path(cmd[0]).stem.lower() != "cargo" or cmd[1] != "test":
        raise PreparationError(
            "expected a `cargo test ... --no-run` command after `--`"
        )

    raw_args = cmd[2:]
    cargo_part = raw_args[: raw_args.index("--")] if "--" in raw_args else raw_args
    test_tail = raw_args[raw_args.index("--") :] if "--" in raw_args else []

    if "--workspace" in cargo_part or "--all" in cargo_part:
        raise PreparationError(
            "`--workspace`/`--all` is not supported; prepare a single package"
        )
    if "--no-run" not in cargo_part:
        raise PreparationError("native test preparation requires `--no-run`")

    cargo_test_args: list[str] = []
    saw_json_format = False
    saw_profile = False
    bin_flags: list[str] = []
    metadata_flags: list[str] = []
    package_name: str | None = None
    i = 0
    while i < len(cargo_part):
        token = cargo_part[i]
        if token == "--doc":
            raise PreparationError("`--doc` is not supported by native harness staging")
        if token == "--message-format":
            if i + 1 >= len(cargo_part):
                raise PreparationError("`--message-format` requires a value")
            fmt = cargo_part[i + 1]
            if fmt != "json":
                raise PreparationError(
                    f"conflicting `{token} {fmt}`; native preparation requires JSON messages"
                )
            saw_json_format = True
            cargo_test_args.extend([token, fmt])
            i += 2
            continue
        if token.startswith("--message-format="):
            fmt = token.split("=", 1)[1]
            if fmt != "json":
                raise PreparationError(
                    f"conflicting `{token}`; native preparation requires JSON messages"
                )
            saw_json_format = True
            cargo_test_args.append(token)
            i += 1
            continue

        cargo_test_args.append(token)
        matched_value = False
        for flag in BUILD_VALUE_FLAGS:
            if token == flag:
                if i + 1 >= len(cargo_part):
                    raise PreparationError(f"`{flag}` requires a value")
                value = cargo_part[i + 1]
                cargo_test_args.append(value)
                bin_flags.extend([flag, value])
                if flag in PROFILE_SELECTING_FLAGS:
                    saw_profile = True
                if flag in VALUE_FLAGS_FOR_METADATA:
                    metadata_flags.extend([flag, value])
                if flag in ("-p", "--package"):
                    package_name = value
                i += 2
                matched_value = True
                break
            if flag.startswith("--") and token.startswith(f"{flag}="):
                value = token.split("=", 1)[1]
                bin_flags.append(token)
                if flag in PROFILE_SELECTING_FLAGS:
                    saw_profile = True
                if flag in VALUE_FLAGS_FOR_METADATA:
                    metadata_flags.append(token)
                if flag == "--package":
                    package_name = value
                i += 1
                matched_value = True
                break
        if matched_value:
            continue

        if token in BUILD_BOOL_FLAGS or token.startswith("-vv") and set(token[1:]) == {"v"}:
            bin_flags.append(token)
            if token in PROFILE_SELECTING_FLAGS:
                saw_profile = True
            if token in BOOL_FLAGS_FOR_METADATA:
                metadata_flags.append(token)
            i += 1
            continue

        matched_test_value = False
        for flag in TEST_TARGET_VALUE_FLAGS:
            if token == flag:
                if i + 1 >= len(cargo_part):
                    raise PreparationError(f"`{flag}` requires a value")
                cargo_test_args.append(cargo_part[i + 1])
                i += 2
                matched_test_value = True
                break
            if token.startswith(f"{flag}="):
                i += 1
                matched_test_value = True
                break
        if matched_test_value:
            continue

        if (
            token == "--no-run"
            or token in TEST_TARGET_BOOL_FLAGS
            or token.startswith("--timings=")
        ):
            i += 1
            continue

        if token.startswith("-"):
            raise PreparationError(
                f"unsupported Cargo flag spelling `{token}`; use separated standard flags"
            )
        i += 1

    if not saw_json_format:
        cargo_test_args.append("--message-format=json")
    if not saw_profile:
        # `cargo test` runs under `[profile.test]` (`debug = "line-tables-only"`,
        # `codegen-units = 256` here), whereas plain `cargo build --bins`
        # defaults to `[profile.dev]` and mismatches debuginfo/rebuilds deps.
        bin_flags.append("--profile=test")

    test_cmd = [cmd[0], "test", *cargo_test_args, *test_tail]
    bin_cmd = [cmd[0], "build", "--bins", "--message-format=json", *bin_flags]
    return test_cmd, bin_cmd, metadata_flags, package_name


def load_package_metadata(
    cargo_exe: str,
    metadata_flags: list[str],
    package_name: str | None,
    cwd: Path,
) -> tuple[str, str, Path, dict[str, Path], dict[tuple[tuple[str, ...], str], Path]]:
    """Query ``cargo metadata`` for exact checkout package identity and targets."""
    cmd = [cargo_exe, "metadata", "--no-deps", "--format-version", "1", *metadata_flags]
    result = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise PreparationError(
            f"`{' '.join(cmd)}` failed with exit {result.returncode}:\n{result.stderr.strip()}"
        )
    try:
        metadata = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise PreparationError(f"malformed `cargo metadata` output: {exc}") from exc

    packages = metadata.get("packages") or []
    if package_name is not None:
        matches = [pkg for pkg in packages if pkg.get("name") == package_name]
        if len(matches) != 1:
            raise PreparationError(
                f"expected one package named `{package_name}` in metadata, found {len(matches)}"
            )
        pkg = matches[0]
    elif len(packages) == 1:
        pkg = packages[0]
    else:
        # The workspace carries execution-boundary member crates; without an
        # explicit selector this harness prepares exactly the root package —
        # the one whose manifest is the workspace root's. (`check_test_lanes`
        # covers every workspace member; this preparer only stages the
        # binaries the root package's real-process tests spawn.)
        root_manifest = (Path(metadata["workspace_root"]) / "Cargo.toml").resolve()
        roots = [
            pkg
            for pkg in packages
            if Path(pkg["manifest_path"]).resolve() == root_manifest
        ]
        if len(roots) != 1:
            raise PreparationError(
                f"expected one root package in workspace metadata (pass `-p`), found {len(roots)}"
            )
        pkg = roots[0]

    pkg_id = pkg["id"]
    pkg_name = pkg["name"]
    manifest_path = Path(pkg["manifest_path"]).resolve()
    bin_targets: dict[str, Path] = {}
    all_targets: dict[tuple[tuple[str, ...], str], Path] = {}
    for target in pkg.get("targets", []):
        kind = tuple(target.get("kind", []))
        name = target["name"]
        src_path = Path(target["src_path"]).resolve()
        all_targets[(kind, name)] = src_path
        if "bin" in kind:
            bin_targets[name] = src_path
    return pkg_id, pkg_name, manifest_path, bin_targets, all_targets


def _relay_diagnostic(row: dict) -> None:
    if row.get("reason") != "compiler-message":
        return
    msg = row.get("message")
    if not isinstance(msg, dict):
        return
    rendered = msg.get("rendered")
    if isinstance(rendered, str) and rendered:
        sys.stderr.write(rendered)
        sys.stderr.flush()


def run_cargo_json(cmd: list[str], cwd: Path) -> list[dict]:
    """Execute Cargo with ``--message-format=json``, relaying diagnostics cleanly."""
    proc = subprocess.run(cmd, cwd=cwd, stdout=subprocess.PIPE, stderr=None, text=True, check=False)
    records: list[dict] = []
    for line_no, raw in enumerate(proc.stdout.splitlines(), start=1):
        stripped = raw.strip()
        if not stripped:
            continue
        try:
            row = json.loads(stripped)
        except json.JSONDecodeError as exc:
            if proc.returncode != 0:
                raise PreparationError(
                    f"`{' '.join(cmd)}` failed with exit {proc.returncode}"
                ) from exc
            raise PreparationError(
                f"`{' '.join(cmd)}` emitted invalid JSON on line {line_no}: {exc}"
            ) from exc
        records.append(row)
        _relay_diagnostic(row)

    if proc.returncode != 0:
        raise PreparationError(
            f"`{' '.join(cmd)}` failed with exit {proc.returncode}"
        )

    finished = [row for row in records if row.get("reason") == "build-finished"]
    if len(finished) != 1 or finished[0].get("success") is not True:
        raise PreparationError(
            f"`{' '.join(cmd)}` did not report a successful `build-finished` record"
        )
    return records


def collect_package_artifacts(
    records: list[dict],
    package_id: str,
    manifest_path: Path,
    all_targets: dict[tuple[tuple[str, ...], str], Path],
) -> tuple[list[dict], dict[str, dict], list[dict]]:
    """Extract and verify checkout identity of ordinary libs, binaries, and harnesses."""
    libraries: list[dict] = []
    binaries: dict[str, dict] = {}
    harnesses: list[dict] = []

    for row in records:
        if row.get("reason") != "compiler-artifact":
            continue
        if row.get("package_id") != package_id:
            continue

        row_manifest = row.get("manifest_path")
        if not row_manifest or Path(row_manifest).resolve() != manifest_path:
            raise PreparationError(
                f"artifact manifest mismatch: expected {manifest_path}, got {row_manifest!r}"
            )

        target = row.get("target") or {}
        kind = tuple(target.get("kind") or ())
        name = target.get("name")
        src_path_raw = target.get("src_path")
        expected_src = all_targets.get((kind, name))
        if expected_src is None or not src_path_raw:
            raise PreparationError(
                f"unexpected target artifact {kind!r} `{name}` with src_path {src_path_raw!r}"
            )
        if Path(src_path_raw).resolve() != expected_src:
            raise PreparationError(
                f"stale or foreign checkout artifact for `{name}`: "
                f"expected {expected_src}, got {src_path_raw}"
            )

        fresh = row.get("fresh")
        if not isinstance(fresh, bool):
            raise PreparationError(
                f"artifact `{name}` is missing boolean `fresh` metadata: {fresh!r}"
            )

        profile = row.get("profile") or {}
        if (kind == ("lib",) or "rlib" in kind) and profile.get("test") is False:
            filenames = [Path(p) for p in (row.get("filenames") or [])]
            if not filenames or not all(path.is_file() for path in filenames):
                raise PreparationError(
                    f"ordinary library artifact `{name}` has missing output files: {filenames!r}"
                )
            libraries.append(row)

        executable = row.get("executable")
        if executable is None:
            continue
        exe_path = Path(executable)
        if not exe_path.is_file():
            raise PreparationError(
                f"Cargo reported missing executable `{exe_path}` for `{name}`"
            )

        if profile.get("test") is True:
            harnesses.append(row)
        elif kind == ("bin",) and profile.get("test") is False:
            if name in binaries and Path(binaries[name]["executable"]).resolve() != exe_path.resolve():
                raise PreparationError(f"conflicting binary executables emitted for `{name}`")
            binaries[name] = row

    return libraries, binaries, harnesses


def profile_signature(row: dict) -> tuple[str | None, object, bool | None, bool | None]:
    profile = row.get("profile") or {}
    return (
        profile.get("opt_level"),
        profile.get("debuginfo"),
        profile.get("debug_assertions"),
        profile.get("overflow_checks"),
    )


def validate_coherence(
    bin_targets: dict[str, Path],
    binaries: dict[str, dict],
    harnesses: list[dict],
    libraries: list[dict] = (),
    has_lib: bool = False,
) -> None:
    """Ensure libs, binaries, and harnesses are complete and share features/profile."""
    if not harnesses:
        raise PreparationError("Cargo emitted no test harness executables")

    missing_bins = sorted(set(bin_targets) - set(binaries))
    if missing_bins:
        raise PreparationError(
            f"Cargo did not emit required package binary artifacts: {', '.join(missing_bins)}"
        )
    extra_bins = sorted(set(binaries) - set(bin_targets))
    if extra_bins:
        raise PreparationError(
            f"Cargo emitted unknown package binaries: {', '.join(extra_bins)}"
        )
    if has_lib and bin_targets and not libraries:
        raise PreparationError(
            "Cargo did not emit the root ordinary library artifact (`profile.test == false`)"
        )

    all_rows = [*libraries, *binaries.values(), *harnesses]
    expected_features = sorted(all_rows[0].get("features") or [])
    expected_profile = profile_signature(all_rows[0])
    for row in all_rows[1:]:
        features = sorted(row.get("features") or [])
        if features != expected_features:
            raise PreparationError(
                f"feature mismatch for `{row['target']['name']}`: "
                f"{features!r} != {expected_features!r}"
            )
        profile = profile_signature(row)
        if profile != expected_profile:
            raise PreparationError(
                f"profile mismatch for `{row['target']['name']}`: "
                f"{profile!r} != {expected_profile!r}"
            )


def stage_binary_beside(source: Path, dest_dir: Path) -> None:
    """Stage ``source`` into ``dest_dir/source.name`` atomically and verify it."""
    dest = dest_dir / source.name
    src_stat = source.stat()
    if dest.exists():
        dst_stat = dest.stat()
        if (src_stat.st_dev, src_stat.st_ino) == (dst_stat.st_dev, dst_stat.st_ino):
            if not os.access(dest, os.X_OK):
                raise PreparationError(f"staged sibling `{dest}` is not executable")
            return

    tmp = dest_dir / f".{source.name}.stage-{os.getpid()}"
    try:
        if tmp.exists() or tmp.is_symlink():
            tmp.unlink()
        linked = False
        try:
            os.link(source, tmp)
            linked = True
        except OSError:
            shutil.copy2(source, tmp)
        if not os.access(tmp, os.X_OK):
            os.chmod(tmp, stat.S_IMODE(src_stat.st_mode) | stat.S_IXUSR)
        os.replace(tmp, dest)
    except Exception:
        if tmp.exists() or tmp.is_symlink():
            tmp.unlink()
        raise

    if not dest.is_file() or not os.access(dest, os.X_OK):
        raise PreparationError(f"failed to stage executable sibling `{dest}`")
    if not linked and sha256_file(dest) != sha256_file(source):
        raise PreparationError(f"checksum mismatch after staging `{dest}` from `{source}`")


# Companion packages whose binaries the root test harnesses compose
# through the executable-sibling contract: the Execution Client the process
# tests drive, and the physical supervisor helpers
# (`rustx-process-supervisor`: `bash-supervisor`, `interactive-supervisor`).
COMPANION_PACKAGES = ("rustx-execution-client", "rustx-process-supervisor")


def companion_bin_command(bin_cmd: list[str], package: str) -> list[str]:
    """Derive a companion package's build from the root ``--bins`` command.

    Package selection becomes ``-p <package>``; the root
    crate's feature selectors are dropped so the companion builds in its own
    ordinary default mode, while target, target-dir, profile, and
    lock/offline flags carry over unchanged.
    """
    cmd: list[str] = []
    saw_package = False
    i = 0
    while i < len(bin_cmd):
        token = bin_cmd[i]
        if token in ("-p", "--package"):
            cmd.extend(["-p", package])
            saw_package = True
            i += 2
            continue
        if token.startswith("--package="):
            cmd.append(f"--package={package}")
            saw_package = True
            i += 1
            continue
        if token in ("--features", "-F"):
            i += 2
            continue
        if token.startswith("--features="):
            i += 1
            continue
        if token in ("--all-features", "--no-default-features"):
            i += 1
            continue
        cmd.append(token)
        i += 1
    if not saw_package:
        cmd.extend(["-p", package])
    return cmd


def stage_binaries(binaries: dict[str, dict], harnesses: list[dict]) -> int:
    """Stage every package binary beside every emitted test harness executable."""
    harness_exes = {Path(row["executable"]).resolve() for row in harnesses}
    harness_dirs = sorted({Path(row["executable"]).parent for row in harnesses})
    binary_paths = sorted((Path(row["executable"]) for row in binaries.values()), key=lambda p: p.name)

    staged = 0
    for directory in harness_dirs:
        if not directory.is_dir():
            raise PreparationError(f"harness directory does not exist: `{directory}`")
        for bin_path in binary_paths:
            dest = (directory / bin_path.name).resolve()
            if dest in harness_exes:
                raise PreparationError(
                    f"binary staging destination `{dest}` collides with a test harness executable"
                )
            stage_binary_beside(bin_path, directory)
            staged += 1
    return staged


def prepare(argv: list[str], cwd: Path | None = None) -> tuple[dict[str, Path], list[Path]]:
    work_dir = (cwd or Path.cwd()).resolve()
    test_cmd, bin_cmd, metadata_flags, package_name = split_cargo_invocation(argv)
    pkg_id, pkg_name, manifest_path, bin_targets, all_targets = load_package_metadata(
        test_cmd[0], metadata_flags, package_name, work_dir
    )
    has_lib = any(kind == ("lib",) or "rlib" in kind for kind, _ in all_targets)

    test_records = run_cargo_json(test_cmd, work_dir)
    libraries, binaries, harnesses = collect_package_artifacts(
        test_records, pkg_id, manifest_path, all_targets
    )
    if not harnesses:
        raise PreparationError(
            f"`{' '.join(test_cmd)}` emitted no test harness executables"
        )

    if bin_targets and not binaries:
        bin_records = run_cargo_json(bin_cmd, work_dir)
        bin_libraries, binaries, _ = collect_package_artifacts(
            bin_records, pkg_id, manifest_path, all_targets
        )
        libraries.extend(bin_libraries)

    validate_coherence(
        bin_targets,
        binaries,
        harnesses,
        libraries=libraries,
        has_lib=has_lib,
    )
    if pkg_name == "rustx":
        # The root package's tests compose companion-package binaries — the
        # Execution Client and the physical supervisor helpers; each trusted
        # sibling is built with the same target, target-dir, profile, and
        # lock flags — never the root crate's feature selectors — and staged
        # beside every emitted harness.
        root_dir = Path(next(iter(binaries.values()))["executable"]).resolve().parent
        root_profile = profile_signature(next(iter(binaries.values())))
        for companion in COMPANION_PACKAGES:
            companion_id, _, companion_manifest, companion_bin_targets, companion_targets = (
                load_package_metadata(
                    test_cmd[0], metadata_flags, companion, work_dir
                )
            )
            companion_records = run_cargo_json(
                companion_bin_command(bin_cmd, companion), work_dir
            )
            _, companion_binaries, _ = collect_package_artifacts(
                companion_records, companion_id, companion_manifest, companion_targets
            )
            # The companion package's own harnesses (for example the
            # process-supervisor lib tests that exec `bash-supervisor`)
            # resolve their helpers through the same executable-sibling
            # contract, so they are staging destinations too.
            _, _, companion_harnesses = collect_package_artifacts(
                test_records, companion_id, companion_manifest, companion_targets
            )
            harnesses.extend(companion_harnesses)
            missing_companion = sorted(
                set(companion_bin_targets) - set(companion_binaries)
            )
            if missing_companion:
                raise PreparationError(
                    f"Cargo did not emit required companion binary artifacts: {', '.join(missing_companion)}"
                )
            if not binaries:
                raise PreparationError(
                    "companion staging requires the ordinary package binaries it must sit beside"
                )
            for name, row in companion_binaries.items():
                if profile_signature(row) != root_profile:
                    raise PreparationError(
                        f"companion binary `{name}` profile differs from the root binaries'"
                    )
                if Path(row["executable"]).resolve().parent != root_dir:
                    raise PreparationError(
                        f"companion binary `{name}` targets a different directory than the root binaries"
                    )
            # Feature names are package-local and never compared across crates.
            binaries.update(companion_binaries)
    staged_count = stage_binaries(binaries, harnesses)
    harness_paths = [Path(row["executable"]) for row in harnesses]
    binary_paths = {name: Path(row["executable"]) for name, row in binaries.items()}
    print(
        f"prepared {len(binary_paths)} package binaries beside {len(harness_paths)} test harness(es) "
        f"({staged_count} sibling placements)",
        file=sys.stderr,
    )
    return binary_paths, harness_paths


def main() -> int:
    try:
        prepare(sys.argv)
    except PreparationError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
