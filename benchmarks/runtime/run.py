#!/usr/bin/env python3
"""Run preserved conformance executables in fresh processes, retaining every result.

Build: cargo test --release --locked --test conformance --no-run --message-format=json
Use the executable in Cargo's compiler-artifact record. Copy it before rebuilding.
A paired run alternates BEFORE/AFTER for each case and repeats at least 3 times.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import time

CASES = ["short-small", "short-large", "stream", "large", "tool", "cancel"]
TEST = "agent_loop::runtime_benchmark::measure_runtime"


def command(*args):
    result = subprocess.run(args, text=True, capture_output=True, check=False)
    return {"argv": args, "status": result.returncode, "stdout": result.stdout, "stderr": result.stderr}


def digest(path):
    return hashlib.file_digest(Path(path).open("rb"), "sha256").hexdigest()


PHASES = ["before_initialization", "after_initialization", "after_turns",
          "after_verification", "after_shutdown", "after_driver_drop",
          "after_final_writer_close", "after_reopen_verification", "after_cleanup",
          "after_provider_teardown"]


def validate_measurement(returncode, log, sample_path, case):
    """Exit zero is insufficient: prove the exact test ran and its oracle passed."""
    errors = []
    if returncode:
        errors.append(f"process exited {returncode}")
    if len(re.findall(r"^test " + re.escape(TEST) + r" \.\.\. ok$", log, re.MULTILINE)) != 1:
        errors.append("intended measurement did not report exactly one passing test")
    if "test result: ok. 1 passed; 0 failed; 0 ignored;" not in log:
        errors.append("measurement test count was not exactly one")
    try:
        sample = json.loads(sample_path.read_text())
        history = {"short-small": 8, "short-large": 64}.get(case, 0)
        measured = 32 if history else 1
        expected_requests = history + measured + int(case == "tool")
        assert sample["case"] == case
        assert sample["measurement_version"] == sample["oracle_version"] == 2
        assert sample["oracle"] == "passed"
        assert isinstance(sample["schema"], int) and isinstance(sample["protocol"], int)
        assert set(sample["initial_process"]) == {"stat", "io", "smaps_rollup"}
        for key in ("before_close", "closed_footprint", "checkpoint_footprint"):
            assert isinstance(sample[key], list)
        assert sample["history"] == history and sample["measured"] == measured
        assert sample["requests"] == expected_requests
        assert sample["verified_live_requests"] == sample["verified_reopened_requests"] == expected_requests
        assert len(sample["samples"]) == history + measured
        for key in ("open_us", "shutdown_us", "close_us", "final_writer_close_us", "reopen_us", "cleanup_us"):
            assert isinstance(sample[key], int) and sample[key] >= 0
        for index, point in enumerate(sample["samples"]):
            assert point["turn"] == index
            for key in ("settled_us", "accepted_us"):
                assert isinstance(point[key], int) and point[key] >= 0
            publication = point["first_consumed_publication_us"]
            assert publication is None or isinstance(publication, int) and publication >= 0
            assert isinstance(point["notification_records"], int) and point["notification_records"] >= 0
            assert isinstance(point["footprint"], list)
            assert set(point["process"]) == {"stat", "io", "smaps_rollup"}
        assert [s["measured"] for s in sample["samples"]] == [i >= history for i in range(history + measured)]
        assert [s["phase"] for s in sample["lifecycle"]] == PHASES
        elapsed = [s["elapsed_us"] for s in sample["lifecycle"]]
        assert all(isinstance(v, int) and v >= 0 for v in elapsed)
        assert elapsed == sorted(elapsed)
        for point in sample["lifecycle"]:
            assert set(point["process"]) == {"stat", "io", "smaps_rollup"}
            assert all(value is None or isinstance(value, str) for value in point["process"].values())
    except (OSError, ValueError, KeyError, TypeError, AssertionError) as error:
        errors.append(f"missing or invalid measurement/oracle sample: {type(error).__name__}: {error}")
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pairs", type=int, default=3)
    parser.add_argument("--case", choices=CASES, action="append")
    parser.add_argument("--trace", action="store_true", help="separate instrumented run, never headline timing")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    executables = {"before": args.before.resolve()}
    if args.after:
        executables["after"] = args.after.resolve()
    manifest = {
        "version": 2, "measurement_test": TEST, "cases": args.case or CASES, "pairs": args.pairs,
        "executables": {name: {"path": str(path), "sha256": digest(path)} for name, path in executables.items()},
        "head": command("git", "rev-parse", "HEAD"), "diff": command("git", "diff", "--stat"),
        "clock_ticks": command("getconf", "CLK_TCK"),
        "toolchain": command("rustc", "-Vv"), "os": platform.platform(),
        "cpu": command("lscpu"), "memory": command("free", "-b"),
        "filesystem": command("findmnt", "-T", str(args.output.resolve())),
        "cache": "warm OS cache; no cache drop; fresh process and store per sample",
        "trace": args.trace, "scope": "runtime plus driver in one process; provider separate child",
        "fixture_sha256": digest("test-support/fake-provider/src/fake_provider/scenarios/runtime_benchmark.py"),
        "cargo_lock_sha256": digest("Cargo.lock"),
        "harness_sha256": digest("tests/conformance/agent_loop/runtime_benchmark.rs"),
        "oracle_sha256": digest("tests/conformance/agent_loop/runtime_benchmark/request_oracle.rs"),
        "expected_tools_sha256": digest("tests/conformance/agent_loop/runtime_benchmark/expected_tools.json"),
        "driver_sha256": digest("tests/conformance/agent_loop.rs"),
        "toolchain_file_sha256": digest("rust-toolchain.toml"),
        "cpu_profiler_probe": command("perf", "stat", "-e", "task-clock,cycles,instructions", "true"),
        "features": "default", "profile": "release", "allocation_profile": None,
    }
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2))
    for pair in range(args.pairs):
        for case in args.case or CASES:
            for label, executable in executables.items():
                stem = args.output / f"{pair}-{case}-{label}"
                env = {key: value for key, value in os.environ.items()
                       if not any(secret in key.upper() for secret in ("API_KEY", "TOKEN", "SECRET", "CREDENTIAL"))}
                env.update(RUSTX_REQUIRE_PROVIDER_EMULATOR="1", RUSTX_BENCH_CASE=case,
                           RUSTX_BENCH_OUTPUT=str(stem.with_suffix(".json").resolve()))
                argv = [str(executable), TEST, "--exact", "--ignored", "--nocapture"]
                if args.trace:
                    argv = ["strace", "-f", "-yy", "-T", "-e", "trace=write,pwrite64,fsync,fdatasync,fcntl,unlink,rename",
                            "-o", str(stem.with_suffix(".strace")), *argv]
                began = time.monotonic()
                with stem.with_suffix(".log").open("w") as log:
                    result = subprocess.run(argv, env=env, stdout=log, stderr=subprocess.STDOUT, check=False)
                elapsed_seconds = time.monotonic()-began
                errors = validate_measurement(result.returncode, stem.with_suffix(".log").read_text(), stem.with_suffix(".json"), case)
                record = {"validation_errors": errors, "accepted": not errors, "argv": argv, "returncode": result.returncode, "elapsed_seconds": elapsed_seconds,
                          "pair": pair, "case": case, "label": label}
                stem.with_suffix(".process.json").write_text(json.dumps(record, indent=2))
                print(json.dumps(record), flush=True)
    # Failure evidence is retained; the overall command still fails closed.
    records = [json.loads(path.read_text()) for path in args.output.glob("*.process.json")]
    raise SystemExit(int(any(not record["accepted"] for record in records)))


if __name__ == "__main__":
    main()
