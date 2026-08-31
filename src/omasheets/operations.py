"""Typed change-operation validation independent of transport schemas."""

from __future__ import annotations

import re
from typing import Any

from .errors import PolicyError

_A1 = re.compile(r"^\$?[A-Z]{1,3}\$?[1-9][0-9]{0,6}(?::\$?[A-Z]{1,3}\$?[1-9][0-9]{0,6})?$")
_SHEET = re.compile(r"^[^\x00-\x1f\\/?*\[\]:]{1,128}$")

_FIELDS: dict[str, tuple[set[str], set[str]]] = {
    "set_value": ({"type", "sheet", "range", "value"}, {"type", "sheet", "range", "value"}),
    "set_formula": ({"type", "sheet", "range", "formula"}, {"type", "sheet", "range", "formula"}),
    "clear_range": ({"type", "sheet", "range"}, {"type", "sheet", "range"}),
    "rename_sheet": ({"type", "sheet", "new_name"}, {"type", "sheet", "new_name"}),
    "add_sheet": ({"type", "sheet"}, {"type", "sheet"}),
    "delete_sheet": ({"type", "sheet"}, {"type", "sheet"}),
}


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
        if "range" in operation and (
            not isinstance(operation["range"], str) or _A1.fullmatch(operation["range"].upper()) is None
        ):
            raise PolicyError(f"operation {index} has an invalid A1 range")
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

        item = dict(operation)
        if "range" in item:
            item["range"] = item["range"].upper()
        normalized.append(item)
    return normalized


def destructive_operations(operations: list[dict[str, Any]]) -> list[int]:
    destructive = {"clear_range", "delete_sheet"}
    return [index for index, operation in enumerate(operations) if operation["type"] in destructive]
