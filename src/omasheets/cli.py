"""Minimal command-line entry point; executable commands land in later slices."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from . import __version__


def _service():
    from .calc_engine import CalcEngine
    from .paths import AppPaths
    from .service import OmaSheetsService

    paths = AppPaths.discover()
    return OmaSheetsService(paths, CalcEngine(paths))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="omasheets")
    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    commands = parser.add_subparsers(dest="command")
    mcp = commands.add_parser("mcp", help="Model Context Protocol operations")
    mcp_commands = mcp.add_subparsers(dest="mcp_command")
    mcp_commands.add_parser("serve", help="Serve MCP over standard input/output")
    select = commands.add_parser("select", help="Select a local workbook for agent access")
    select.add_argument("path", type=Path)
    plan = commands.add_parser("plan", help="Review a staged plan")
    plan_commands = plan.add_subparsers(dest="plan_command")
    approve = plan_commands.add_parser("approve", help="Review and publish a plan locally")
    approve.add_argument("plan_id")
    approve.add_argument("--revision", required=True, type=int)
    approve.add_argument("--replace", action="store_true")
    approve.add_argument("--destination", type=Path)
    undo = commands.add_parser("undo", help="Undo a replacement receipt locally")
    undo.add_argument("receipt_id")
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
    if arguments.command == "mcp" and arguments.mcp_command == "serve":
        from .mcp import serve_stdio

        return serve_stdio(_service())
    if arguments.command == "select":
        print(json.dumps(_service().select_workbook(arguments.path), indent=2, sort_keys=True))
        return 0
    if arguments.command == "plan" and arguments.plan_command == "approve":
        service = _service()
        mode = "replace" if arguments.replace else "copy"
        review = service.prepare_local_review(
            arguments.plan_id,
            arguments.revision,
            mode=mode,
            destination=arguments.destination,
        )
        print(json.dumps(review, indent=2, sort_keys=True))
        supplied = input(f"Type {review['approval_token']} to publish: ")
        receipt = service.commit_local_review(arguments.plan_id, arguments.revision, supplied)
        print(json.dumps(receipt, indent=2, sort_keys=True))
        return 0
    if arguments.command == "undo":
        service = _service()
        supplied = input(f"Type UNDO {arguments.receipt_id} to restore the verified backup: ")
        receipt = service.undo_receipt(arguments.receipt_id, supplied)
        print(json.dumps(receipt, indent=2, sort_keys=True))
        return 0
    return 0
