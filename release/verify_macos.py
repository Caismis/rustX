#!/usr/bin/env python3
"""Run the native macOS installation proof outside the Linux verification profile."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import platform
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parent.parent


def run(*args: str | Path) -> None:
    subprocess.run(args, cwd=ROOT, check=True)


def native_harness(package: str, target: str) -> Path:
    args = ["cargo", "test", "--package", package, "--test", target, "--all-features",
            "--locked", "--no-run", "--message-format=json"]
    build = subprocess.run(args, cwd=ROOT, text=True, stdout=subprocess.PIPE)
    if build.returncode:
        print(build.stdout, flush=True)
        build.check_returncode()
    artifacts = map(json.loads, build.stdout.splitlines())
    [harness] = [Path(record["executable"]) for record in artifacts
                 if record["reason"] == "compiler-artifact"
                 and record["target"]["name"] == target
                 and record["profile"]["test"] and record["executable"]]
    return harness


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("this verification requires a native macOS runner")
    directory = args.out.resolve()
    run("cargo", "clippy", "--all-targets", "--all-features", "--locked", "--", "-D", "warnings")
    run("cargo", "test", "--lib", "--all-features", "--locked", "--no-run")
    harness = native_harness("rustx", "process")
    client_harness = native_harness("rustx-execution-client", "bootstrap")
    run("python3", "release/package.py", "all", "--out", directory)
    run("corepack", "pnpm@11.13.1", "--dir", "web-console", "install", "--frozen-lockfile")
    run("corepack", "pnpm@11.13.1", "--dir", "web-console", "exec", "playwright", "install", "chromium")
    archives = sorted(directory.glob("*.tar.gz"))
    run("python3", "release/conformance.py", *archives, "--browser-proof", "--process-harness", harness,
        "--client-harness", client_harness)
    for archive in archives:
        with tarfile.open(archive, "r:gz") as package:
            member = package.extractfile(f"{archive.name.removesuffix('.tar.gz')}/manifest.json")
            if member is None:
                raise ValueError(f"Missing manifest in {archive.name}")
            (directory / f"{archive.name}.manifest.json").write_bytes(member.read())


if __name__ == "__main__":
    main()
