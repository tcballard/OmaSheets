#!/usr/bin/env python3
"""Record the exact inputs of a native release build as JSON.

The record names the container image digest, the Arch Linux Archive snapshot
the package set was pinned to, the installed versions of the packages that
shape the executables, the compiler flags, and the workflow identity. It is
embedded in the bundle manifest so every installation receipt carries it, and
it is the recipe a maintainer follows to rebuild the bundle bit for bit before
signing it.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

DEFAULT_PACKAGES = (
    "gcc", "glibc", "cmake", "make", "rust", "gtk3", "libreoffice-fresh", "libreoffice-fresh-sdk",
)


def installed_versions(packages: tuple[str, ...]) -> dict[str, str]:
    completed = subprocess.run(
        ["pacman", "-Q", *packages], text=True, capture_output=True, check=True, timeout=30,
    )
    versions: dict[str, str] = {}
    for line in completed.stdout.splitlines():
        name, _, version = line.partition(" ")
        if name and version:
            versions[name] = version.strip()
    missing = sorted(set(packages) - set(versions))
    if missing:
        raise RuntimeError(f"packages are not installed: {', '.join(missing)}")
    return versions


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True, help="container image reference including its digest")
    parser.add_argument("--snapshot", required=True, help="Arch Linux Archive snapshot date, YYYY/MM/DD")
    parser.add_argument("--source-date-epoch", type=int, required=True)
    parser.add_argument("--cxxflags", default="", help="pass as --cxxflags=VALUE because flags begin with a dash")
    parser.add_argument("--package", action="append", default=[], help="package whose version is recorded")
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args(argv)
    if "@sha256:" not in arguments.image:
        parser.error("--image must carry a sha256 digest")
    packages = tuple(arguments.package) or DEFAULT_PACKAGES
    record = {
        "schema": 1,
        "image": arguments.image,
        "package_snapshot": f"https://archive.archlinux.org/repos/{arguments.snapshot}/",
        "packages": installed_versions(packages),
        "source_date_epoch": arguments.source_date_epoch,
        "cxxflags": arguments.cxxflags,
        "workflow": {
            "repository": os.environ.get("GITHUB_REPOSITORY", ""),
            "ref": os.environ.get("GITHUB_WORKFLOW_REF", ""),
            "sha": os.environ.get("GITHUB_SHA", ""),
            "run_id": os.environ.get("GITHUB_RUN_ID", ""),
            "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT", ""),
        },
    }
    arguments.output.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    print(json.dumps(record, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
