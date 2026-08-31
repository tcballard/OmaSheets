"""Typed change-operation validation independent of transport schemas."""

from __future__ import annotations

import re
from typing import Any

from .errors import PolicyError

_A1 = re.compile(r"^\$?([A-Z]{1,3})\$?([1-9][0-9]{0,6})(?::\$?([A-Z]{1,3})\$?([1-9][0-9]{0,6}))?$")
_SHEET = re.compile(r"^[^\x00-\x1f\\/?*\[\]:]{1,128}$")
_COLOR = re.compile(r"^#[0-9A-Fa-f]{6}$")
_COLUMN = re.compile(r"^[A-Z]{1,3}$")
_OBJECT_NAME = re.compile(r"^[^\x00-\x1f]{1,128}$")
_UNSAFE_FORMULA = re.compile(
    r"(?:\b(?:WEBSERVICE|DDE)\s*\(|(?:https?|ftp|file)://|(?:^|[;(])\s*['\"]?[^'\"]+\.(?:ods|xlsx?|xlsm)['\"]?[#.])",
    re.IGNORECASE,
)
_MAX_RANGE_CELLS = 10_000

_FIELDS: dict[str, tuple[set[str], set[str]]] = {
    "set_value": ({"type", "sheet", "range", "value"}, {"type", "sheet", "range", "value"}),
    "set_formula": ({"type", "sheet", "range", "formula"}, {"type", "sheet", "range", "formula"}),
    "clear_range": ({"type", "sheet", "range"}, {"type", "sheet", "range"}),
    "rename_sheet": ({"type", "sheet", "new_name"}, {"type", "sheet", "new_name"}),
    "add_sheet": ({"type", "sheet"}, {"type", "sheet"}),
    "delete_sheet": ({"type", "sheet"}, {"type", "sheet"}),
    "set_range_values": (
        {"type", "sheet", "range", "values"},
        {"type", "sheet", "range", "values"},
    ),
    "set_range_formulas": (
        {"type", "sheet", "range", "formulas"},
        {"type", "sheet", "range", "formulas"},
    ),
    "format_cells": (
        {"type", "sheet", "range", "number_format", "bold", "text_color", "background_color", "wrap_text"},
        {"type", "sheet", "range"},
    ),
    "insert_rows": ({"type", "sheet", "row", "count"}, {"type", "sheet", "row", "count"}),
    "delete_rows": ({"type", "sheet", "row", "count"}, {"type", "sheet", "row", "count"}),
    "insert_columns": ({"type", "sheet", "column", "count"}, {"type", "sheet", "column", "count"}),
    "delete_columns": ({"type", "sheet", "column", "count"}, {"type", "sheet", "column", "count"}),
    "fill_down": ({"type", "sheet", "range", "source_rows"}, {"type", "sheet", "range", "source_rows"}),
    "fill_right": ({"type", "sheet", "range", "source_columns"}, {"type", "sheet", "range", "source_columns"}),
    "sort_range": (
        {"type", "sheet", "range", "key_column", "ascending", "has_header"},
        {"type", "sheet", "range", "key_column", "ascending", "has_header"},
    ),
    "upsert_chart": (
        {"type", "sheet", "name", "source_sheet", "source_range", "anchor_range", "chart_type", "title", "has_column_headers", "has_row_headers", "legend"},
        {"type", "sheet", "name", "source_sheet", "source_range", "anchor_range", "chart_type", "title", "has_column_headers", "has_row_headers", "legend"},
    ),
    "upsert_pivot": (
        {"type", "sheet", "name", "source_sheet", "source_range", "output_cell", "rows", "columns", "filters", "values"},
        {"type", "sheet", "name", "source_sheet", "source_range", "output_cell", "rows", "columns", "filters", "values"},
    ),
    "refresh_pivot": ({"type", "sheet", "name"}, {"type", "sheet", "name"}),
}
SUPPORTED_OPERATIONS = tuple(_FIELDS)


def _column_number(name: str) -> int:
    value = 0
    for character in name:
        value = value * 26 + ord(character) - ord("A") + 1
    return value


def range_shape(value: str) -> tuple[int, int]:
    """Return rows and columns for a validated A1 cell or range."""

    matched = _A1.fullmatch(value.upper())
    if matched is None:
        raise PolicyError("invalid A1 range")
    start_column, start_row, end_column, end_row = matched.groups()
    end_column = end_column or start_column
    end_row = end_row or start_row
    start_column_number = _column_number(start_column)
    end_column_number = _column_number(end_column)
    start_row_number = int(start_row)
    end_row_number = int(end_row)
    if end_column_number > 16_384 or end_row_number > 1_048_576:
        raise PolicyError("A1 range exceeds spreadsheet bounds")
    columns = end_column_number - start_column_number + 1
    rows = end_row_number - start_row_number + 1
    if rows < 1 or columns < 1:
        raise PolicyError("A1 range must run from top-left to bottom-right")
    return rows, columns


def _validate_matrix(value: Any, *, rows: int, columns: int, formulas: bool, index: int) -> list[list[Any]]:
    if not isinstance(value, list) or len(value) != rows:
        raise PolicyError(f"operation {index} matrix does not match its range rows")
    normalized: list[list[Any]] = []
    for row in value:
        if not isinstance(row, list) or len(row) != columns:
            raise PolicyError(f"operation {index} matrix does not match its range columns")
        output_row = []
        for item in row:
            if formulas:
                if not isinstance(item, str) or not item.startswith("=") or len(item) > 8192:
                    raise PolicyError(f"operation {index} contains an invalid formula")
                if _UNSAFE_FORMULA.search(item):
                    raise PolicyError(f"operation {index} contains an external or network-capable formula")
            else:
                if not (item is None or isinstance(item, (str, int, float, bool))):
                    raise PolicyError(f"operation {index} contains an invalid scalar value")
                if isinstance(item, str) and len(item) > 32767:
                    raise PolicyError(f"operation {index} contains a value that is too long")
            output_row.append(item)
        normalized.append(output_row)
    return normalized


def validate_operations(operations: list[dict[str, Any]]) -> list[dict[str, Any]]:
    if not isinstance(operations, list) or not 1 <= len(operations) <= 100:
        raise PolicyError("a plan requires between 1 and 100 operations")

    normalized: list[dict[str, Any]] = []
    for index, operation in enumerate(operations):
        if not isinstance(operation, dict):
            raise PolicyError(f"operation {index} must be an object")
        kind = operation.get("type")
        if kind not in _FIELDS:
            raise PolicyError(f"operation {index} has an unsupported type")
        allowed, required = _FIELDS[kind]
        unknown = set(operation) - allowed
        missing = required - set(operation)
        if unknown or missing:
            raise PolicyError(f"operation {index} has invalid fields")

        for key in ("sheet", "new_name"):
            if key in operation and (
                not isinstance(operation[key], str) or _SHEET.fullmatch(operation[key]) is None
            ):
                raise PolicyError(f"operation {index} has an invalid {key}")
        for key in ("name",):
            if key in operation and (
                not isinstance(operation[key], str) or _OBJECT_NAME.fullmatch(operation[key]) is None
            ):
                raise PolicyError(f"operation {index} has an invalid {key}")
        if "source_sheet" in operation and (
            not isinstance(operation["source_sheet"], str) or _SHEET.fullmatch(operation["source_sheet"]) is None
        ):
            raise PolicyError(f"operation {index} has an invalid source_sheet")
        shape = None
        if "range" in operation:
            if not isinstance(operation["range"], str):
                raise PolicyError(f"operation {index} has an invalid A1 range")
            try:
                shape = range_shape(operation["range"])
            except PolicyError as exc:
                raise PolicyError(f"operation {index} has an invalid A1 range") from exc
            if shape[0] * shape[1] > _MAX_RANGE_CELLS:
                raise PolicyError(f"operation {index} exceeds the {_MAX_RANGE_CELLS}-cell range limit")
        if kind in {"set_value", "set_formula"} and ":" in operation["range"]:
            raise PolicyError(f"operation {index} must target one cell")
        if "formula" in operation and (
            not isinstance(operation["formula"], str)
            or not operation["formula"].startswith("=")
            or len(operation["formula"]) > 8192
        ):
            raise PolicyError(f"operation {index} has an invalid formula")
        if "formula" in operation and _UNSAFE_FORMULA.search(operation["formula"]):
            raise PolicyError(f"operation {index} contains an external or network-capable formula")
        if "value" in operation:
            value = operation["value"]
            if not (value is None or isinstance(value, (str, int, float, bool))):
                raise PolicyError(f"operation {index} has an invalid scalar value")
            if isinstance(value, str) and len(value) > 32767:
                raise PolicyError(f"operation {index} value is too long")

        if kind == "set_range_values":
            assert shape is not None
            operation = dict(operation)
            operation["values"] = _validate_matrix(
                operation["values"], rows=shape[0], columns=shape[1], formulas=False, index=index
            )
        if kind == "set_range_formulas":
            assert shape is not None
            operation = dict(operation)
            operation["formulas"] = _validate_matrix(
                operation["formulas"], rows=shape[0], columns=shape[1], formulas=True, index=index
            )
        if kind == "format_cells":
            format_fields = {"number_format", "bold", "text_color", "background_color", "wrap_text"}
            if not format_fields.intersection(operation):
                raise PolicyError(f"operation {index} must specify at least one format change")
            if "number_format" in operation and (
                not isinstance(operation["number_format"], str)
                or not 1 <= len(operation["number_format"]) <= 128
                or any(ord(character) < 32 for character in operation["number_format"])
            ):
                raise PolicyError(f"operation {index} has an invalid number format")
            for key in ("bold", "wrap_text"):
                if key in operation and not isinstance(operation[key], bool):
                    raise PolicyError(f"operation {index} has an invalid {key}")
            for key in ("text_color", "background_color"):
                if key in operation and (
                    not isinstance(operation[key], str) or _COLOR.fullmatch(operation[key]) is None
                ):
                    raise PolicyError(f"operation {index} has an invalid {key}")

        if kind in {"insert_rows", "delete_rows"}:
            row = operation["row"]
            count = operation["count"]
            if (
                not isinstance(row, int) or isinstance(row, bool) or not 1 <= row <= 1_048_576
                or not isinstance(count, int) or isinstance(count, bool) or not 1 <= count <= 10_000
                or row + count - 1 > 1_048_576
            ):
                raise PolicyError(f"operation {index} has invalid row bounds")
        if kind in {"insert_columns", "delete_columns"}:
            column = operation["column"]
            count = operation["count"]
            if not isinstance(column, str) or _COLUMN.fullmatch(column.upper()) is None:
                raise PolicyError(f"operation {index} has an invalid column")
            if (
                not isinstance(count, int) or isinstance(count, bool) or not 1 <= count <= 1_000
                or _column_number(column.upper()) + count - 1 > 16_384
            ):
                raise PolicyError(f"operation {index} has invalid column bounds")
        if kind == "fill_down":
            rows, _ = shape or (0, 0)
            count = operation["source_rows"]
            if not isinstance(count, int) or isinstance(count, bool) or not 1 <= count < rows:
                raise PolicyError(f"operation {index} has an invalid source row count")
        if kind == "fill_right":
            _, columns = shape or (0, 0)
            count = operation["source_columns"]
            if not isinstance(count, int) or isinstance(count, bool) or not 1 <= count < columns:
                raise PolicyError(f"operation {index} has an invalid source column count")
        if kind == "sort_range":
            _, columns = shape or (0, 0)
            key_column = operation["key_column"]
            if not isinstance(key_column, int) or isinstance(key_column, bool) or not 1 <= key_column <= columns:
                raise PolicyError(f"operation {index} has an invalid sort key column")
            for key in ("ascending", "has_header"):
                if not isinstance(operation[key], bool):
                    raise PolicyError(f"operation {index} has an invalid {key}")
        if kind == "upsert_chart":
            for key in ("source_range", "anchor_range"):
                if not isinstance(operation[key], str):
                    raise PolicyError(f"operation {index} has an invalid {key}")
                rows, columns = range_shape(operation[key])
                if key == "source_range" and rows * columns > _MAX_RANGE_CELLS:
                    raise PolicyError(f"operation {index} exceeds the {_MAX_RANGE_CELLS}-cell chart source limit")
                if key == "anchor_range" and (rows < 2 or columns < 2):
                    raise PolicyError(f"operation {index} chart anchor must span at least two rows and columns")
            if operation["chart_type"] not in {"column", "bar", "line", "pie", "scatter"}:
                raise PolicyError(f"operation {index} has an invalid chart type")
            if not isinstance(operation["title"], str) or not 1 <= len(operation["title"]) <= 256:
                raise PolicyError(f"operation {index} has an invalid chart title")
            for key in ("has_column_headers", "has_row_headers", "legend"):
                if not isinstance(operation[key], bool):
                    raise PolicyError(f"operation {index} has an invalid {key}")
        if kind == "upsert_pivot":
            if not isinstance(operation["source_range"], str):
                raise PolicyError(f"operation {index} has an invalid source_range")
            rows_count, columns_count = range_shape(operation["source_range"])
            if rows_count * columns_count > _MAX_RANGE_CELLS:
                raise PolicyError(f"operation {index} exceeds the {_MAX_RANGE_CELLS}-cell pivot source limit")
            if not isinstance(operation["output_cell"], str) or ":" in operation["output_cell"]:
                raise PolicyError(f"operation {index} has an invalid output_cell")
            range_shape(operation["output_cell"])
            for key, maximum in (("rows", 8), ("columns", 4), ("filters", 4)):
                fields = operation[key]
                if not isinstance(fields, list) or len(fields) > maximum or any(
                    not isinstance(field, str) or _OBJECT_NAME.fullmatch(field) is None for field in fields
                ):
                    raise PolicyError(f"operation {index} has invalid pivot {key}")
            layout_fields = operation["rows"] + operation["columns"] + operation["filters"]
            if len({field.casefold() for field in layout_fields}) != len(layout_fields):
                raise PolicyError(f"operation {index} reuses a pivot layout field")
            values = operation["values"]
            if not isinstance(values, list) or not 1 <= len(values) <= 8:
                raise PolicyError(f"operation {index} has invalid pivot values")
            for value in values:
                if not isinstance(value, dict) or set(value) - {"field", "function", "label"} or not {"field", "function"} <= set(value):
                    raise PolicyError(f"operation {index} has an invalid pivot value")
                if not isinstance(value["field"], str) or _OBJECT_NAME.fullmatch(value["field"]) is None:
                    raise PolicyError(f"operation {index} has an invalid pivot value field")
                if value["function"] not in {"sum", "count", "average", "min", "max"}:
                    raise PolicyError(f"operation {index} has an invalid pivot function")
                if "label" in value and (not isinstance(value["label"], str) or _OBJECT_NAME.fullmatch(value["label"]) is None):
                    raise PolicyError(f"operation {index} has an invalid pivot label")
            value_fields = [value["field"] for value in values]
            if (len({field.casefold() for field in value_fields}) != len(value_fields)
                    or {field.casefold() for field in value_fields} & {field.casefold() for field in layout_fields}):
                raise PolicyError(f"operation {index} reuses a pivot value field")
        item = dict(operation)
        if "range" in item:
            item["range"] = item["range"].upper()
        for key in ("source_range", "anchor_range", "output_cell"):
            if key in item:
                item[key] = item[key].upper()
        if "column" in item:
            item["column"] = item["column"].upper()
        for key in ("text_color", "background_color"):
            if key in item:
                item[key] = item[key].upper()
        normalized.append(item)
    return normalized


def destructive_operations(operations: list[dict[str, Any]]) -> list[int]:
    destructive = {"clear_range", "delete_sheet", "delete_rows", "delete_columns", "sort_range"}
    return [index for index, operation in enumerate(operations) if operation["type"] in destructive]
