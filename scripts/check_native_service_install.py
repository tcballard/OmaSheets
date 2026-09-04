#!/usr/bin/env python3
"""Exercise the installed native service through its authenticated CLI."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import time
import zipfile


def write_fixture(path: Path) -> None:
    parts = {
        "[Content_Types].xml": """<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>""",
        "_rels/.rels": """<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>""",
        "xl/workbook.xml": """<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>""",
        "xl/_rels/workbook.xml.rels": """<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>""",
        "xl/worksheets/sheet1.xml": """<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="b"><v>1</v></c><c r="B1"><f>IF(A1,4,0)</f><v>4</v></c></row><row r="2"><c r="A2"><v>3</v></c><c r="B2"><v>6</v></c></row></sheetData></worksheet>""",
    }
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as workbook:
        for name, content in parts.items():
            workbook.writestr(name, content)


def call(service: Path, runtime: Path, request: dict, *, ok: bool = True) -> dict:
    completed = subprocess.run(
        [service, "call", "--runtime-dir", runtime, json.dumps(request, separators=(",", ":"))],
        text=True,
        capture_output=True,
        check=False,
        timeout=10,
    )
    expected_status = 0 if ok else 2
    if completed.returncode != expected_status:
        raise RuntimeError(
            f"native service call exited {completed.returncode}, expected {expected_status}: "
            f"{completed.stderr.strip()}"
        )
    envelope = json.loads(completed.stdout)
    if envelope.get("ok") is not ok:
        raise RuntimeError(f"native service envelope disagrees with expected status: {envelope}")
    return envelope


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--service", type=Path, required=True)
    parser.add_argument("--workdir", type=Path, required=True)
    arguments = parser.parse_args(argv)
    service = arguments.service.resolve(strict=True)
    workdir = arguments.workdir.resolve(strict=True)
    runtime = workdir / "runtime"
    runtime.mkdir(mode=0o700)
    source = workdir / "source.xlsx"
    document = workdir / "workflow.omasheets"
    output = workdir / "workflow.csv"
    xlsx_output = workdir / "workflow.xlsx"
    write_fixture(source)

    server = subprocess.Popen(
        [service, "serve", "--runtime-dir", runtime],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        socket = runtime / "omasheets/native.sock"
        for _ in range(200):
            if socket.is_socket():
                break
            if server.poll() is not None:
                _, stderr = server.communicate()
                raise RuntimeError(f"native service exited before listening: {stderr.strip()}")
            time.sleep(0.025)
        else:
            raise RuntimeError("native service did not create its socket")

        imported = call(service, runtime, {
            "kind": "import_xlsx", "source": str(source), "output": str(document),
            "actor": {"kind": "human", "id": "installed-ci"}, "name": "Installed workflow",
        })["response"]
        assert imported["formula_cells_observed"] == 1
        assert imported["formula_cells_native"] == 1
        assert imported["formula_cells_cached_only"] == 0
        assert imported["formula_cells_omitted"] == 0
        sheet = imported["sheets"][0]["id"]

        append = lambda branch, actor, command: call(service, runtime, {
            "kind": "append", "path": str(document), "branch": branch,
            "actor": actor, "command": command,
        })
        human = {"kind": "human", "id": "installed-ci"}
        agent = {"kind": "agent", "id": "installed-agent"}
        append(None, human, {
            "command": "add_check", "name": "ready", "sheet": sheet, "a1": "A1",
            "severity": "error", "message": "A1 must be true",
        })
        call(service, runtime, {
            "kind": "branch", "path": str(document), "name": "review",
            "actor": agent,
        })
        append("review", agent, {
            "command": "set_value", "sheet": sheet, "a1": "A1",
            "value": {"type": "boolean", "value": False},
        })
        failed_check = call(service, runtime, {
            "kind": "check", "path": str(document), "branch": "review",
        })["response"]
        assert failed_check["passed"] is False
        refused = call(service, runtime, {
            "kind": "merge", "path": str(document), "source": "review",
            "approver": human,
        }, ok=False)
        assert refused["error"]["code"] == "checks_failed"

        append("review", agent, {
            "command": "set_value", "sheet": sheet, "a1": "A1",
            "value": {"type": "boolean", "value": True},
        })
        append("review", agent, {
            "command": "set_value", "sheet": sheet, "a1": "B2",
            "value": {"type": "number", "value": 8},
        })
        passed_check = call(service, runtime, {
            "kind": "check", "path": str(document), "branch": "review",
        })["response"]
        assert passed_check["passed"] is True
        diff = call(service, runtime, {
            "kind": "diff", "path": str(document), "source": "review",
        })["response"]
        assert len(diff["source_operations"]) == 3
        call(service, runtime, {
            "kind": "merge", "path": str(document), "source": "review",
            "approver": human,
        })
        exported = call(service, runtime, {
            "kind": "export_csv", "path": str(document), "sheet": sheet,
            "output": str(output),
        })["response"]
        assert exported["formula_cells"] == 1
        assert output.read_text() == "TRUE,4\n3,8" or output.read_bytes() == b"TRUE,4\r\n3,8"
        xlsx_exported = call(service, runtime, {
            "kind": "export_xlsx", "path": str(document),
            "output": str(xlsx_output),
        })["response"]
        assert xlsx_exported["formula_cells"] == 1
        assert xlsx_exported["formula_cells_preserved"] == 1
        assert xlsx_exported["formula_cells_flattened"] == 0
        with zipfile.ZipFile(xlsx_output) as package:
            worksheet = package.read("xl/worksheets/sheet1.xml")
        assert b"<f>IF(A1,4,0)</f><v>4</v>" in worksheet

        before = call(service, runtime, {
            "kind": "document", "path": str(document),
        })["response"]
        call(service, runtime, {"kind": "close", "path": str(document)})
        after = call(service, runtime, {
            "kind": "document", "path": str(document),
        })["response"]
        assert after["digest"] == before["digest"] == exported["document_digest"]
        assert after["digest"] == xlsx_exported["document_digest"]
        call(service, runtime, {"kind": "close", "path": str(document)})
        print(json.dumps({
            "schema": 1,
            "digest": after["digest"],
            "events": after["event_count"],
            "formula_cells_native": imported["formula_cells_native"],
            "checks_exercised": 2,
            "merge_refusal_exercised": True,
            "csv_bytes": output.stat().st_size,
            "xlsx_bytes": xlsx_output.stat().st_size,
        }, sort_keys=True))
        return 0
    finally:
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=5)


if __name__ == "__main__":
    raise SystemExit(main())
