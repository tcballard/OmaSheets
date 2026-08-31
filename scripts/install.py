#!/usr/bin/env python3
"""Bootstrap OmaSheets from an Omarchy-managed plugin checkout."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

from omasheets.installation import dependency_report, install, uninstall  # noqa: E402


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="omasheets-plugin")
    parser.add_argument("command", choices=("install", "doctor", "uninstall"))
    arguments = parser.parse_args(argv)
    if arguments.command == "doctor":
        result = dependency_report()
    elif arguments.command == "install":
        result = install(ROOT)
    else:
        result = uninstall()
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result.get("ready", True) and not result.get("conflicts") else 1


if __name__ == "__main__":
    raise SystemExit(main())
