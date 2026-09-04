#!/usr/bin/env python3
"""Build aggregate corpus summary and delta evidence from one measured score."""

from __future__ import annotations

import argparse
from datetime import date
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
from typing import Any


PERFORMANCE_SCHEMA = "OMASHEETS_PERFORMANCE_V1"
OWNED_KEYS = (
    "opened", "failed", "timed_out", "formula_cells_observed",
    "formula_cells_loaded", "formula_cells_compared", "stored_values_matched",
    "stored_values_mismatched", "unsupported_formulas", "formula_parse_rate",
    "comparison_coverage", "stored_value_match_rate", "peak_rss_bytes_max",
)
OWNED_ADDITIONS = ("workbooks_with_skipped_sheets", "skipped_sheets")
HEX40 = re.compile(r"^[0-9a-f]{40}$")
SOURCE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def classify_failure(message: str, *, owned: bool) -> str:
    lower = message.lower()
    if "timed out" in lower or "timeout" in lower:
        return "timeout"
    if owned:
        if "1904" in lower and "date" in lower:
            return "date_system_1904_rejected"
        if "2,000,000" in lower or "2000000" in lower or "cell limit" in lower:
            return "cell_limit_2000000_exceeded"
    else:
        if "relationship" in lower and ("not found" in lower or "missing" in lower):
            return "relationship_not_found"
        if "chart" in lower and "worksheet" in lower:
            return "chartsheet_instead_of_worksheet"
        if "memory allocation" in lower or "out of memory" in lower:
            return "memory_allocation_failed"
        if "undefined name" in lower or ("formula" in lower and "parse" in lower):
            return "undefined_name_or_formula_parse"
    return "other"


def failure_kinds(entries: list[Any], *, owned: bool) -> dict[str, int]:
    status_key = "owned_status" if owned else "status"
    error_key = "owned_error" if owned else "error"
    counts: dict[str, int] = {}
    for entry in entries:
        if not isinstance(entry, dict) or entry.get(status_key) == "ok":
            continue
        kind = classify_failure(str(entry.get(error_key, "")), owned=owned)
        counts[kind] = counts.get(kind, 0) + 1
    return dict(sorted(counts.items()))


def atomic_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(payload, output, indent=2, ensure_ascii=False)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def parse_counts(values: list[str]) -> dict[str, int]:
    result: dict[str, int] = {}
    for value in values:
        name, separator, raw_count = value.partition("=")
        if not separator or not name or not raw_count.isdecimal():
            raise ValueError(f"expected NAME=COUNT, got {value!r}")
        result[name] = int(raw_count)
    return dict(sorted(result.items()))


def build_summary(arguments: argparse.Namespace) -> dict[str, Any]:
    score = load_json(arguments.score)
    performance = load_json(arguments.performance)
    if score.get("schema") != 2 or not isinstance(score.get("entries"), list):
        raise ValueError("score must be an omasheets-corpus schema 2 report")
    if performance.get("schema") != PERFORMANCE_SCHEMA:
        raise ValueError("performance input has the wrong schema")
    if performance.get("exit_code") != 0 or performance.get("timed_out") is not False:
        raise ValueError("performance input does not describe a successful completed score")
    peak = performance.get("memory", {}).get("peak_rss_bytes")
    if not isinstance(peak, int) or peak < 0:
        raise ValueError("performance input has no measured peak RSS")
    commit = arguments.engine_commit or subprocess.run(
        ["git", "rev-parse", "HEAD"], check=True, text=True, capture_output=True,
    ).stdout.strip()
    if not HEX40.fullmatch(commit):
        raise ValueError("engine commit must be a full lowercase Git SHA-1")
    try:
        date.fromisoformat(arguments.scored)
    except ValueError as error:
        raise ValueError("scored must be an ISO 8601 calendar date") from error

    owned = dict(score.get("owned_summary", {}))
    unsupported = owned.pop("unsupported_functions", None)
    if not isinstance(unsupported, dict):
        raise ValueError("score has no owned unsupported-function aggregate")
    unsupported = dict(sorted(
        unsupported.items(),
        key=lambda item: (-int(item[1]["formula_cells"]), item[0]),
    ))
    source = arguments.source or arguments.manifest.stem
    if arguments.manifest.suffix != ".jsonl":
        raise ValueError("manifest must be a .jsonl file")
    if not SOURCE.fullmatch(source):
        raise ValueError("source must be a bounded lowercase identifier")
    command = arguments.command or (
        f"omasheets-corpus score {arguments.manifest.name} <root> <out> --timeout-seconds 30"
    )
    safety_note = "Aggregate only: no workbook paths, cell contents or per-file results are recorded here."
    return {
        "schema": 1,
        "source": source,
        "manifest": arguments.manifest.name,
        "manifest_sha256": sha256(arguments.manifest),
        "scored": arguments.scored,
        "command": command,
        "engine_commit": commit,
        "runner": arguments.runner,
        "wall_seconds": performance["wall_seconds"],
        "process_tree_peak_rss_bytes": peak,
        "candidate_engine": score.get("engine"),
        "candidate_summary": score.get("summary"),
        "owned_engine": score.get("owned_engine"),
        "owned_summary": owned,
        "owned_unsupported_functions": unsupported,
        "candidate_failure_kinds": failure_kinds(score["entries"], owned=False),
        "owned_failure_kinds": failure_kinds(score["entries"], owned=True),
        "notes": [safety_note, *arguments.note],
    }


def build_delta(
    summary: dict[str, Any], baseline: dict[str, Any], arguments: argparse.Namespace,
) -> dict[str, Any]:
    for key in ("source", "manifest", "manifest_sha256"):
        if baseline.get(key) != summary[key]:
            raise ValueError(f"baseline {key} does not match the new summary")
    before = baseline.get("owned_summary", {})
    after = summary["owned_summary"]
    missing = [key for key in OWNED_KEYS if key not in before or key not in after]
    if missing:
        raise ValueError(f"owned summaries are missing required keys: {', '.join(missing)}")
    lane = {key: {"before": before[key], "after": after[key]} for key in OWNED_KEYS}
    additions = {
        key: {"before": before.get(key), "after": after.get(key)}
        for key in OWNED_ADDITIONS if key in before or key in after
    }
    return {
        "schema": 1,
        "source": summary["source"],
        "manifest": summary["manifest"],
        "manifest_sha256": summary["manifest_sha256"],
        "baseline_engine_commit": baseline["engine_commit"],
        "engine_commit": summary["engine_commit"],
        "scored": summary["scored"],
        "owned_lane": lane,
        "owned_lane_added": additions,
        "unsupported_reasons": {
            "before": before.get("unsupported_reasons", {}),
            "after": after.get("unsupported_reasons", {}),
        },
        "resolved_functions": arguments.resolved_function,
        "resolved_defects": arguments.resolved_defect,
        "resolved_open_failures": parse_counts(arguments.resolved_open_failure),
        "wall_seconds": {
            "before": baseline["wall_seconds"],
            "after": summary["wall_seconds"],
        },
        "process_tree_peak_rss_bytes": {
            "before": baseline["process_tree_peak_rss_bytes"],
            "after": summary["process_tree_peak_rss_bytes"],
        },
        "notes": arguments.delta_note,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--score", type=Path, required=True)
    parser.add_argument("--performance", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--baseline-summary", type=Path, required=True)
    parser.add_argument("--summary", type=Path, required=True)
    parser.add_argument("--delta", type=Path, required=True)
    parser.add_argument("--runner", required=True)
    parser.add_argument("--scored", default=date.today().isoformat())
    parser.add_argument("--source")
    parser.add_argument("--command")
    parser.add_argument("--engine-commit")
    parser.add_argument("--note", action="append", default=[])
    parser.add_argument("--delta-note", action="append", default=[])
    parser.add_argument("--resolved-function", action="append", default=[])
    parser.add_argument("--resolved-defect", action="append", default=[])
    parser.add_argument("--resolved-open-failure", action="append", default=[])
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
    summary = build_summary(arguments)
    delta = build_delta(summary, load_json(arguments.baseline_summary), arguments)
    atomic_json(arguments.summary, summary)
    atomic_json(arguments.delta, delta)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
