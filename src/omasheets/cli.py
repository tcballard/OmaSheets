"""Minimal command-line entry point; executable commands land in later slices."""

from __future__ import annotations

import argparse

from . import __version__


class _UnavailableService:
    """Fail closed until the local service wiring is installed."""

    def __getattr__(self, name: str):
        from .errors import EngineError

        def unavailable(**arguments):
            del arguments
            raise EngineError(f"local OmaSheets service is unavailable for {name}")

        return unavailable


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="omasheets")
    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    commands = parser.add_subparsers(dest="command")
    mcp = commands.add_parser("mcp", help="Model Context Protocol operations")
    mcp_commands = mcp.add_subparsers(dest="mcp_command")
    mcp_commands.add_parser("serve", help="Serve MCP over standard input/output")
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
    if arguments.command == "mcp" and arguments.mcp_command == "serve":
        from .mcp import serve_stdio

        return serve_stdio(_UnavailableService())
    return 0
