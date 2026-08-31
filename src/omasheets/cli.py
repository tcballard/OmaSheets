"""Local command-line entry point and MCP transport launcher."""

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
    status = commands.add_parser("status", help="Show bounded local panel status")
    status.add_argument("--json", action="store_true", help="Emit JSON (the stable v0.0.1 format)")
    open_command = commands.add_parser("open", help="Open workbooks in LibreOffice Calc")
    open_command.add_argument("paths", nargs="+", type=Path)
    commands.add_parser("open-current", help="Open the selected workbook in LibreOffice Calc")
    convert = commands.add_parser("convert", help="Convert .xls to a new adjacent .xlsx")
    convert.add_argument("path", type=Path)
    doctor = commands.add_parser("doctor", help="Check the local OmaSheets runtime")
    doctor.add_argument("--json", action="store_true", help="Emit JSON")
    commands.add_parser("review-current", help="Review the newest staged plan in this terminal")
    plan = commands.add_parser("plan", help="Review a staged plan")
    plan_commands = plan.add_subparsers(dest="plan_command")
    approve = plan_commands.add_parser("approve", help="Review and publish a plan locally")
    approve.add_argument("plan_id")
    approve.add_argument("--revision", required=True, type=int)
    approve.add_argument("--replace", action="store_true")
    approve.add_argument("--destination", type=Path)
    undo = commands.add_parser("undo", help="Undo a replacement receipt locally")
    undo.add_argument("receipt_id")
    integrate = commands.add_parser("integrate", help="Manage user-local desktop and MIME integration")
    integrate_commands = integrate.add_subparsers(dest="integrate_command", required=True)
    integrate_commands.add_parser("install", help="Install the desktop entry and MIME associations")
    integrate_commands.add_parser("uninstall", help="Restore or remove OmaSheets integration")
    lok = commands.add_parser("lok", help="Run the experimental LibreOfficeKit spike")
    lok_commands = lok.add_subparsers(dest="lok_command", required=True)
    lok_status = lok_commands.add_parser("status", help="Check LibreOfficeKit spike dependencies")
    lok_status.add_argument("--json", action="store_true", help="Emit JSON")
    lok_render = lok_commands.add_parser("render", help="Render a workbook tile through LibreOfficeKit")
    lok_render.add_argument("path", type=Path)
    lok_render.add_argument("--output", required=True, type=Path)
    lok_render.add_argument("--width", type=int, default=1024)
    lok_render.add_argument("--height", type=int, default=640)
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
    if arguments.command == "mcp" and arguments.mcp_command == "serve":
        from .mcp import serve_stdio

        return serve_stdio(_service())
    if arguments.command == "select":
        print(json.dumps(_service().select_workbook(arguments.path), indent=2, sort_keys=True))
        return 0
    if arguments.command == "status":
        print(json.dumps(_service().local_status(), indent=2, sort_keys=True))
        return 0
    if arguments.command in {"open", "open-current"}:
        from .desktop import open_workbooks

        service = _service()
        paths = arguments.paths if arguments.command == "open" else [service.current_local_path()]
        print(json.dumps({"pid": open_workbooks(paths), "opened": len(paths)}, sort_keys=True))
        return 0
    if arguments.command == "convert":
        print(json.dumps(_service().convert_legacy_local(arguments.path), indent=2, sort_keys=True))
        return 0
    if arguments.command == "doctor":
        from .doctor import diagnose

        result = diagnose()
        if arguments.json:
            print(json.dumps(result, indent=2, sort_keys=True))
        else:
            for check in result["checks"]:
                mark = "ok" if check["ok"] else "missing"
                print(f"{mark:7} {check['name']}: {check['detail']}")
            print("ready" if result["ready"] else "not ready")
        return 0 if result["ready"] else 1
    if arguments.command == "review-current":
        service = _service()
        status = service.local_status()["review"]
        if not status["pending"]:
            raise SystemExit("No staged plan is awaiting local review.")
        if status["status"] in {"verified", "review_pending"}:
            review = service.prepare_local_review(status["plan_id"], status["revision"])
        else:
            review = service.get_plan(status["plan_id"])
            review["approval_token"] = f"APPLY {status['plan_id']}"
        print(json.dumps(review, indent=2, sort_keys=True))
        supplied = input(f"Type {review['approval_token']} to publish a new copy: ")
        receipt = service.commit_local_review(status["plan_id"], status["revision"], supplied)
        print(json.dumps(receipt, indent=2, sort_keys=True))
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
    if arguments.command == "integrate":
        from .integration import install, uninstall

        result = install() if arguments.integrate_command == "install" else uninstall()
        print(json.dumps(result, indent=2, sort_keys=True))
        return 0
    if arguments.command == "lok":
        from .lok_spike import render_workbook, status

        if arguments.lok_command == "status":
            result = status()
            if arguments.json:
                print(json.dumps(result, indent=2, sort_keys=True))
            else:
                for check in result["checks"]:
                    print(f"{'ok' if check['ok'] else 'missing':7} {check['name']}: {check['detail']}")
                print("spike ready" if result["ready"] else "spike not ready")
            return 0 if result["ready"] else 1
        result = render_workbook(
            arguments.path,
            arguments.output,
            width=arguments.width,
            height=arguments.height,
        )
        print(json.dumps(result, indent=2, sort_keys=True))
        return 0
    return 0
