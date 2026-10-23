"""Install CI prerequisites without waiting on the runner's Azure mirror."""

import argparse
import pathlib
import subprocess


MIRROR_LIST = pathlib.Path("/etc/apt/apt-mirrors.txt")
ARCHIVE = "https://archive.ubuntu.com/ubuntu/\n"
APT_OPTIONS = [
    "-o", "Acquire::http::Timeout=30",
    "-o", "Acquire::https::Timeout=30",
    "-o", "Acquire::Retries=2",
    "-o", "APT::Update::Error-Mode=any",
]


def install(packages):
    # GitHub's Ubuntu image points its signed distribution sources at this
    # mirror list. Keep those suites/components/keyrings; select the official
    # archive directly instead of spending the job budget on Azure failover.
    if not MIRROR_LIST.is_file():
        raise RuntimeError("the GitHub Ubuntu runner's APT mirror list is missing")
    MIRROR_LIST.write_text(ARCHIVE)
    # These are connection/idle deadlines and bounded file retries, not test
    # retries or a larger job budget. Any index failure must stop installation
    # rather than use stale metadata from the previous mirror.
    subprocess.run(["apt-get", *APT_OPTIONS, "update"], check=True)
    subprocess.run(["apt-get", *APT_OPTIONS, "install", "--yes", *packages], check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packages", nargs="+")
    install(parser.parse_args().packages)
