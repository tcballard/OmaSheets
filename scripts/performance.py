#!/usr/bin/env python3
"""Generate deterministic fixtures and record bounded Linux performance evidence."""

from __future__ import annotations

import argparse
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

from omasheets.performance import (  # noqa: E402
    bounded_json,
    fixture_manifest,
    generate_fixture_suite,
    measure_command,
    write_bounded_json,
)


def _emit(payload: dict, output: Path | None) -> None:
    if output is None:
        print(bounded_json(payload), end="")
    else:
        write_bounded_json(output, payload)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)

    specs = commands.add_parser("specs", help="Print exact fixture sizes without generating files")
    specs.add_argument("--profile", choices=("smoke", "standard"), default="standard")
    specs.add_argument("--output", type=Path)

    fixtures = commands.add_parser("fixtures", help="Generate deterministic FODS fixtures")
    fixtures.add_argument("--profile", choices=("smoke", "standard"), default="standard")
    fixtures.add_argument("--directory", type=Path, required=True)

    run = commands.add_parser("run", help="Measure one foreground command and its process group")
    run.add_argument("--name", required=True)
    run.add_argument("--output", type=Path)
    run.add_argument("--interval", type=float, default=0.1)
    run.add_argument("--timeout", type=float)
    run.add_argument("--max-samples", type=int, default=512)
    run.add_argument(
        "--include-command",
        action="store_true",
        help="Include argv in evidence; off by default to avoid recording secrets",
    )
    run.add_argument("command", nargs=argparse.REMAINDER)
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
    if arguments.action == "specs":
        _emit(fixture_manifest(arguments.profile), arguments.output)
        return 0
    if arguments.action == "fixtures":
        print(bounded_json(generate_fixture_suite(arguments.directory, arguments.profile)), end="")
        return 0
    command = arguments.command
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        raise SystemExit("performance run requires a command after --")
    result = measure_command(
        arguments.name,
        command,
        interval_seconds=arguments.interval,
        timeout_seconds=arguments.timeout,
        max_samples=arguments.max_samples,
        include_command=arguments.include_command,
    )
    _emit(result, arguments.output)
    return 0 if result["exit_code"] == 0 and not result["timed_out"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
