#!/usr/bin/env python3
"""Generate deterministic sparse XLSX fixtures and check the bounded owned lane."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import zipfile

from scripts import generate_m0_xlsx, sample_corpus

# Every expected value is a saved cell cache, checked independently of OmaSheets.
CASES = {
    "sparse-grid": (
        '<row r="1"><c r="A1"><v>7</v></c></row>'
        '<row r="1048576"><c r="XFD1048576"><f>A1+1</f><v>8</v></c></row>',
        {"A1": 7, "XFD1048576": 8}, 1,
    ),
    "cached-formula": (
        '<row r="1"><c r="A1"><f>1+1</f><v>2</v></c></row>',
        {"A1": 2}, 1,
    ),
    "styled-empty": (
        '<row r="1"><c r="A1"><f>1+1</f><v>2</v></c></row>'
        '<row r="1048576"><c r="XFD1048576" s="0"/></row>',
        {"A1": 2, "XFD1048576": None}, 1,
    ),
    "shared-anchor": (
        '<row r="1"><c r="A1"><f t="shared" si="0"/><v>12</v></c>'
        '<c r="B1"><f t="shared" si="0" ref="A1:B2">C1+1</f><v>11</v></c>'
        '<c r="C1"><v>10</v></c></row>'
        '<row r="2"><c r="A2"><f t="shared" si="0"/><v>22</v></c>'
        '<c r="B2"><f t="shared" si="0"/><v>21</v></c>'
        '<c r="C2"><v>20</v></c></row>',
        {"A1": 12, "B1": 11, "C1": 10, "A2": 22, "B2": 21, "C2": 20}, 4,
    ),
}


def generate(output: Path) -> list[dict]:
    output.mkdir(parents=True, exist_ok=False)
    root = output / "workbooks"
    root.mkdir()
    for name, (rows, _, _) in CASES.items():
        worksheet = (
            '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
            '<dimension ref="A1:XFD1048576"/>'
            f'<sheetData>{rows}</sheetData></worksheet>'
        )
        with zipfile.ZipFile(root / f"{name}.xlsx", "x") as archive:
            for member_name, text in generate_m0_xlsx.payloads(1):
                if member_name == "xl/worksheets/sheet1.xml":
                    text = worksheet
                info, encoded = generate_m0_xlsx.member(member_name, text)
                # Stored entries make the bytes independent of the zlib version.
                info.compress_type = zipfile.ZIP_STORED
                archive.writestr(info, encoded)
    entries, _ = sample_corpus.sample(root, len(CASES), "import")
    sample_corpus.write_manifest(entries, output / "manifest.jsonl")
    return entries


def check_independent_reader(root: Path) -> None:
    import openpyxl

    for name, (_, cells, _) in CASES.items():
        workbook = openpyxl.load_workbook(root / f"{name}.xlsx", data_only=True)
        try:
            sheet = workbook["M0"]
            for address, expected in cells.items():
                if sheet[address].value != expected:
                    raise ValueError(f"independent reader disagrees on {name} {address}")
        finally:
            workbook.close()


def check_score(score: dict, entries: list[dict]) -> dict:
    expected = {entry["id"]: entry for entry in entries}
    reports = score["entries"]
    if len(reports) != len(expected) or {entry["id"] for entry in reports} != set(expected):
        raise ValueError("scorer did not report every generated workbook exactly once")
    for entry in reports:
        manifest_entry = expected[entry["id"]]
        name = Path(manifest_entry["path"]).stem
        if entry["sha256"] != manifest_entry["sha256"] or entry["owned_status"] != "ok":
            raise ValueError(f"owned import failed for {name}: {entry.get('owned_error')}")
        report = entry["owned"]["report"]
        formulas = CASES[name][2]
        for field in ("formula_cells_observed", "formula_cells_loaded",
                      "formula_cells_compared", "stored_values_matched"):
            if report[field] != formulas:
                raise ValueError(f"{name}: {field} was {report[field]}, expected {formulas}")
        if report["stored_values_mismatched"] or report["unsupported_formulas"]:
            raise ValueError(f"{name}: formulas disagreed or were refused")
    return {
        "workbooks": len(reports),
        "owned": score["owned_summary"],
        "candidate": score["summary"],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new evidence directory, never overwritten")
    parser.add_argument("--corpus-bin", type=Path)
    parser.add_argument("--independent-reader", action="store_true", help="requires openpyxl")
    args = parser.parse_args(argv)
    entries = generate(args.output)
    if args.independent_reader:
        check_independent_reader(args.output / "workbooks")
    result = {"workbooks": len(entries)}
    if args.corpus_bin:
        score_path = args.output / "score.json"
        # The corpus executable enforces memory, time and output limits per lane.
        # Candidate failures stay visible but do not fail this owned-import gate.
        subprocess.run([
            str(args.corpus_bin.resolve()), "score", str(args.output / "manifest.jsonl"),
            str(args.output / "workbooks"), str(score_path), "--timeout-seconds", "10",
        ], check=True, timeout=100, stdout=subprocess.DEVNULL)
        result = check_score(json.loads(score_path.read_text()), entries)
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
