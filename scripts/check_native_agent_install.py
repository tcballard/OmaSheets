#!/usr/bin/env python3
"""Exercise the actual service and bounded agent bridge in an isolated runtime."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess

from omasheets import native_agent


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--service", type=Path, required=True)
    parser.add_argument("--document", type=Path, required=True)
    args = parser.parse_args()
    path = args.document.resolve()
    if path.exists():
        raise SystemExit("The fixture destination must not exist")
    runtime = Path(os.environ["XDG_RUNTIME_DIR"])
    context = runtime / "omasheets/native-agent-session.json"
    if context.exists():
        raise SystemExit("Use an isolated runtime without an active agent session")
    actor = {"kind": "human", "id": "native-workflow-check"}

    def service(kind: str, **arguments):
        result = subprocess.run([str(args.service.resolve()), "call", "--runtime-dir", str(runtime),
                                 json.dumps({"kind": kind, "path": str(path), **arguments})],
                                capture_output=True, text=True, timeout=35, check=True)
        answer = json.loads(result.stdout)
        assert answer["ok"], answer
        return answer["response"]

    service("create", name="Agent review example", actor=actor)
    service("append", actor=actor, command={"command": "add_sheet", "name": "Forecast"})
    sheet = service("document")["sheets"][0]["id"]
    service("append_batch", actor=actor, commands=[
        {"command": "add_columns", "sheet": sheet, "count": 8, "at": 0},
        {"command": "add_rows", "sheet": sheet, "count": 100, "at": 0, "table": None},
        {"command": "set_value", "sheet": sheet, "a1": "A1", "value": {"type": "text", "value": "Forecast"}},
        {"command": "set_value", "sheet": sheet, "a1": "A2", "value": {"type": "text", "value": "Unit cost"}},
        {"command": "set_value", "sheet": sheet, "a1": "B2", "value": {"type": "number", "value": 18}},
        {"command": "set_value", "sheet": sheet, "a1": "A3", "value": {"type": "text", "value": "Quantity"}},
        {"command": "set_value", "sheet": sheet, "a1": "B3", "value": {"type": "number", "value": 5}},
        {"command": "set_value", "sheet": sheet, "a1": "A4", "value": {"type": "text", "value": "Total"}},
        {"command": "set_formula", "sheet": sheet, "a1": "B4", "source": "=B2*B3"},
        {"command": "set_formula", "sheet": sheet, "a1": "B5", "source": "=B4<150"},
        {"command": "add_check", "sheet": sheet, "a1": "B5", "name": "Budget ceiling", "severity": "error", "message": "Forecast must stay below 150"},
    ])
    selected = {"schema": 1, "pid": os.getpid(), "session_id": "a" * 32, "path": str(path),
                "selection": {"sheet": sheet, "row": 1, "column": 1, "rows": 1, "columns": 1}}
    descriptor = os.open(context, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump(selected, output)

    def call(name, **arguments):
        return native_agent.call(name, {"session_id": selected["session_id"], **arguments})

    def propose(value):
        overview = call("native_overview")
        return call("native_propose", expected_revision=overview["revision"], goal="Update the unit-cost forecast",
                    explanation="The revised unit cost changes the forecast total while keeping the quantity and budget ceiling.",
                    assumptions=["Quantity remains five units."], evidence=[f"Forecast!B2 contains {service('cell', sheet=sheet, a1='B2')['value']['value']}; Forecast!B4 multiplies cost by quantity."],
                    edits=[{"sheet": sheet, "a1": "B2", "value": str(value)}])["branch"]

    try:
        resource = native_agent.resource()
        assert resource and str(path) not in json.dumps(resource)
        assert call("native_read", sheet=sheet, row=0, column=0, rows=5, columns=2)["cells"]
        assert call("native_lineage", sheet=sheet, a1="B4")["inputs"]
        before = service("document")["digest"]
        branch = propose(20)
        assert service("document")["digest"] == before
        review = call("native_review", branch=branch)
        assert review["can_approve"] and any(change["after"]["a1"] == "B4" for change in review["cells"])
        service("approve_native", source=branch, source_revision=review["source_revision"], target_revision=review["target_revision"])
        approved = service("document")["digest"]
        service("close")
        assert service("document")["digest"] == approved
        assert service("cell", sheet=sheet, a1="B4")["value"] == {"type": "number", "value": 100.0}
        branch = propose(21)
        review = call("native_review", branch=branch)
        service("reject_native", source=branch, source_revision=review["source_revision"], reason="Keep the agreed forecast")
        service("close")
        assert service("document")["digest"] == approved
        assert call("native_review", branch=branch)["status"] == "rejected"
        pending = propose(22)
        service("snapshot")
        print(json.dumps({"status": "ok", "review_branch": pending,
                          "checks": ["bounded resource/read/lineage", "atomic proposal", "derived changes", "human approval", "reopen", "durable rejection"]}))
    finally:
        native_agent.clear()


if __name__ == "__main__":
    main()
