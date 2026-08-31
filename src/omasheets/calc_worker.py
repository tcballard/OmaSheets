"""Single-job LibreOffice UNO worker. Runs only inside the Calc sandbox."""

from __future__ import annotations

import json
import math
import re
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any

FORMULA_ERRORS = {501, 502, 503, 504, 509, 510, 511, 512, 513, 514, 515, 516, 517, 518, 519, 520, 521, 522, 523, 524, 525, 526, 527, 532}
REFERENCE = re.compile(r"(?:'([^']+)'|([A-Za-z_][^.!]*))?[.!]?(\$?[A-Z]{1,3}\$?[1-9][0-9]{0,6}(?::\$?[A-Z]{1,3}\$?[1-9][0-9]{0,6})?)")
STARTUP_PATH = re.compile(r"(?<![A-Za-z0-9._-])/(?:[^\s:]+/)*[^\s:]*")
STARTUP_URL = re.compile(r"[A-Za-z][A-Za-z0-9+.-]*://\S+")


def _property(name: str, value: Any):
    from com.sun.star.beans import PropertyValue

    item = PropertyValue()
    item.Name = name
    item.Value = value
    return item


def _url(path: Path) -> str:
    import uno

    return uno.systemPathToFileUrl(str(path.resolve()))


def _startup_diagnostic(path: Path) -> str:
    """Return one bounded, path-free LibreOffice startup diagnostic."""

    try:
        text = path.read_text(encoding="utf-8", errors="replace")[-4096:]
    except OSError:
        return ""
    lines = []
    for raw_line in text.splitlines():
        line = "".join(character if character.isprintable() else " " for character in raw_line).strip()
        if line:
            lines.append(line)
    if not lines:
        return ""
    detail = STARTUP_PATH.sub("<path>", STARTUP_URL.sub("<url>", lines[-1]))
    return detail[:256]


def _connect(soffice: str, profile: Path):
    import uno

    pipe = f"omasheets-{uuid.uuid4().hex}"
    diagnostic_path = profile.parent / "soffice-startup.log"
    with diagnostic_path.open("wb") as diagnostic:
        process = subprocess.Popen(
            [
                soffice,
                "--headless",
                "--nologo",
                "--nodefault",
                "--norestore",
                "--nofirststartwizard",
                "--nolockcheck",
                f"-env:UserInstallation={_url(profile)}",
                f"--accept=pipe,name={pipe};urp;StarOffice.ComponentContext",
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=diagnostic,
        )
    local = uno.getComponentContext()
    resolver = local.ServiceManager.createInstanceWithContext("com.sun.star.bridge.UnoUrlResolver", local)
    for _ in range(100):
        if process.poll() is not None:
            detail = _startup_diagnostic(diagnostic_path)
            suffix = f": {detail}" if detail else ""
            raise RuntimeError(f"LibreOffice exited before opening its private pipe{suffix}")
        try:
            context = resolver.resolve(f"uno:pipe,name={pipe};urp;StarOffice.ComponentContext")
            return process, context
        except Exception:
            time.sleep(0.05)
    process.terminate()
    raise RuntimeError("timed out connecting to LibreOffice")


def _load(context, path: Path, *, read_only: bool):
    desktop = context.ServiceManager.createInstanceWithContext("com.sun.star.frame.Desktop", context)
    properties = (
        _property("Hidden", True),
        _property("ReadOnly", read_only),
        _property("MacroExecutionMode", 0),
        _property("UpdateDocMode", 0),
        _property("Silent", True),
    )
    document = desktop.loadComponentFromURL(_url(path), "_blank", 0, properties)
    if document is None or not document.supportsService("com.sun.star.sheet.SpreadsheetDocument"):
        raise RuntimeError("input is not a Calc spreadsheet")
    return document


def _used_range(sheet):
    cursor = sheet.createCursor()
    cursor.gotoEndOfUsedArea(True)
    address = cursor.getRangeAddress()
    return address, sheet.getCellRangeByPosition(0, 0, address.EndColumn, address.EndRow)


def _formula_errors(sheet_name: str, area, formulas, values, limit: int) -> list[dict[str, Any]]:
    errors = []
    for row_index, row in enumerate(formulas):
        for column_index, formula in enumerate(row):
            if not formula.startswith("="):
                continue
            cell = area.getCellByPosition(column_index, row_index)
            code = int(cell.getError())
            displayed = str(cell.getString())
            if code or displayed.startswith("#"):
                errors.append({
                    "sheet": sheet_name,
                    "row": row_index + 1,
                    "column": column_index + 1,
                    "formula": formula,
                    "error_code": code,
                    "displayed": displayed,
                })
                if len(errors) >= limit:
                    return errors
    return errors


def _named_ranges(document, limit: int) -> dict[str, Any]:
    ranges = getattr(document, "NamedRanges", None)
    if ranges is None:
        ranges = document.getNamedRanges()
    names = sorted(ranges.getElementNames())
    result = []
    for name in names[:limit]:
        item = ranges.getByName(name)
        content = str(item.getContent())
        if re.search(r"(?i)(?:file:|[a-z][a-z0-9+.-]*://)", content):
            result.append({"name": name, "content_redacted": True})
        else:
            result.append({"name": name, "content": content, "content_redacted": False})
    return {"items": result, "truncated": len(names) > limit, "total": len(names)}


def _inspect(document, limits: dict[str, int], *, include_formulas: bool) -> dict[str, Any]:
    sheets = document.getSheets()
    names = list(sheets.getElementNames())
    if len(names) > limits["max_sheets"]:
        raise RuntimeError("workbook exceeds the sheet limit")
    result_sheets = []
    formula_records = []
    errors = []
    total_cells = 0
    formula_count = 0
    for name in names:
        sheet = sheets.getByName(name)
        address, area = _used_range(sheet)
        rows = address.EndRow + 1
        columns = address.EndColumn + 1
        total_cells += rows * columns
        if total_cells > limits["max_cells"]:
            raise RuntimeError("workbook exceeds the inspected-cell limit")
        formulas = area.getFormulaArray()
        values = area.getDataArray()
        sheet_formula_count = sum(1 for row in formulas for formula in row if str(formula).startswith("="))
        formula_count += sheet_formula_count
        if formula_count > limits["max_formulas"]:
            raise RuntimeError("workbook exceeds the formula limit")
        errors.extend(_formula_errors(name, area, formulas, values, limits["max_results"] - len(errors)))
        if include_formulas:
            for row_index, row in enumerate(formulas):
                for column_index, formula in enumerate(row):
                    if str(formula).startswith("="):
                        formula_records.append({
                            "sheet": name,
                            "row": row_index + 1,
                            "column": column_index + 1,
                            "formula": str(formula),
                        })
        result_sheets.append({
            "name": name,
            "used_range": {
                "start_column": 1,
                "start_row": 1,
                "end_column": columns,
                "end_row": rows,
            },
            "rows": rows,
            "columns": columns,
            "formula_count": sheet_formula_count,
        })
    return {
        "sheets": result_sheets,
        "sheet_count": len(result_sheets),
        "inspected_cells": total_cells,
        "formula_count": formula_count,
        "formula_errors": errors,
        "formulas": formula_records if include_formulas else [],
        "named_ranges": _named_ranges(document, limits["max_results"]),
    }


def _column_name(index: int) -> str:
    result = ""
    number = index + 1
    while number:
        number, remainder = divmod(number - 1, 26)
        result = chr(65 + remainder) + result
    return result


def _object_inventory(document) -> dict[str, Any]:
    charts = []
    pivots = []
    truncated = False
    for sheet_name in document.getSheets().getElementNames():
        sheet = document.getSheets().getByName(sheet_name)
        table_charts = sheet.getCharts()
        for name in sorted(table_charts.getElementNames()):
            if len(charts) >= 500:
                truncated = True
                break
            chart = table_charts.getByName(name)
            title = ""
            ranges = []
            column_headers = False
            row_headers = False
            try:
                embedded = chart.getEmbeddedObject()
                title = str(embedded.Title.String) if embedded.HasMainTitle else ""
                ranges = [[address.Sheet, address.StartColumn, address.StartRow, address.EndColumn, address.EndRow] for address in chart.getRanges()]
                column_headers = bool(chart.getHasColumnHeaders())
                row_headers = bool(chart.getHasRowHeaders())
            except Exception:
                pass
            charts.append({
                "sheet": sheet_name,
                "name": name,
                "title": title,
                "column_headers": column_headers,
                "row_headers": row_headers,
                "source_ranges": ranges,
            })
        tables = sheet.getDataPilotTables()
        for name in sorted(tables.getElementNames()):
            if len(pivots) >= 500:
                truncated = True
                break
            table = tables.getByName(name)
            try:
                source = table.getSourceRange()
                output = table.getOutputRange()
                pivots.append({
                    "sheet": sheet_name,
                    "name": name,
                    "source": [source.Sheet, source.StartColumn, source.StartRow, source.EndColumn, source.EndRow],
                    "output": [output.Sheet, output.StartColumn, output.StartRow, output.EndColumn, output.EndRow],
                })
            except Exception:
                pivots.append({"sheet": sheet_name, "name": name, "details_unavailable": True})
    return {"charts": charts, "pivots": pivots, "truncated": truncated}


def _object_fingerprints(document, operations: list[dict[str, Any]]) -> dict[str, Any]:
    inventory = _object_inventory(document)
    chart_keys = {(operation["sheet"], operation["name"]) for operation in operations if operation["type"] == "upsert_chart"}
    pivot_keys = {(operation["sheet"], operation["name"]) for operation in operations if operation["type"] in {"upsert_pivot", "refresh_pivot"}}
    return {
        "charts": [item for item in inventory["charts"] if (item["sheet"], item["name"]) in chart_keys],
        "pivots": [item for item in inventory["pivots"] if (item["sheet"], item["name"]) in pivot_keys],
    }


def _analyze(document, arguments: dict[str, Any], limits: dict[str, int]) -> dict[str, Any]:
    """Return a deterministic, bounded workbook-wide audit for any MCP agent."""

    maximum = min(int(arguments.get("max_findings", 50)), 100)
    focus = arguments.get("focus", "all")
    inspection = _inspect(document, limits, include_formulas=False)
    findings: list[dict[str, Any]] = []
    finding_total = 0
    profiles = []

    def add(severity: str, category: str, sheet: str, address: str, message: str, metrics: dict[str, Any]) -> None:
        nonlocal finding_total
        finding_total += 1
        if len(findings) < maximum:
            findings.append({
                "id": f"F{len(findings) + 1:03d}", "severity": severity, "category": category,
                "sheet": sheet, "range": address, "message": message, "metrics": metrics,
            })

    total_rows = 0
    for sheet_info in inspection["sheets"]:
        name = sheet_info["name"]
        sheet = document.getSheets().getByName(name)
        address, area = _used_range(sheet)
        values = [list(row) for row in area.getDataArray()]
        formulas = [list(row) for row in area.getFormulaArray()]
        rows = len(values)
        columns = len(values[0]) if values else 0
        total_rows += max(rows - 1, 0)
        headers = [str(value).strip() for value in (values[0] if values else [])]
        if rows > 1 and any(not header for header in headers):
            blanks = [_column_name(index) for index, header in enumerate(headers) if not header]
            add("warning", "missing_header", name, f"A1:{_column_name(columns - 1)}1", "The table has blank header cells.", {"blank_columns": blanks[:20]})
        normalized_headers = [header.casefold() for header in headers if header]
        duplicates = sorted({header for header in normalized_headers if normalized_headers.count(header) > 1})
        if duplicates:
            add("warning", "duplicate_header", name, f"A1:{_column_name(columns - 1)}1", "The table has duplicate column names.", {"headers": duplicates[:20]})

        seen_rows: dict[str, int] = {}
        duplicate_examples = []
        duplicate_count = 0
        for row_index, row in enumerate(values[1:], start=2):
            key = json.dumps(row, sort_keys=True, default=str)
            if any(value not in ("", None) for value in row):
                if key in seen_rows:
                    duplicate_count += 1
                    if len(duplicate_examples) < 10:
                        duplicate_examples.append({"row": row_index, "matches_row": seen_rows[key]})
                else:
                    seen_rows[key] = row_index
        if duplicate_examples and focus in ("all", "quality"):
            add("warning", "duplicate_rows", name, f"A1:{_column_name(columns - 1)}{rows}", "Duplicate data rows may distort totals.", {"duplicate_count": duplicate_count, "examples": duplicate_examples})

        column_profiles = []
        for column in range(columns):
            data = [row[column] for row in values[1:]]
            populated = [value for value in data if value not in ("", None)]
            numeric = [float(value) for value in populated if isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(float(value))]
            blank_count = len(data) - len(populated)
            header = headers[column] or f"Column {_column_name(column)}"
            profile: dict[str, Any] = {
                "column": _column_name(column), "header": header, "populated": len(populated),
                "blanks": blank_count, "distinct": len({str(value) for value in populated[:10_000]}),
                "numeric": len(numeric), "formula_cells": sum(1 for row in formulas[1:] if str(row[column]).startswith("=")),
            }
            if numeric:
                ordered = sorted(numeric)
                profile.update({"min": ordered[0], "max": ordered[-1], "sum": sum(numeric), "mean": sum(numeric) / len(numeric)})
                if len(ordered) >= 8:
                    q1 = ordered[len(ordered) // 4]
                    q3 = ordered[(len(ordered) * 3) // 4]
                    low, high = q1 - 1.5 * (q3 - q1), q3 + 1.5 * (q3 - q1)
                    outliers = [value for value in numeric if value < low or value > high]
                    if outliers and focus in ("all", "quality", "management"):
                        add("notice", "numeric_outliers", name, f"{_column_name(column)}2:{_column_name(column)}{rows}", f"{header} contains values outside the IQR range.", {"count": len(outliers), "low": low, "high": high, "examples": outliers[:5]})
            if data and blank_count / len(data) >= 0.25 and focus in ("all", "quality"):
                add("notice", "sparse_column", name, f"{_column_name(column)}2:{_column_name(column)}{rows}", f"{header} is at least 25% blank.", {"blank_count": blank_count, "row_count": len(data)})
            column_profiles.append(profile)
        profiles.append({"sheet": name, "table_range": f"A1:{_column_name(max(columns - 1, 0))}{max(rows, 1)}", "headers": headers, "columns": column_profiles})

    for error in inspection["formula_errors"]:
        add("error", "formula_error", error["sheet"], f"{_column_name(error['column'] - 1)}{error['row']}", "A formula currently evaluates to an error.", {"displayed": error["displayed"], "error_code": error["error_code"]})
    objects = _object_inventory(document)
    opportunities = []
    for profile in profiles:
        numeric_headers = [column["header"] for column in profile["columns"] if column["numeric"] > 0]
        dimension_headers = [column["header"] for column in profile["columns"] if column["populated"] > 0 and column["numeric"] == 0]
        if numeric_headers:
            opportunities.append({
                "sheet": profile["sheet"], "source_range": profile["table_range"],
                "dimensions": dimension_headers[:4], "measures": numeric_headers[:8],
                "recommended_chart": "column" if dimension_headers else "line",
            })
    return {
        "focus": focus,
        "summary": {
            "sheet_count": inspection["sheet_count"], "inspected_cells": inspection["inspected_cells"],
            "data_rows": total_rows, "formula_count": inspection["formula_count"],
            "formula_error_count": len(inspection["formula_errors"]), "finding_count": len(findings),
            "finding_total": finding_total, "truncated": finding_total > len(findings),
        },
        "sheets": profiles, "findings": findings, "objects": objects,
        "management_summary_opportunities": opportunities[:20],
        "method": "deterministic_bounded_profile_v1",
    }


def _sheet(document, name: str):
    sheets = document.getSheets()
    if not sheets.hasByName(name):
        raise RuntimeError("sheet not found")
    return sheets.getByName(name)


def _style_color(value: int) -> str:
    return "automatic" if int(value) < 0 else f"#{int(value) & 0xFFFFFF:06X}"


def _cell_style(document, cell) -> dict[str, Any]:
    formats = document.getNumberFormats()
    properties = formats.getByKey(int(cell.NumberFormat))
    return {
        "style_name": str(cell.CellStyle),
        "number_format": str(properties.getPropertyValue("FormatString")),
        "bold": float(cell.CharWeight) >= 150.0,
        "text_color": _style_color(cell.CharColor),
        "background_color": _style_color(cell.CellBackColor),
        "wrap_text": bool(cell.IsTextWrapped),
    }


def _style_table(document, area, rows: int, columns: int) -> dict[str, Any]:
    styles: list[dict[str, Any]] = []
    indexes: dict[str, int] = {}
    style_ids = []
    for row in range(rows):
        output_row = []
        for column in range(columns):
            style = _cell_style(document, area.getCellByPosition(column, row))
            key = json.dumps(style, sort_keys=True, separators=(",", ":"))
            if key not in indexes:
                indexes[key] = len(styles)
                styles.append(style)
            output_row.append(indexes[key])
        style_ids.append(output_row)
    return {"styles": styles, "style_ids": style_ids}


def _read_range(document, arguments: dict[str, Any], limits: dict[str, int]) -> dict[str, Any]:
    area = _sheet(document, arguments["sheet"]).getCellRangeByName(arguments["range"])
    address = area.getRangeAddress()
    cells = (address.EndColumn - address.StartColumn + 1) * (address.EndRow - address.StartRow + 1)
    if cells > min(limits["max_cells"], 10_000):
        raise RuntimeError("requested range exceeds the read limit")
    result = {"sheet": arguments["sheet"], "range": arguments["range"], "values": area.getDataArray()}
    if arguments.get("include_formulas", True):
        result["formulas"] = area.getFormulaArray()
    if arguments.get("include_styles", False):
        if cells > 1_000:
            raise RuntimeError("styled range reads are limited to 1000 cells")
        result["style_table"] = _style_table(
            document,
            area,
            address.EndRow - address.StartRow + 1,
            address.EndColumn - address.StartColumn + 1,
        )
    return result


def _search(document, arguments: dict[str, Any], limits: dict[str, int]) -> dict[str, Any]:
    query = arguments["query"].casefold()
    scope = arguments.get("scope", "both")
    maximum = min(arguments.get("max_results", 50), limits["max_results"])
    matches = []
    for sheet_name in document.getSheets().getElementNames():
        sheet = document.getSheets().getByName(sheet_name)
        address, area = _used_range(sheet)
        values = area.getDataArray()
        formulas = area.getFormulaArray()
        for row in range(address.EndRow + 1):
            for column in range(address.EndColumn + 1):
                value = str(values[row][column])
                formula = str(formulas[row][column])
                matched = (scope in ("values", "both") and query in value.casefold()) or (
                    scope in ("formulas", "both") and query in formula.casefold()
                )
                if matched:
                    matches.append({
                        "sheet": sheet_name,
                        "row": row + 1,
                        "column": column + 1,
                        "value": value,
                        "formula": formula if formula.startswith("=") else None,
                    })
                    if len(matches) >= maximum:
                        return {"matches": matches, "truncated": True}
    return {"matches": matches, "truncated": False}


def _trace(document, arguments: dict[str, Any], limits: dict[str, int]) -> dict[str, Any]:
    sheet_name = arguments["sheet"]
    cell_name = arguments["cell"]
    cell = _sheet(document, sheet_name).getCellRangeByName(cell_name)
    formula = str(cell.getFormula())
    precedents = []
    if formula.startswith("="):
        for match in REFERENCE.finditer(formula):
            precedents.append({
                "sheet": match.group(1) or match.group(2) or sheet_name,
                "range": match.group(3).replace("$", ""),
            })
            if len(precedents) >= limits["max_results"]:
                break
    return {
        "root": {"sheet": sheet_name, "cell": cell_name, "formula": formula},
        "precedents": precedents if arguments.get("direction", "both") in ("precedents", "both") else [],
        "dependents": [],
        "max_depth": arguments.get("max_depth", 5),
        "warnings": ["v0.0.1 tracing resolves literal A1 precedents only; dynamic references and dependents may be incomplete"],
    }


def _color(value: str) -> int:
    return int(value.removeprefix("#"), 16)


def _number_format(document, format_code: str) -> int:
    from com.sun.star.lang import Locale

    formats = document.getNumberFormats()
    locale = Locale()
    key = formats.queryKey(format_code, locale, True)
    return key if key >= 0 else formats.addNew(format_code, locale)


def _matrix_values(values: list[list[Any]]) -> tuple[tuple[Any, ...], ...]:
    rows = []
    for row in values:
        output = []
        for value in row:
            if value is None:
                output.append("")
            elif isinstance(value, bool):
                output.append(1.0 if value else 0.0)
            elif isinstance(value, (int, float)):
                output.append(float(value))
            else:
                output.append(value)
        rows.append(tuple(output))
    return tuple(rows)


def _column_index(name: str) -> int:
    value = 0
    for character in name:
        value = value * 26 + ord(character) - ord("A") + 1
    return value - 1


def _fill_direction(name: str):
    import uno

    return uno.Enum("com.sun.star.sheet.FillDirection", name)


def _sort_field(column: int, ascending: bool):
    import uno

    field = uno.createUnoStruct("com.sun.star.util.SortField")
    field.Field = column
    field.SortAscending = ascending
    return field


def _sort(area, operation: dict[str, Any]) -> None:
    descriptor = area.createSortDescriptor()
    for item in descriptor:
        if item.Name == "ContainsHeader":
            item.Value = operation["has_header"]
        elif item.Name == "SortFields":
            item.Value = (_sort_field(operation["key_column"] - 1, operation["ascending"]),)
    area.sort(descriptor)


def _upsert_chart(document, operation: dict[str, Any]) -> None:
    import uno

    sheets = document.getSheets()
    target = sheets.getByName(operation["sheet"])
    source = sheets.getByName(operation["source_sheet"]).getCellRangeByName(operation["source_range"])
    anchor = target.getCellRangeByName(operation["anchor_range"])
    address = anchor.getRangeAddress()
    start = target.getCellByPosition(address.StartColumn, address.StartRow)
    end = target.getCellByPosition(address.EndColumn, address.EndRow)
    rectangle = uno.createUnoStruct("com.sun.star.awt.Rectangle")
    rectangle.X = int(start.Position.X)
    rectangle.Y = int(start.Position.Y)
    rectangle.Width = max(1000, int(end.Position.X + end.Size.Width - rectangle.X))
    rectangle.Height = max(1000, int(end.Position.Y + end.Size.Height - rectangle.Y))
    charts = target.getCharts()
    if charts.hasByName(operation["name"]):
        charts.removeByName(operation["name"])
    charts.addNewByName(
        operation["name"], rectangle, (source.getRangeAddress(),),
        operation["has_column_headers"], operation["has_row_headers"],
    )
    embedded = charts.getByName(operation["name"]).getEmbeddedObject()
    services = {
        "column": "com.sun.star.chart.BarDiagram", "bar": "com.sun.star.chart.BarDiagram",
        "line": "com.sun.star.chart.LineDiagram", "pie": "com.sun.star.chart.PieDiagram",
        "scatter": "com.sun.star.chart.XYDiagram",
    }
    diagram = embedded.createInstance(services[operation["chart_type"]])
    if operation["chart_type"] in {"column", "bar"}:
        diagram.Vertical = operation["chart_type"] == "column"
    embedded.setDiagram(diagram)
    embedded.HasMainTitle = bool(operation["title"])
    if operation["title"]:
        embedded.Title.String = operation["title"]
    embedded.HasLegend = operation["legend"]


def _pivot_field_map(descriptor) -> dict[str, Any]:
    fields = descriptor.getDataPilotFields()
    result = {}
    for index in range(fields.getCount()):
        field = fields.getByIndex(index)
        name = str(field.Name)
        result[name] = field
        result.setdefault(name.casefold(), field)
    return result


def _pivot_orientation(name: str):
    import uno

    return uno.Enum("com.sun.star.sheet.DataPilotFieldOrientation", name)


def _general_function(name: str):
    import uno

    return uno.Enum("com.sun.star.sheet.GeneralFunction", name)


def _upsert_pivot(document, operation: dict[str, Any]) -> None:
    sheets = document.getSheets()
    target = sheets.getByName(operation["sheet"])
    tables = target.getDataPilotTables()
    if tables.hasByName(operation["name"]):
        tables.removeByName(operation["name"])
    descriptor = tables.createDataPilotDescriptor()
    source = sheets.getByName(operation["source_sheet"]).getCellRangeByName(operation["source_range"])
    descriptor.setSourceRange(source.getRangeAddress())
    fields = _pivot_field_map(descriptor)

    def field(name: str):
        found = fields.get(name) or fields.get(name.casefold())
        if found is None:
            raise RuntimeError(f"pivot field not found: {name}")
        return found

    for name in operation["rows"]:
        field(name).Orientation = _pivot_orientation("ROW")
    for name in operation["columns"]:
        field(name).Orientation = _pivot_orientation("COLUMN")
    for name in operation["filters"]:
        field(name).Orientation = _pivot_orientation("PAGE")
    functions = {"sum": "SUM", "count": "COUNT", "average": "AVERAGE", "min": "MIN", "max": "MAX"}
    for value in operation["values"]:
        item = field(value["field"])
        item.Orientation = _pivot_orientation("DATA")
        item.Function = _general_function(functions[value["function"]])
        if value.get("label"):
            item.Name = value["label"]
    destination = target.getCellRangeByName(operation["output_cell"]).getCellAddress()
    tables.insertNewByName(operation["name"], destination, descriptor)


def _apply(document, operations: list[dict[str, Any]]) -> None:
    sheets = document.getSheets()
    for operation in operations:
        kind = operation["type"]
        if kind == "upsert_chart":
            _upsert_chart(document, operation)
        elif kind == "upsert_pivot":
            _upsert_pivot(document, operation)
        elif kind == "refresh_pivot":
            table = sheets.getByName(operation["sheet"]).getDataPilotTables().getByName(operation["name"])
            table.refresh()
        elif kind == "add_sheet":
            sheets.insertNewByName(operation["sheet"], sheets.getCount())
        elif kind == "delete_sheet":
            sheets.removeByName(operation["sheet"])
        elif kind == "rename_sheet":
            sheets.getByName(operation["sheet"]).setName(operation["new_name"])
        elif kind in {"insert_rows", "delete_rows"}:
            rows = sheets.getByName(operation["sheet"]).getRows()
            if kind == "insert_rows":
                rows.insertByIndex(operation["row"] - 1, operation["count"])
            else:
                rows.removeByIndex(operation["row"] - 1, operation["count"])
        elif kind in {"insert_columns", "delete_columns"}:
            columns = sheets.getByName(operation["sheet"]).getColumns()
            index = _column_index(operation["column"])
            if kind == "insert_columns":
                columns.insertByIndex(index, operation["count"])
            else:
                columns.removeByIndex(index, operation["count"])
        else:
            area = sheets.getByName(operation["sheet"]).getCellRangeByName(operation["range"])
            if kind == "clear_range":
                area.clearContents(1023)
            elif kind == "set_formula":
                area.setFormula(operation["formula"])
            elif kind == "set_value":
                value = operation["value"]
                if value is None:
                    area.clearContents(1023)
                elif isinstance(value, bool):
                    area.setValue(1 if value else 0)
                elif isinstance(value, (int, float)):
                    area.setValue(float(value))
                else:
                    area.setString(value)
            elif kind == "set_range_values":
                area.clearContents(1023)
                area.setDataArray(_matrix_values(operation["values"]))
            elif kind == "set_range_formulas":
                area.setFormulaArray(tuple(tuple(row) for row in operation["formulas"]))
            elif kind == "format_cells":
                if "number_format" in operation:
                    area.NumberFormat = _number_format(document, operation["number_format"])
                if "bold" in operation:
                    area.CharWeight = 150.0 if operation["bold"] else 100.0
                if "text_color" in operation:
                    area.CharColor = _color(operation["text_color"])
                if "background_color" in operation:
                    area.CellBackColor = _color(operation["background_color"])
                if "wrap_text" in operation:
                    area.IsTextWrapped = operation["wrap_text"]
            elif kind == "fill_down":
                area.fillAuto(_fill_direction("TO_BOTTOM"), operation["source_rows"])
            elif kind == "fill_right":
                area.fillAuto(_fill_direction("TO_RIGHT"), operation["source_columns"])
            elif kind == "sort_range":
                _sort(area, operation)


def _format_snapshot(document, area, operation: dict[str, Any]) -> dict[str, Any]:
    snapshot: dict[str, Any] = {}
    if "number_format" in operation:
        formats = document.getNumberFormats()
        properties = formats.getByKey(int(area.NumberFormat))
        snapshot["number_format"] = str(properties.getPropertyValue("FormatString"))
    if "bold" in operation:
        snapshot["bold"] = float(area.CharWeight) >= 150.0
    if "text_color" in operation:
        snapshot["text_color"] = f"#{int(area.CharColor) & 0xFFFFFF:06X}"
    if "background_color" in operation:
        snapshot["background_color"] = f"#{int(area.CellBackColor) & 0xFFFFFF:06X}"
    if "wrap_text" in operation:
        snapshot["wrap_text"] = bool(area.IsTextWrapped)
    return snapshot


def _target_fingerprints(document, operations: list[dict[str, Any]]) -> list[dict[str, Any]]:
    sheets = document.getSheets()
    targets: dict[tuple[str, str], dict[str, Any]] = {}
    for operation in operations:
        if "range" not in operation or not sheets.hasByName(operation["sheet"]):
            continue
        key = (operation["sheet"], operation["range"])
        target = targets.setdefault(key, {"sheet": key[0], "range": key[1], "format": {}})
        if operation["type"] == "format_cells":
            target["format"].update(_format_snapshot(document, sheets.getByName(key[0]).getCellRangeByName(key[1]), operation))
    result = []
    for key in sorted(targets):
        target = targets[key]
        area = sheets.getByName(key[0]).getCellRangeByName(key[1])
        result.append({
            **target,
            "values": area.getDataArray(),
            "formulas": area.getFormulaArray(),
        })
    return result


def _target_changes(before: list[dict[str, Any]], after: list[dict[str, Any]]) -> list[dict[str, Any]]:
    prior = {(item["sheet"], item["range"]): item for item in before}
    following = {(item["sheet"], item["range"]): item for item in after}
    return [
        {
            "sheet": key[0], "range": key[1],
            "before": prior.get(key, {}), "after": following.get(key, {}),
        }
        for key in sorted(set(prior) | set(following))
    ]


def _store(document, path: Path, filter_name: str) -> None:
    document.storeAsURL(_url(path), (_property("FilterName", filter_name), _property("Overwrite", False)))


def _render_pdf(document, path: Path) -> None:
    document.storeToURL(_url(path), (_property("FilterName", "calc_pdf_Export"), _property("Overwrite", False)))


def _filter_for(path: Path) -> str:
    if path.suffix.lower() == ".xlsx":
        return "Calc MS Excel 2007 XML"
    if path.suffix.lower() == ".ods":
        return "calc8"
    raise RuntimeError("unsupported writable workbook format")


def _inventory(inspection: dict[str, Any]) -> dict[str, Any]:
    return {
        "sheets": [{"name": sheet["name"], "used_range": sheet["used_range"]} for sheet in inspection["sheets"]],
        "formula_count": inspection["formula_count"],
        "formula_errors": inspection["formula_errors"],
        "named_ranges": inspection["named_ranges"],
    }


def run(request: dict[str, Any]) -> dict[str, Any]:
    import uno  # noqa: F401 - verifies the system UNO bridge before Calc launch

    action = request["action"]
    source = Path("/job" if Path("/job").exists() else Path.cwd()) / request["source"]
    out = source.parent.parent / "out"
    limits = request["limits"]
    process, context = _connect(request["soffice"], source.parent.parent / "profile")
    document = None
    reopened = None
    try:
        document = _load(context, source, read_only=action not in ("stage", "convert_xls"))
        if action == "describe":
            result = _inspect(document, limits, include_formulas=request["arguments"].get("include_formulas", False))
            return {"result": result, "artifacts": {}}
        if action == "read_range":
            return {"result": _read_range(document, request["arguments"], limits), "artifacts": {}}
        if action == "search":
            return {"result": _search(document, request["arguments"], limits), "artifacts": {}}
        if action == "trace":
            return {"result": _trace(document, request["arguments"], limits), "artifacts": {}}
        if action == "analyze":
            return {"result": _analyze(document, request["arguments"], limits), "artifacts": {}}
        if action == "render":
            preview = out / "preview.pdf"
            _render_pdf(document, preview)
            return {"result": {"format": "pdf", "engine": {"name": "LibreOffice Calc"}}, "artifacts": {"preview": "out/preview.pdf"}}

        before = _inspect(document, limits, include_formulas=True)
        before_targets = _target_fingerprints(document, request["arguments"].get("operations", []))
        before_objects = _object_fingerprints(document, request["arguments"].get("operations", []))
        if action == "stage":
            _apply(document, request["arguments"]["operations"])
            extension = source.suffix.lower()
            filter_name = _filter_for(Path(f"x{extension}"))
        elif action == "convert_xls":
            filter_name = "Calc MS Excel 2007 XML"
            extension = ".xlsx"
        else:
            raise RuntimeError("unsupported Calc action")

        document.calculateAll()
        expected = _inspect(document, limits, include_formulas=True)
        expected_targets = _target_fingerprints(document, request["arguments"].get("operations", []))
        expected_objects = _object_fingerprints(document, request["arguments"].get("operations", []))
        workbook = out / f"workbook{extension}"
        _store(document, workbook, filter_name)
        document.close(True)
        document = None
        reopened = _load(context, workbook, read_only=True)
        reopened.calculateAll()
        after = _inspect(reopened, limits, include_formulas=True)
        reopened_targets = _target_fingerprints(reopened, request["arguments"].get("operations", []))
        reopened_objects = _object_fingerprints(reopened, request["arguments"].get("operations", []))
        preview = out / "preview.pdf"
        _render_pdf(reopened, preview)
        comparison = {
            "sheet_inventory_match": _inventory(expected)["sheets"] == _inventory(after)["sheets"],
            "named_ranges_match": expected["named_ranges"] == after["named_ranges"],
            "formula_count_match": expected["formula_count"] == after["formula_count"],
            "target_ranges_match": expected_targets == reopened_targets,
            "workbook_objects_match": expected_objects == reopened_objects,
            "new_formula_errors": [error for error in after["formula_errors"] if error not in before["formula_errors"]],
        }
        if (
            not comparison["sheet_inventory_match"]
            or not comparison["named_ranges_match"]
            or not comparison["formula_count_match"]
            or not comparison["target_ranges_match"]
            or not comparison["workbook_objects_match"]
        ):
            raise RuntimeError("staged workbook did not survive save and reopen verification")
        status = "manual_review_required" if action == "convert_xls" else "verified"
        result = {
            "semantic_diff": {
                "operation_count": len(request["arguments"].get("operations", [])),
                "before": _inventory(before),
                "after": _inventory(after),
                "target_changes": _target_changes(before_targets, expected_targets),
                "object_changes": {"before": before_objects, "after": expected_objects},
            },
            "verification": {
                "status": status,
                "recalculated": True,
                "reopened": True,
                "filter_name": filter_name,
                "comparison": comparison,
                "excel_equivalence": "not_claimed",
            },
            "warnings": (
                ["Legacy conversion requires manual review in both Calc and the rendered PDF"]
                if action == "convert_xls" else
                (["Pivot tables in .xlsx may render differently in Microsoft Excel; review the staged workbook and PDF"]
                 if source.suffix.lower() == ".xlsx" and any(operation["type"] in {"upsert_pivot", "refresh_pivot"} for operation in request["arguments"].get("operations", [])) else [])
            ),
            "engine": {"name": "LibreOffice Calc", "filter_name": filter_name},
        }
        return {"result": result, "artifacts": {"workbook": f"out/{workbook.name}", "preview": "out/preview.pdf"}}
    finally:
        for item in (reopened, document):
            if item is not None:
                try:
                    item.close(True)
                except Exception:
                    pass
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()


def main(argv: list[str]) -> int:
    request_path = Path(argv[1])
    result_path = Path(argv[2])
    try:
        request = json.loads(request_path.read_text(encoding="utf-8"))
        payload = {"ok": True, **run(request)}
    except Exception as exc:
        payload = {"ok": False, "error": f"{type(exc).__name__}: {exc}"[:512]}
    result_path.write_text(json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")
    return 0 if payload["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
