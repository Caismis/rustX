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
import subprocess
import time

CASES = ["short-small", "short-large", "stream", "large", "tool", "cancel"]
TEST = "agent_loop::runtime_benchmark::measure_runtime"


def command(*args):
    result = subprocess.run(args, text=True, capture_output=True, check=False)
    return {"argv": args, "status": result.returncode, "stdout": result.stdout, "stderr": result.stderr}


def digest(path):
    return hashlib.file_digest(Path(path).open("rb"), "sha256").hexdigest()


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
        "version": 1, "cases": args.case or CASES, "pairs": args.pairs,
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
                record = {"argv": argv, "returncode": result.returncode, "elapsed_seconds": time.monotonic()-began,
                          "pair": pair, "case": case, "label": label}
                stem.with_suffix(".process.json").write_text(json.dumps(record, indent=2))
                print(json.dumps(record), flush=True)
    # Failure evidence is retained; the overall command still fails closed.
    records = [json.loads(path.read_text()) for path in args.output.glob("*.process.json")]
    raise SystemExit(int(any(record["returncode"] for record in records)))


if __name__ == "__main__":
    main()
