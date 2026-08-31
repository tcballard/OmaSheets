"""Typed change-operation validation independent of transport schemas."""

from __future__ import annotations

import re
from typing import Any

from .errors import PolicyError

_A1 = re.compile(r"^\$?([A-Z]{1,3})\$?([1-9][0-9]{0,6})(?::\$?([A-Z]{1,3})\$?([1-9][0-9]{0,6}))?$")
_SHEET = re.compile(r"^[^\x00-\x1f\\/?*\[\]:]{1,128}$")
_COLOR = re.compile(r"^#[0-9A-Fa-f]{6}$")
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

        item = dict(operation)
        if "range" in item:
            item["range"] = item["range"].upper()
        for key in ("text_color", "background_color"):
            if key in item:
                item[key] = item[key].upper()
        normalized.append(item)
    return normalized


def destructive_operations(operations: list[dict[str, Any]]) -> list[int]:
    destructive = {"clear_range", "delete_sheet"}
    return [index for index, operation in enumerate(operations) if operation["type"] in destructive]
