"""Local command-line entry point and MCP transport launcher."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

from . import __version__


def _json_object(value: str) -> dict:
    try:
        payload = json.loads(value)
    except json.JSONDecodeError as exc:
        raise argparse.ArgumentTypeError("arguments must be valid JSON") from exc
    if not isinstance(payload, dict):
        raise argparse.ArgumentTypeError("arguments must be a JSON object")
    return payload


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
    status.add_argument("--json", action="store_true", help="Emit stable JSON")
    open_command = commands.add_parser("open", help="Open workbooks in LibreOffice Calc")
    open_command.add_argument("paths", nargs="+", type=Path)
    commands.add_parser("open-current", help="Open the selected workbook in LibreOffice Calc")
    window = commands.add_parser("window", help="Open a workbook in the native OmaSheets window")
    window.add_argument("path", type=Path)
    commands.add_parser("window-current", help="Open the selected workbook in the OmaSheets window")
    convert = commands.add_parser("convert", help="Convert .xls to a new adjacent .xlsx")
    convert.add_argument("path", type=Path)
    doctor = commands.add_parser("doctor", help="Check the local OmaSheets runtime")
    doctor.add_argument("--json", action="store_true", help="Emit JSON")
    commands.add_parser("review-current", help="Review the newest staged plan in this terminal")
    agent_session = commands.add_parser(
        "agent-session", help="Open or interact with an OmaSheets agent session",
    )
    agent_session_commands = agent_session.add_subparsers(dest="agent_session_command")
    agent_session_commands.add_parser("resource", help="Read the path-free current session")
    agent_session_commands.add_parser("tools", help="List bounded agent tool schemas")
    agent_call = agent_session_commands.add_parser("call", help="Call one bounded agent tool")
    agent_call.add_argument("tool")
    agent_call.add_argument("--arguments", type=_json_object, default={})
    plan = commands.add_parser("plan", help="Review a staged plan")
    plan_commands = plan.add_subparsers(dest="plan_command")
    approve = plan_commands.add_parser("approve", help="Review and publish a plan locally")
    approve.add_argument("plan_id")
    approve.add_argument("--revision", required=True, type=int)
    approve.add_argument("--replace", action="store_true")
    approve.add_argument("--destination", type=Path)
    publish_copy = plan_commands.add_parser("publish-copy-native", help=argparse.SUPPRESS)
    publish_copy.add_argument("plan_id")
    publish_copy.add_argument("--revision", required=True, type=int)
    publish_copy.add_argument("--destination", required=True, type=Path)
    undo = commands.add_parser("undo", help="Undo a replacement receipt locally")
    undo.add_argument("receipt_id")
    integrate = commands.add_parser("integrate", help="Manage user-local desktop and MIME integration")
    integrate_commands = integrate.add_subparsers(dest="integrate_command", required=True)
    integrate_commands.add_parser("install", help="Install the desktop entry and MIME associations")
    integrate_commands.add_parser("uninstall", help="Restore or remove OmaSheets integration")
    commands.add_parser("uninstall", help="Remove the user-local OmaSheets product installation")
    lok = commands.add_parser("lok", help="Inspect the installed LibreOfficeKit engine")
    lok_commands = lok.add_subparsers(dest="lok_command", required=True)
    lok_status = lok_commands.add_parser("status", help="Check LibreOfficeKit engine dependencies")
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
    if arguments.command in {"window", "window-current"}:
        from .diff_overlay import overlay_path
        from .native_window import open_window
        from .live_bridge import bridge_path

        service = _service()
        if arguments.command == "window":
            session = service.select_workbook(arguments.path)
        else:
            session = service.current_resource()
            if not session.get("selected"):
                raise SystemExit("No workbook is selected.")
        path = service.current_local_path()
        context = service.prepare_window_context(session["session_id"])
        pid = open_window(
            path,
            context_path=context,
            session_id=session["session_id"],
            revision=session["revision"],
            bridge_path=bridge_path(service.paths),
            diff_path=overlay_path(service.paths.runtime),
            cli_path=Path(sys.argv[0]).resolve(),
        )
        print(json.dumps({"pid": pid, "session_id": session["session_id"], "window": "omasheets"}, sort_keys=True))
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
    if arguments.command == "agent-session":
        if arguments.agent_session_command == "resource":
            print(json.dumps(_service().agent_session_resource(), indent=2, sort_keys=True))
            return 0
        if arguments.agent_session_command == "tools":
            from .mcp import TOOLS

            print(json.dumps({"tools": TOOLS}, indent=2, sort_keys=True))
            return 0
        if arguments.agent_session_command == "call":
            from .mcp import validate_tool_arguments

            tool_arguments = validate_tool_arguments(arguments.tool, arguments.arguments)
            method_name = "apply_plan_handoff" if arguments.tool == "apply_plan" else arguments.tool
            result = getattr(_service(), method_name)(**tool_arguments)
            print(json.dumps(result, indent=2, sort_keys=True))
            return 0
        from .agent_session import launch_agent_session

        print(json.dumps({"pid": launch_agent_session(), "session": "agent"}, sort_keys=True))
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
    if arguments.command == "plan" and arguments.plan_command == "publish-copy-native":
        service = _service()
        receipt = service.commit_native_overlay(
            arguments.plan_id, arguments.revision, arguments.destination,
        )
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
    if arguments.command == "uninstall":
        from .installation import uninstall

        result = uninstall()
        print(json.dumps(result, indent=2, sort_keys=True))
        return 1 if result["conflicts"] else 0
    if arguments.command == "lok":
        from .lok_spike import render_workbook, status

        if arguments.lok_command == "status":
            result = status()
            if arguments.json:
                print(json.dumps(result, indent=2, sort_keys=True))
            else:
                for check in result["checks"]:
                    print(f"{'ok' if check['ok'] else 'missing':7} {check['name']}: {check['detail']}")
                print("native engine ready" if result["ready"] else "native engine not ready")
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


if __name__ == "__main__":
    raise SystemExit(main())
