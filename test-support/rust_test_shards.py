#!/usr/bin/env python3
"""Build-once/test-many orchestration for native libtest harnesses.

Subcommands:

- ``manifest``: capture the build-side identity (git head, runner triple,
  toolchain, Cargo.lock digest, feature/profile args) that later consumers
  must match before trusting a prepared artifact.
- ``inventory``: list every runnable/ignored test of each prepared harness
  (applying the lane's libtest filters), split the runnable set across N
  shards by duration weight, and emit ``inventory.json`` plus
  ``shard-<i>.txt`` files.
- ``run``: on a shard runner, re-verify artifact identity, execute this
  shard's tests exactly by name, and write a machine-readable report.
- ``verify``: prove every runnable inventory test executed exactly once and
  passed across all shard reports.

The tool never compiles: every test that runs is a test the built artifact
itself reported under the lane's own filters.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import time
import sys
from pathlib import Path

TEST_LINE = re.compile(
    r"^test (\S+) \.\.\. (ok|FAILED|ignored|bench: .*)(?: in ([0-9.]+)s)?$"
)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git_head(root: Path) -> str:
    override = os.environ.get("RUSTX_SOURCE_HEAD") or os.environ.get("GITHUB_SHA")
    if override:
        return override.strip()
    return subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=root, check=True,
        capture_output=True, text=True,
    ).stdout.strip()


def build_manifest(root: Path, features: str, profile: str) -> dict:
    rustc = subprocess.run(
        ["rustc", "--version", "--verbose"], cwd=root, check=True,
        capture_output=True, text=True,
    ).stdout
    triple = next(
        (line.split(":", 1)[1].strip() for line in rustc.splitlines()
         if line.startswith("host:")),
        f"{platform.system()}-{platform.machine()}",
    )
    return {
        "git_head": git_head(root),
        "runner_os": platform.system(),
        "runner_arch": platform.machine(),
        "target_triple": triple,
        "rustc_version": rustc.splitlines()[0].strip(),
        "cargo_lock_sha256": sha256_file(root / "Cargo.lock"),
        "features": features,
        "profile": profile,
    }


def check_manifest(
    manifest: dict, root: Path,
    expected_features: str, expected_profile: str,
) -> list[str]:
    problems = []
    if manifest.get("git_head") != git_head(root):
        problems.append("artifact was built for a different commit")
    if manifest.get("runner_os") != platform.system():
        problems.append("artifact was built on a different OS")
    if manifest.get("runner_arch") != platform.machine():
        problems.append("artifact was built for a different architecture")
    if manifest.get("cargo_lock_sha256") != sha256_file(root / "Cargo.lock"):
        problems.append("Cargo.lock changed since the artifact was built")
    # Toolchain binding without invoking rustc (the rustup shim would
    # auto-install the pinned toolchain on a shard runner that ships only
    # `stable`): compare the manifest against the checkout's own pin in
    # rust-toolchain.toml and this runner's architecture.
    toolchain_file = root / "rust-toolchain.toml"
    channel = None
    if toolchain_file.is_file():
        match = re.search(
            r'channel\s*=\s*"([^"]+)"', toolchain_file.read_text()
        )
        channel = match.group(1) if match else None
    if channel and channel not in manifest.get("rustc_version", ""):
        problems.append(
            f"artifact toolchain does not match pinned channel {channel}"
        )
    machine_alias = {"arm64": "aarch64", "amd64": "x86_64"}.get(
        platform.machine().lower(), platform.machine().lower()
    )
    triple = str(manifest.get("target_triple", ""))
    if triple and not triple.lower().startswith(machine_alias):
        problems.append("artifact was built for a different target triple")
    if manifest.get("features") != expected_features:
        problems.append("artifact was built with different features")
    if manifest.get("profile") != expected_profile:
        problems.append("artifact was built with a different profile")
    return problems


def list_harness_tests(
    harness: Path, libtest_args: list[str], ignored: bool
) -> list[str]:
    command = [str(harness), *libtest_args, "--list", "--format", "terse"]
    if ignored:
        command.append("--ignored")
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(
            f"{' '.join(command)} failed ({result.returncode}): {result.stderr}"
        )
    return [
        line.split(": test", 1)[0]
        for line in result.stdout.splitlines()
        if line.endswith(": test")
    ]


def assign_shards(
    tests: list[tuple[str, float]], shard_count: int
) -> list[list[str]]:
    """Longest-processing-time assignment over (key, seconds) pairs."""
    loads = [0.0] * shard_count
    shards: list[list[str]] = [[] for _ in range(shard_count)]
    for key, seconds in sorted(tests, key=lambda item: (-item[1], item[0])):
        target = loads.index(min(loads))
        shards[target].append(key)
        loads[target] += seconds
    return shards


def load_weights(path: Path | None) -> dict[str, float]:
    if not path or not path.is_file():
        return {}
    weights = json.loads(path.read_text())
    if not isinstance(weights, dict):
        raise RuntimeError(f"weights file {path} is not a JSON object")
    return {str(key): float(value) for key, value in weights.items()}


def run_shard(
    prepared: Path,
    inventory: dict,
    shard_index: int,
    report_path: Path,
    batch_size: int = 400,
) -> int:
    shard_tests = inventory["shards"][shard_index]
    by_harness: dict[str, list[str]] = {}
    for key in shard_tests:
        harness, name = key.split("::", 1)
        by_harness.setdefault(harness, []).append(name)

    results: dict[str, str] = {}
    durations: dict[str, float] = {}
    failed = False
    for harness_rel, names in sorted(by_harness.items()):
        harness = prepared / harness_rel
        if not harness.is_file():
            print(f"missing harness {harness}", file=sys.stderr)
            failed = True
            continue
        expected = inventory["harnesses"][harness_rel]["sha256"]
        actual = sha256_file(harness)
        if actual != expected:
            print(f"harness {harness_rel} sha256 mismatch", file=sys.stderr)
            failed = True
            continue
        for offset in range(0, len(names), batch_size):
            batch = names[offset:offset + batch_size]
            # NOTE: --report-time is nightly-only; stable libtest has no
            # per-test timing. Only the harness subprocess is timed, so
            # verification work never inflates the weights.
            command = [
                str(harness), "--exact", *batch, "--format", "pretty",
            ]
            batch_started = time.monotonic()
            result = subprocess.run(command, capture_output=True, text=True)
            elapsed = time.monotonic() - batch_started
            sys.stdout.write(result.stdout)
            sys.stderr.write(result.stderr)
            for line in result.stdout.splitlines():
                match = TEST_LINE.match(line)
                if match:
                    key = f"{harness_rel}::{match.group(1)}"
                    results[key] = match.group(2)
                    if match.group(3) is not None:
                        durations[key] = float(match.group(3))
            if result.returncode != 0 or "FAILED" in results.values():
                failed = True
            # Per-batch wall time spread evenly: enough signal for the
            # weights file without nightly-only --report-time.
            for name in batch:
                key = f"{harness_rel}::{name}"
                if results.get(key) not in (None, "ignored"):
                    durations[key] = round(elapsed / len(batch), 3)

    expected_keys = set(shard_tests)
    ran = {key for key, status in results.items() if status != "ignored"}
    for key in sorted(expected_keys - ran):
        print(f"test did not execute: {key}", file=sys.stderr)
        failed = True
    for key, status in sorted(results.items()):
        if status == "FAILED":
            print(f"FAILED {key}", file=sys.stderr)

    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps({
        "shard": shard_index,
        "results": results,
        "durations": durations,
    }, indent=2, sort_keys=True))
    return 1 if failed else 0


def collect_weights(runs_dir: Path) -> dict[str, float]:
    """The measured per-test durations of one completed run, for the next
    run's LPT assignment. Harness-qualified keys win; a bare-name
    fallback in load_weights covers renamed harnesses."""
    weights: dict[str, float] = {}
    for report_path in sorted(runs_dir.glob("*/report.json")):
        report = json.loads(report_path.read_text())
        for key, seconds in report.get("durations", {}).items():
            name = key.split("::", 1)[1]
            weights[key] = float(seconds)
            weights[name] = max(float(seconds), weights.get(name, 0.0))
    return weights


def verify(inventory: dict, runs_dir: Path) -> int:
    expected: dict[str, int] = {
        key: 0 for key in inventory["runnable"]
    }
    failures = []
    shard_reports = sorted(runs_dir.glob("*/report.json"))
    expected_shards = {str(i) for i in range(len(inventory["shards"]))}
    seen_shards = set()
    for report_path in shard_reports:
        report = json.loads(report_path.read_text())
        shard = str(report["shard"])
        seen_shards.add(shard)
        for key, status in report["results"].items():
            if status == "ignored":
                continue
            if key not in expected:
                failures.append(f"shard {shard} ran unlisted test {key}")
                continue
            expected[key] += 1
            if status != "ok":
                failures.append(f"shard {shard}: {key} -> {status}")
    missing_shards = expected_shards - seen_shards
    if missing_shards:
        failures.append(f"no report from shard(s) {sorted(missing_shards)}")
    for key, count in expected.items():
        if count == 0:
            failures.append(f"test never executed: {key}")
        elif count > 1:
            failures.append(f"test executed {count} times: {key}")
    for failure in failures[:50]:
        print(failure, file=sys.stderr)
    if len(failures) > 50:
        print(f"... and {len(failures) - 50} more", file=sys.stderr)
    if failures:
        return 1
    print(
        f"coverage holds: {len(expected)} tests across "
        f"{len(shard_reports)} shard report(s), each executed exactly once"
    )
    return 0


def pack(root: Path, target_dir: Path, cargo_json: Path, out: Path) -> int:
    """Stage every test-bearing artifact under `out`, preserving the
    artifact's repo-relative layout so compile-time baked paths
    (CARGO_BIN_EXE_*) still resolve on a shard runner."""
    harness_index: dict[str, str] = {}
    copied = 0
    for line in cargo_json.read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        record = json.loads(line)
        if record.get("reason") != "compiler-artifact":
            continue
        if not (record.get("profile") or {}).get("test"):
            continue
        executable = record.get("executable")
        target = record.get("target") or {}
        kinds = target.get("kind") or []
        name = target.get("name")
        if not executable or not kinds or not name:
            continue
        source = Path(executable)
        if not source.is_file():
            print(f"artifact executable missing: {source}", file=sys.stderr)
            return 1
        key = f"{kinds[0]}:{name}"
        rel = Path(os.path.relpath(source.resolve(), root.resolve()))
        if key in harness_index:
            print(f"two harnesses for target {key}", file=sys.stderr)
            return 1
        harness_index[key] = str(rel)
        dest = out / rel
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(source.read_bytes())
        dest.chmod(dest.stat().st_mode | 0o755)
        copied += 1

    # Real executables the harnesses exec (staged siblings and top-level
    # target/debug binaries), keeping the repo-relative layout.
    for candidate in sorted(target_dir.rglob("*")):
        if not candidate.is_file():
            continue
        rel = Path(os.path.relpath(candidate.resolve(), root.resolve()))
        if (out / rel).exists():
            continue
        executable = os.access(candidate, os.X_OK)
        shared_lib = candidate.suffix == ".so" or ".so." in candidate.name
        if not executable and not shared_lib:
            continue
        if candidate.suffix in {".d", ".rlib", ".rmeta"}:
            continue
        dest = out / rel
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(candidate.read_bytes())
        dest.chmod(dest.stat().st_mode | 0o755)
        copied += 1

    (out / "harnesses.json").write_text(
        json.dumps(harness_index, indent=2, sort_keys=True)
    )
    print(f"packed {copied} file(s), {len(harness_index)} harness(es)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("manifest")
    p.add_argument("--root", type=Path, default=Path("."))
    p.add_argument("--features", default="")
    p.add_argument("--profile", default="test")
    p.add_argument("--out", type=Path, required=True)

    p = sub.add_parser("inventory")
    p.add_argument("--manifest", type=Path, required=True)
    p.add_argument("--prepared", type=Path, required=True)
    p.add_argument("--spec", type=Path, required=True,
                   help="JSON: [{name, harness, libtest_args}]")
    p.add_argument("--shards", type=int, required=True)
    p.add_argument("--weights", type=Path)
    p.add_argument("--out", type=Path, required=True)

    p = sub.add_parser("run")
    p.add_argument("--manifest", type=Path, required=True)
    p.add_argument("--prepared", type=Path, required=True)
    p.add_argument("--inventory", type=Path, required=True)
    p.add_argument("--spec", type=Path, required=True,
                   help="the lane spec this run declares; the lane check "
                        "reads it to know which targets this step executes")
    p.add_argument("--features", required=True,
                   help="the features string the manifest must record")
    p.add_argument("--profile", required=True,
                   help="the build profile the manifest must record")
    p.add_argument("--root", type=Path, default=Path("."))
    p.add_argument("--shard", type=int, required=True)
    p.add_argument("--report", type=Path, required=True)

    p = sub.add_parser("weights")
    p.add_argument("--runs", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)

    p = sub.add_parser("verify")
    p.add_argument("--inventory", type=Path, required=True)
    p.add_argument("--runs", type=Path, required=True)

    p = sub.add_parser("pack")
    p.add_argument("--root", type=Path, default=Path("."))
    p.add_argument("--target-dir", type=Path, required=True)
    p.add_argument("--cargo-json", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)

    args = parser.parse_args()

    if args.command == "manifest":
        manifest = build_manifest(args.root, args.features, args.profile)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(manifest, indent=2, sort_keys=True))
        return 0

    if args.command == "inventory":
        spec = json.loads(args.spec.read_text())
        weights = load_weights(args.weights)
        index = json.loads(
            (args.prepared / "harnesses.json").read_text()
        ) if (args.prepared / "harnesses.json").is_file() else {}
        harnesses: dict[str, dict] = {}
        runnable: list[str] = []
        for step in spec:
            selector = step["harness"]
            if "*" in selector:
                matched = sorted(
                    rel for key, rel in index.items()
                    if key.startswith(selector[:-1])
                )
            elif selector in index:
                matched = [index[selector]]
            else:
                matched = [selector]
            if not matched:
                print(f"spec selector {selector} matched no harness",
                      file=sys.stderr)
                return 1
            for rel in matched:
                harness = args.prepared / rel
                if not harness.is_file():
                    print(f"missing harness {harness}", file=sys.stderr)
                    return 1
                listed = list_harness_tests(
                    harness, step.get("libtest_args", []), ignored=False
                )
                ignored = list_harness_tests(
                    harness, step.get("libtest_args", []), ignored=True
                )
                entry = harnesses.setdefault(rel, {
                    "sha256": sha256_file(harness),
                    "runnable": [],
                    "ignored": [],
                })
                entry["runnable"].extend(listed)
                entry["ignored"].extend(ignored)
                runnable.extend(f"{rel}::{name}" for name in listed)
        weighted = [
            (key, weights.get(key, weights.get(key.split("::", 1)[1], 1.0)))
            for key in runnable
        ]
        shards = assign_shards(weighted, args.shards)
        args.out.mkdir(parents=True, exist_ok=True)
        for index, keys in enumerate(shards):
            (args.out / f"shard-{index}.txt").write_text(
                "\n".join(sorted(keys)) + "\n"
            )
        inventory = {
            "manifest": json.loads(args.manifest.read_text()),
            "harnesses": harnesses,
            "runnable": runnable,
            "shards": shards,
        }
        (args.out / "inventory.json").write_text(
            json.dumps(inventory, indent=2, sort_keys=True)
        )
        print(
            f"inventory: {len(runnable)} runnable tests across "
            f"{len(harnesses)} harness(es) -> {args.shards} shard(s)"
        )
        return 0

    if args.command == "run":
        manifest = json.loads(args.manifest.read_text())
        problems = check_manifest(
            manifest, args.root, args.features, args.profile)
        for problem in problems:
            print(f"artifact identity mismatch: {problem}", file=sys.stderr)
        if problems:
            return 1
        if not args.spec.is_file():
            print(f"missing shard spec {args.spec}", file=sys.stderr)
            return 1
        inventory = json.loads(args.inventory.read_text())
        if args.shard < 0 or args.shard >= len(inventory["shards"]):
            print(f"shard {args.shard} out of range", file=sys.stderr)
            return 1
        return run_shard(
            args.prepared, inventory, args.shard, args.report,
        )

    if args.command == "weights":
        weights = collect_weights(args.runs)
        args.out.write_text(json.dumps(weights, indent=2, sort_keys=True) + "\n")
        return 0

    if args.command == "verify":
        inventory = json.loads(args.inventory.read_text())
        return verify(inventory, args.runs)

    if args.command == "pack":
        return pack(args.root, args.target_dir, args.cargo_json, args.out)

    return 2


if __name__ == "__main__":
    sys.exit(main())
