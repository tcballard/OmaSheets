#!/usr/bin/env python3
"""Produce the bounded JSON status record the Omarchy bar widget displays.

The installed launcher is run in its own process group with a hard deadline
and a producer-side byte limit. Only a JSON object that arrived complete and
within both limits is re-emitted, compactly, on stdout; every other outcome
exits non-zero so the widget shows its fixed "status unavailable" text.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

from omasheets.bounded_process import run_bounded  # noqa: E402

STATUS_BYTE_LIMIT = 16 * 1024
STATUS_TIMEOUT_SECONDS = 5.0
NOT_INSTALLED = {"installed": False, "current": {"selected": False}, "review": {"pending": False}}


def main(argv: list[str] | None = None) -> int:
    arguments = sys.argv[1:] if argv is None else argv
    if len(arguments) != 1:
        print("usage: panel_status.py LAUNCHER", file=sys.stderr)
        return 2
    launcher = Path(arguments[0])
    if not (launcher.is_file() and os.access(launcher, os.X_OK)):
        print(json.dumps(NOT_INSTALLED, separators=(",", ":"), sort_keys=True))
        return 0
    result = run_bounded(
        [str(launcher), "status", "--json"],
        byte_limit=STATUS_BYTE_LIMIT,
        timeout_seconds=STATUS_TIMEOUT_SECONDS,
    )
    if not result.ok:
        print(f"omasheets status {result.status} (exit {result.returncode})", file=sys.stderr)
        return 1
    try:
        payload = json.loads(result.output.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        print("omasheets status is not valid JSON", file=sys.stderr)
        return 1
    if not isinstance(payload, dict):
        print("omasheets status is not a JSON object", file=sys.stderr)
        return 1
    print(json.dumps(payload, separators=(",", ":"), sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
