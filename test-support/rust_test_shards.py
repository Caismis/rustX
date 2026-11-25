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
import subprocess
import sys
from pathlib import Path

TEST_LINE = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored|bench: .*)$")


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


def check_manifest(manifest: dict, root: Path) -> list[str]:
    problems = []
    if manifest.get("git_head") != git_head(root):
        problems.append("artifact was built for a different commit")
    if manifest.get("runner_os") != platform.system():
        problems.append("artifact was built on a different OS")
    if manifest.get("runner_arch") != platform.machine():
        problems.append("artifact was built for a different architecture")
    if manifest.get("cargo_lock_sha256") != sha256_file(root / "Cargo.lock"):
        problems.append("Cargo.lock changed since the artifact was built")
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
            command = [
                str(harness), "--exact", *batch, "--format", "pretty",
            ]
            result = subprocess.run(command, capture_output=True, text=True)
            sys.stdout.write(result.stdout)
            sys.stderr.write(result.stderr)
            for line in result.stdout.splitlines():
                match = TEST_LINE.match(line)
                if match:
                    results[f"{harness_rel}::{match.group(1)}"] = match.group(2)
            if result.returncode != 0 or "FAILED" in results.values():
                failed = True

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
    }, indent=2, sort_keys=True))
    return 1 if failed else 0


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
    p.add_argument("--root", type=Path, default=Path("."))
    p.add_argument("--shard", type=int, required=True)
    p.add_argument("--report", type=Path, required=True)

    p = sub.add_parser("verify")
    p.add_argument("--inventory", type=Path, required=True)
    p.add_argument("--runs", type=Path, required=True)

    args = parser.parse_args()

    if args.command == "manifest":
        manifest = build_manifest(args.root, args.features, args.profile)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(manifest, indent=2, sort_keys=True))
        return 0

    if args.command == "inventory":
        spec = json.loads(args.spec.read_text())
        weights = load_weights(args.weights)
        harnesses: dict[str, dict] = {}
        runnable: list[str] = []
        for step in spec:
            harness = args.prepared / step["harness"]
            if not harness.is_file():
                print(f"missing harness {harness}", file=sys.stderr)
                return 1
            rel = step["harness"]
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
        problems = check_manifest(manifest, args.root)
        for problem in problems:
            print(f"artifact identity mismatch: {problem}", file=sys.stderr)
        if problems:
            return 1
        inventory = json.loads(args.inventory.read_text())
        if args.shard < 0 or args.shard >= len(inventory["shards"]):
            print(f"shard {args.shard} out of range", file=sys.stderr)
            return 1
        return run_shard(
            args.prepared, inventory, args.shard, args.report,
        )

    if args.command == "verify":
        inventory = json.loads(args.inventory.read_text())
        return verify(inventory, args.runs)

    return 2


if __name__ == "__main__":
    sys.exit(main())
