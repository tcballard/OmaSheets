#!/usr/bin/env python3
"""Generate a deterministic Excel-syntax workbook for the M0 corpus gate.

The default workbook holds one ``M0`` sheet of additions. ``--date-rows``
adds a ``Dates`` sheet whose stored values follow Excel's 1900 date system, so
the owned engine's date functions can be checked against cached results.
``--date-system 1904`` only flips the workbook's ``date1904`` flag; it exists
so the importer's explicit rejection of that system can be exercised.
"""

from __future__ import annotations

import argparse
import calendar
from datetime import date, timedelta
import hashlib
import json
import os
from pathlib import Path
import zipfile

MIN_ROWS = 1
MAX_ROWS = 100_000
MIN_DATE_ROWS = 0
MAX_DATE_ROWS = 10_000
DATE_SYSTEMS = ("1900", "1904")
DATE_FORMULAS_PER_ROW = 7
FIXED_TIMESTAMP = (1980, 1, 1, 0, 0, 0)
# Excel serial 0 is the fictitious 1900-01-00; for real dates on or after
# 1900-03-01 the serial is the day count from 1899-12-30. Every sampled date
# is later than that, so the Lotus leap-year quirk never enters this script.
SERIAL_EPOCH = date(1899, 12, 30)
DATE_STYLE = 1


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


def bounded_date_rows(value: str) -> int:
    try:
        rows = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("date rows must be an integer") from exc
    if not MIN_DATE_ROWS <= rows <= MAX_DATE_ROWS:
        raise argparse.ArgumentTypeError(
            f"date rows must be between {MIN_DATE_ROWS} and {MAX_DATE_ROWS}",
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


def serial(value: date) -> int:
    return (value - SERIAL_EPOCH).days


def add_months(value: date, months: int) -> date:
    """Excel EDATE: same day of month, clamped to the target month's length."""
    year, month_index = divmod(value.year * 12 + value.month - 1 + months, 12)
    month = month_index + 1
    return date(year, month, min(value.day, calendar.monthrange(year, month)[1]))


def end_of_month(value: date, months: int) -> date:
    """Excel EOMONTH: last day of the month ``months`` away."""
    shifted = add_months(value.replace(day=1), months)
    return shifted.replace(day=calendar.monthrange(shifted.year, shifted.month)[1])


def weekday(value: date) -> int:
    """Excel WEEKDAY return type 1: Sunday is 1, Saturday is 7."""
    return value.isoweekday() % 7 + 1


def sample_date(index: int) -> date:
    if index % 2 == 0:
        # Month ends exercise EDATE and EOMONTH clamping across leap years.
        return end_of_month(date(2023, 12, 31), index // 2)
    return date(2020, 1, 1) + timedelta(days=97 * index)


def date_worksheet(rows: int) -> str:
    records = [
        '<row r="1">'
        '<c r="A1" t="inlineStr"><is><t>Date</t></is></c>'
        '<c r="B1" t="inlineStr"><is><t>Year</t></is></c>'
        '<c r="C1" t="inlineStr"><is><t>Month</t></is></c>'
        '<c r="D1" t="inlineStr"><is><t>Day</t></is></c>'
        '<c r="E1" t="inlineStr"><is><t>NextMonth</t></is></c>'
        '<c r="F1" t="inlineStr"><is><t>NextMonthEnd</t></is></c>'
        '<c r="G1" t="inlineStr"><is><t>Rebuilt</t></is></c>'
        '<c r="H1" t="inlineStr"><is><t>Weekday</t></is></c>'
        "</row>",
    ]
    for index in range(rows):
        row = index + 2
        value = sample_date(index)
        records.append(
            f'<row r="{row}">'
            f'<c r="A{row}" s="{DATE_STYLE}"><v>{serial(value)}</v></c>'
            f'<c r="B{row}"><f>YEAR(A{row})</f><v>{value.year}</v></c>'
            f'<c r="C{row}"><f>MONTH(A{row})</f><v>{value.month}</v></c>'
            f'<c r="D{row}"><f>DAY(A{row})</f><v>{value.day}</v></c>'
            f'<c r="E{row}" s="{DATE_STYLE}"><f>EDATE(A{row},1)</f>'
            f"<v>{serial(add_months(value, 1))}</v></c>"
            f'<c r="F{row}" s="{DATE_STYLE}"><f>EOMONTH(A{row},1)</f>'
            f"<v>{serial(end_of_month(value, 1))}</v></c>"
            f'<c r="G{row}" s="{DATE_STYLE}"><f>DATE(B{row},C{row},D{row})</f>'
            f"<v>{serial(value)}</v></c>"
            f'<c r="H{row}"><f>WEEKDAY(A{row})</f><v>{weekday(value)}</v></c>'
            "</row>"
        )
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        f'<dimension ref="A1:H{rows + 1}"/>'
        '<sheetViews><sheetView workbookViewId="0"/></sheetViews>'
        '<sheetFormatPr defaultRowHeight="15"/>'
        f'<sheetData>{"".join(records)}</sheetData>'
        '</worksheet>'
    )


STYLES = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
    '<fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>'
    '<fills count="1"><fill><patternFill patternType="none"/></fill></fills>'
    '<borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>'
    '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>'
    '<cellXfs count="2">'
    '<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>'
    '<xf numFmtId="14" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>'
    "</cellXfs>"
    "</styleSheet>"
)


def payloads(
    rows: int,
    date_rows: int = 0,
    date_system: str = "1900",
) -> list[tuple[str, str]]:
    if date_system not in DATE_SYSTEMS:
        raise ValueError(f"unsupported date system {date_system!r}")
    with_dates = date_rows > 0
    content_overrides = (
        '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
        '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
    )
    sheets = '<sheet name="M0" sheetId="1" r:id="rId1"/>'
    workbook_relationships = (
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
    )
    if with_dates:
        content_overrides += (
            '<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
            '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>'
        )
        sheets += '<sheet name="Dates" sheetId="2" r:id="rId2"/>'
        workbook_relationships += (
            '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>'
            '<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>'
        )
    workbook_properties = '<workbookPr date1904="1"/>' if date_system == "1904" else ""
    members = [
        (
            "[Content_Types].xml",
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="xml" ContentType="application/xml"/>'
            f"{content_overrides}"
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
            f"{workbook_properties}"
            f"<sheets>{sheets}</sheets>"
            '<calcPr calcMode="auto" fullCalcOnLoad="1" forceFullCalc="1"/>'
            '</workbook>',
        ),
        (
            "xl/_rels/workbook.xml.rels",
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            f"{workbook_relationships}"
            '</Relationships>',
        ),
        ("xl/worksheets/sheet1.xml", worksheet(rows)),
    ]
    if with_dates:
        members.append(("xl/worksheets/sheet2.xml", date_worksheet(date_rows)))
        members.append(("xl/styles.xml", STYLES))
    return members


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as workbook:
        for chunk in iter(lambda: workbook.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def generate(
    output: Path,
    rows: int,
    date_rows: int = 0,
    date_system: str = "1900",
) -> dict[str, object]:
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
            for name, payload in payloads(rows, date_rows, date_system):
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
        "header_cells": 3 + (8 if date_rows else 0),
        "numeric_value_cells": rows * 2,
        "date_rows": date_rows,
        "date_system": date_system,
        "date_value_cells": date_rows,
        "date_formula_cells": date_rows * DATE_FORMULAS_PER_ROW,
        "formula_cells": rows + date_rows * DATE_FORMULAS_PER_ROW,
        "logical_cells": (rows + 1) * 3 + ((date_rows + 1) * 8 if date_rows else 0),
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--rows", type=bounded_rows, default=100)
    parser.add_argument("--date-rows", type=bounded_date_rows, default=0)
    parser.add_argument("--date-system", choices=DATE_SYSTEMS, default="1900")
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
    report = generate(
        arguments.output,
        arguments.rows,
        arguments.date_rows,
        arguments.date_system,
    )
    print(json.dumps(report, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
