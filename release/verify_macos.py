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


def native_harness() -> Path:
    args = ["cargo", "test", "--test", "process", "--all-features", "--locked",
            "--no-run", "--message-format=json"]
    build = subprocess.run(args, cwd=ROOT, text=True, stdout=subprocess.PIPE, check=True)
    artifacts = map(json.loads, build.stdout.splitlines())
    [harness] = [Path(record["executable"]) for record in artifacts
                 if record["reason"] == "compiler-artifact"
                 and record["target"]["name"] == "process"
                 and record["profile"]["test"] and record["executable"]]
    return harness


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("this verification requires a native macOS runner")
    directory = args.out.resolve()
    run("cargo", "test", "--lib", "--all-features", "--locked", "--no-run")
    run("cargo", "clippy", "--all-targets", "--all-features", "--locked", "--", "-D", "warnings")
    harness = native_harness()
    run("python3", "release/package.py", "all", "--out", directory)
    archives = sorted(directory.glob("*.tar.gz"))
    run("python3", "release/conformance.py", *archives, "--process-harness", harness)
    for archive in archives:
        with tarfile.open(archive, "r:gz") as package:
            member = package.extractfile(f"{archive.name.removesuffix('.tar.gz')}/manifest.json")
            if member is None:
                raise ValueError(f"Missing manifest in {archive.name}")
            (directory / f"{archive.name}.manifest.json").write_bytes(member.read())


if __name__ == "__main__":
    main()
