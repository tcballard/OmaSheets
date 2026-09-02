#!/usr/bin/env python3
"""Generate a deterministic Excel-syntax workbook for the M0 corpus gate."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import zipfile

MIN_ROWS = 1
MAX_ROWS = 100_000
FIXED_TIMESTAMP = (1980, 1, 1, 0, 0, 0)


def bounded_rows(value: str) -> int:
    try:
        rows = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("rows must be an integer") from exc
    if not MIN_ROWS <= rows <= MAX_ROWS:
        raise argparse.ArgumentTypeError(
            f"rows must be between {MIN_ROWS} and {MAX_ROWS}",
        )
    return rows


def member(name: str, payload: str) -> tuple[zipfile.ZipInfo, bytes]:
    info = zipfile.ZipInfo(name, FIXED_TIMESTAMP)
    info.compress_type = zipfile.ZIP_DEFLATED
    info.create_system = 3
    info.external_attr = 0o100600 << 16
    return info, payload.encode("utf-8")


def worksheet(rows: int) -> str:
    records = [
        '<row r="1">'
        '<c r="A1" t="inlineStr"><is><t>Left</t></is></c>'
        '<c r="B1" t="inlineStr"><is><t>Right</t></is></c>'
        '<c r="C1" t="inlineStr"><is><t>Total</t></is></c>'
        "</row>",
    ]
    for index in range(1, rows + 1):
        row = index + 1
        left = index
        right = index * 2
        total = left + right
        records.append(
            f'<row r="{row}">'
            f'<c r="A{row}"><v>{left}</v></c>'
            f'<c r="B{row}"><v>{right}</v></c>'
            f'<c r="C{row}"><f>A{row}+B{row}</f><v>{total}</v></c>'
            "</row>"
        )
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        f'<dimension ref="A1:C{rows + 1}"/>'
        '<sheetViews><sheetView workbookViewId="0"/></sheetViews>'
        '<sheetFormatPr defaultRowHeight="15"/>'
        f'<sheetData>{"".join(records)}</sheetData>'
        '</worksheet>'
    )


def payloads(rows: int) -> list[tuple[str, str]]:
    return [
        (
            "[Content_Types].xml",
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
            '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
            '</Types>',
        ),
        (
            "_rels/.rels",
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'
            '</Relationships>',
        ),
        (
            "xl/workbook.xml",
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
            'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
            '<sheets><sheet name="M0" sheetId="1" r:id="rId1"/></sheets>'
            '<calcPr calcMode="auto" fullCalcOnLoad="1" forceFullCalc="1"/>'
            '</workbook>',
        ),
        (
            "xl/_rels/workbook.xml.rels",
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
            '</Relationships>',
        ),
        ("xl/worksheets/sheet1.xml", worksheet(rows)),
    ]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as workbook:
        for chunk in iter(lambda: workbook.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def generate(output: Path, rows: int) -> dict[str, object]:
    output.parent.mkdir(parents=True, exist_ok=True)
    created = False
    try:
        with zipfile.ZipFile(
            output,
            mode="x",
            compression=zipfile.ZIP_DEFLATED,
            compresslevel=9,
            strict_timestamps=True,
        ) as archive:
            created = True
            for name, payload in payloads(rows):
                info, encoded = member(name, payload)
                archive.writestr(info, encoded, compresslevel=9)
    except Exception:
        if created:
            output.unlink(missing_ok=True)
        raise

    os.chmod(output, 0o600)

    return {
        "schema": 1,
        "file": output.name,
        "sha256": sha256(output),
        "bytes": output.stat().st_size,
        "data_rows": rows,
        "header_cells": 3,
        "numeric_value_cells": rows * 2,
        "formula_cells": rows,
        "logical_cells": (rows + 1) * 3,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--rows", type=bounded_rows, default=100)
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
    report = generate(arguments.output, arguments.rows)
    print(json.dumps(report, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
