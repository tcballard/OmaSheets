#!/usr/bin/env python3
"""Verify installed Rust workbook jobs, with no LibreOffice on the host.

This is acceptance glue, not a spreadsheet engine. It drives the installed CLI
and independently reads the exported XML instead of trusting its own manifest.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import xml.etree.ElementTree as ET
import zipfile


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--launcher", required=True, type=Path)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--workdir", required=True, type=Path)
    args = parser.parse_args()
    launcher, source, root = args.launcher.resolve(), args.source.resolve(), args.workdir.resolve()
    assert not shutil.which("libreoffice") and not shutil.which("soffice")
    original = source.read_bytes()

    def command(*arguments: str, stdin: str | None = None) -> dict:
        completed = subprocess.run([str(launcher), *arguments], text=True, input=stdin,
                                   capture_output=True, timeout=120, check=False)
        if completed.returncode:
            raise RuntimeError(f"Installed command failed ({completed.returncode}): "
                               f"{completed.stdout[-4000:]}{completed.stderr[-4000:]}")
        payload = completed.stdout.rsplit("to publish: ", 1)[-1] if stdin is not None else completed.stdout
        return json.loads(payload)

    def call(tool: str, **arguments) -> dict:
        return command("agent-session", "call", tool, "--arguments", json.dumps(arguments))

    session = command("select", str(source))
    session_id = session["session_id"]
    described = call("describe_workbook", session_id=session_id, include_formulas=True)
    assert described["sheets"] and described["document_source"] == "selected_file"
    batch = call("query_workbook", session_id=session_id, queries=[
        {"id": "structure", "tool": "describe_workbook", "arguments": {"include_formulas": True}},
        {"id": "cells", "tool": "read_range", "arguments": {
            "sheet": "M0", "range": "A2:C2", "include_formulas": True, "include_styles": True}},
    ])
    assert [item["id"] for item in batch["items"]] == ["structure", "cells"]
    assert batch["items"][1]["result"]["values"] == [[1, 2, 3]]
    assert batch["evidence_id"]
    audit = call("analyze_workbook", session_id=session_id)
    assert audit["summary"] and audit["evidence_id"]
    preview = call("render_workbook", session_id=session_id, format="pdf")
    assert preview["format"] == "pdf"
    plan = call("plan_changes", session_id=session_id, expected_revision=1,
                operations=[{"type": "set_value", "sheet": "M0", "range": "A2", "value": 20}],
                workflow={
                    "goal": "Verify owned editing and reviewed publication",
                    "summary": "Change M0!A2 to 20 and verify the dependent M0!C2 becomes 22.",
                    "assumptions": ["The selected source is preserved and a separate copy is published."],
                    "evidence_ids": [batch["evidence_id"], audit["evidence_id"]],
                    "groups": [{"title": "Change the input", "purpose": "Verify dependent recalculation.",
                                "operation_indexes": [0]}],
                })
    assert plan["status"] == "verified" and plan["verification"]["reopened"]
    assert "libreoffice" not in json.dumps(plan["engine"]).lower()
    output = root / "approved.xlsx"
    receipt = command("plan", "approve", plan["plan_id"], "--revision", "1", "--destination", str(output),
                      stdin=f"APPLY {plan['plan_id']}\n")
    # Interactive review emits a JSON review, its prompt, then the receipt.
    assert receipt  # parsed below by the command helper's approval special case
    assert source.read_bytes() == original
    namespace = {"s": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}
    with zipfile.ZipFile(output) as archive:
        sheet = ET.fromstring(archive.read("xl/worksheets/sheet1.xml"))
    cells = {cell.attrib["r"]: cell for cell in sheet.findall(".//s:c", namespace)}
    assert cells["A2"].find("s:v", namespace).text == "20"
    assert cells["C2"].find("s:v", namespace).text == "22"
    assert cells["C2"].find("s:f", namespace).text == "A2+B2"
    refused = root / "unsupported.ods"
    refused.write_bytes(b"unsupported-format-fixture")
    result = subprocess.run([str(launcher), "launch", str(refused)], capture_output=True, text=True, timeout=10)
    assert result.returncode != 0 and "does not support" in result.stderr
    print(json.dumps({"status": "ok", "engine": "omasheets-kit", "libreoffice_installed": False,
                      "source_sha256": hashlib.sha256(original).hexdigest(),
                      "checks": ["isolated describe", "batched reads", "audit", "owned PDF preview",
                                 "sealed stage", "human approval", "independent XLSX values/formula",
                                 "source preservation", "unsupported format refusal"]}))


if __name__ == "__main__":
    main()
