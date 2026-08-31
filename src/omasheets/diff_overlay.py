"""Bounded, path-free review payload for the native workbook overlay."""

from __future__ import annotations

from pathlib import Path
from typing import Any
from urllib.parse import quote, unquote

from .store import write_text_atomic

VERSION = "OMASHEETS_DIFF_V1"
MAX_ITEMS = 200
MAX_FIELD_CHARS = 512
MAX_FILE_BYTES = 256 * 1024


def overlay_path(runtime: Path) -> Path:
    return runtime / "window-diff.overlay"


def _display(value: Any) -> str:
    if value is None or value == "":
        return "∅"
    if isinstance(value, bool):
        return "TRUE" if value else "FALSE"
    text = str(value).replace("\r", " ").replace("\n", " ↵ ")
    return text[:MAX_FIELD_CHARS]


def _display_format(value: dict[str, Any]) -> str:
    if not value:
        return "Default formatting"
    return " · ".join(
        f"{key.replace('_', ' ')}: {_display(item)}" for key, item in sorted(value.items())
    )[:MAX_FIELD_CHARS]


def _column_name(number: int) -> str:
    output = ""
    while number:
        number, remainder = divmod(number - 1, 26)
        output = chr(65 + remainder) + output
    return output


def _range_origin(address: str) -> tuple[int, int]:
    start = address.replace("$", "").split(":", 1)[0]
    split = next(index for index, character in enumerate(start) if character.isdigit())
    column = 0
    for character in start[:split]:
        column = column * 26 + ord(character) - 64
    return column, int(start[split:])


def _target_map(semantic_diff: dict[str, Any]) -> dict[tuple[str, str], dict[str, Any]]:
    result = {}
    for change in semantic_diff.get("target_changes", []):
        if isinstance(change, dict) and isinstance(change.get("sheet"), str) and isinstance(change.get("range"), str):
            result[(change["sheet"], change["range"])] = change
    return result


def _matrix_item(matrix: Any, row: int, column: int) -> Any:
    try:
        return matrix[row][column]
    except (IndexError, TypeError):
        return ""


def build_overlay(plan: dict[str, Any]) -> dict[str, Any]:
    """Convert sealed plan evidence into a bounded native review model."""

    targets = _target_map(plan.get("semantic_diff") or {})
    items: list[dict[str, str]] = []
    total = 0
    for operation in plan.get("operations", []):
        kind = operation["type"]
        sheet = operation.get("sheet", "")
        address = operation.get("range", "")
        target = targets.get((sheet, address), {})
        before = target.get("before") or {}
        after = target.get("after") or {}
        if address and kind not in {"format_cells"}:
            if not target:
                total += 1
                if len(items) < MAX_ITEMS:
                    proposed = operation.get("formula", operation.get("value", kind.replace("_", " ")))
                    items.append({
                        "kind": kind, "sheet": sheet, "range": address,
                        "before": "Current content", "after": _display(proposed),
                    })
                continue
            values_before = before.get("values", [])
            values_after = after.get("values", [])
            formulas_before = before.get("formulas", [])
            formulas_after = after.get("formulas", [])
            rows = max(len(values_before), len(values_after), 1)
            columns = max(
                max((len(row) for row in values_before), default=0),
                max((len(row) for row in values_after), default=0),
                1,
            )
            origin_column, origin_row = _range_origin(address)
            for row in range(rows):
                for column in range(columns):
                    old_formula = _matrix_item(formulas_before, row, column)
                    new_formula = _matrix_item(formulas_after, row, column)
                    old = old_formula or _matrix_item(values_before, row, column)
                    new = new_formula or _matrix_item(values_after, row, column)
                    if old == new:
                        continue
                    total += 1
                    if len(items) < MAX_ITEMS:
                        items.append({
                            "kind": kind,
                            "sheet": sheet,
                            "range": f"{_column_name(origin_column + column)}{origin_row + row}",
                            "before": _display(old),
                            "after": _display(new),
                        })
            continue
        total += 1
        if len(items) >= MAX_ITEMS:
            continue
        if kind == "format_cells":
            old = _display_format(before.get("format", {}))
            new = _display_format(after.get("format", {}))
        elif kind == "rename_sheet":
            old, new = sheet, operation["new_name"]
        elif kind == "add_sheet":
            old, new = "∅", f"Add sheet {sheet}"
        elif kind == "delete_sheet":
            old, new = f"Sheet {sheet}", "Deleted"
        else:
            old, new = "Current content", kind.replace("_", " ")
        items.append({
            "kind": kind,
            "sheet": sheet,
            "range": address or "Sheet",
            "before": _display(old),
            "after": _display(new),
        })
    return {
        "version": 1,
        "session_id": plan["session_id"],
        "revision": plan["revision"],
        "plan_id": plan["plan_id"],
        "status": plan["status"],
        "operation_count": len(plan.get("operations", [])),
        "destructive_count": len(plan.get("destructive_operations", [])),
        "warning_count": len(plan.get("warnings", [])),
        "total_changes": total,
        "truncated": total > len(items),
        "items": items,
    }


def encode_overlay(overlay: dict[str, Any]) -> str:
    def encoded(value: Any) -> str:
        return quote(str(value), safe="")

    lines = [VERSION]
    for key in (
        "session_id", "revision", "plan_id", "status", "operation_count",
        "destructive_count", "warning_count", "total_changes", "truncated",
    ):
        lines.append(f"meta\t{key}\t{encoded(str(overlay[key]).lower() if isinstance(overlay[key], bool) else overlay[key])}")
    for item in overlay["items"]:
        lines.append("item\t" + "\t".join(encoded(item[key]) for key in ("kind", "sheet", "range", "before", "after")))
    payload = "\n".join(lines) + "\n"
    if len(payload.encode()) > MAX_FILE_BYTES:
        raise ValueError("diff overlay exceeds its file limit")
    return payload


def decode_overlay(payload: str) -> dict[str, Any]:
    if len(payload.encode()) > MAX_FILE_BYTES:
        raise ValueError("diff overlay exceeds its file limit")
    lines = payload.splitlines()
    if not lines or lines[0] != VERSION:
        raise ValueError("unsupported diff overlay")
    metadata: dict[str, str] = {}
    items = []
    for line in lines[1:]:
        fields = line.split("\t")
        if fields[0] == "meta" and len(fields) == 3:
            metadata[fields[1]] = unquote(fields[2])
        elif fields[0] == "item" and len(fields) == 6 and len(items) < MAX_ITEMS:
            values = [unquote(value) for value in fields[1:]]
            items.append(dict(zip(("kind", "sheet", "range", "before", "after"), values, strict=True)))
        else:
            raise ValueError("malformed diff overlay")
    required = {
        "session_id", "revision", "plan_id", "status", "operation_count",
        "destructive_count", "warning_count", "total_changes", "truncated",
    }
    if set(metadata) != required:
        raise ValueError("incomplete diff overlay")
    return {
        "version": 1,
        "session_id": metadata["session_id"],
        "revision": int(metadata["revision"]),
        "plan_id": metadata["plan_id"],
        "status": metadata["status"],
        "operation_count": int(metadata["operation_count"]),
        "destructive_count": int(metadata["destructive_count"]),
        "warning_count": int(metadata["warning_count"]),
        "total_changes": int(metadata["total_changes"]),
        "truncated": metadata["truncated"] == "true",
        "items": items,
    }


def publish_overlay(path: Path, plan: dict[str, Any]) -> dict[str, Any]:
    overlay = build_overlay(plan)
    write_text_atomic(path, encode_overlay(overlay), mode=0o600)
    return overlay
