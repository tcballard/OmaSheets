#!/usr/bin/env python3
"""Check the isolated Qt grid spike and validate its bounded smoke report."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MARKER = "OMASHEETS_GRID_BENCHMARK "


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(message)


def check_static_contract() -> None:
    workspace = (ROOT / "Cargo.toml").read_text()
    manifest = (ROOT / "spikes/qt-grid/Cargo.toml").read_text()
    qml = (ROOT / "spikes/qt-grid/qml/Main.qml").read_text()
    model = (ROOT / "spikes/qt-grid/src/grid_model.rs").read_text()

    require('"spikes/qt-grid"' in workspace, "Qt spike must remain outside the release workspace")
    require('cxx-qt = "=0.10.0"' in manifest, "CXX-Qt must be pinned exactly")
    require('cxx-qt-build = { version = "=0.10.0"' in manifest, "CXX-Qt build must be pinned exactly")
    require("const ROWS: i32 = 1_000_000;" in model, "fixture must contain one million rows")
    require("const COLUMNS: i32 = 64;" in model, "fixture must contain 64 columns")
    require("grid.visibleDelegates" in qml, "delegates must be bounded to the viewport")
    require("Accessible.role" in qml and "Accessible.name" in qml, "grid needs accessibility metadata")
    require("FrameAnimation" in qml and "measuredFrames: 180" in qml, "scroll smoke needs measured frames")
    for key in ("Key_Left", "Key_Right", "Key_Up", "Key_Down", "Key_PageUp", "Key_PageDown"):
        require(key in qml, f"keyboard contract missing {key}")
    for rejected in ("Electron", "React", "Glide Data Grid"):
        require(rejected not in qml + manifest, f"isolated Qt spike unexpectedly contains {rejected}")


def read_report(path: Path) -> dict[str, object]:
    payloads = [line[len(MARKER) :] for line in path.read_text().splitlines() if line.startswith(MARKER)]
    require(len(payloads) == 1, f"expected one {MARKER.strip()} line, found {len(payloads)}")
    try:
        report = json.loads(payloads[0])
    except json.JSONDecodeError as error:
        raise SystemExit(f"invalid grid benchmark JSON: {error}") from error
    require(isinstance(report, dict), "grid benchmark payload must be an object")
    return report


def check_report(report: dict[str, object]) -> None:
    require(report.get("schema") == 1, "unexpected benchmark schema")
    require(report.get("fixture") == "synthetic-1000000x64", "unexpected benchmark fixture")
    require(report.get("rows") == 1_000_000, "benchmark did not exercise one million rows")
    require(report.get("columns") == 64, "benchmark did not exercise 64 columns")
    require(isinstance(report.get("frames"), int) and report["frames"] >= 180, "benchmark needs 180 measured frames")
    for field in ("elapsed_seconds", "p95_frame_ms", "worst_frame_ms", "startup_to_report_ms"):
        require(isinstance(report.get(field), (int, float)) and report[field] > 0, f"{field} must be positive")
    delegates = report.get("visible_delegates")
    require(isinstance(delegates, int) and 0 < delegates <= 1_000, "visible delegates must remain bounded")
    require(isinstance(report.get("cell_reads"), int) and report["cell_reads"] > 0, "benchmark must read cells")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", type=Path, help="stdout captured from the headless grid smoke")
    args = parser.parse_args()

    check_static_contract()
    if args.report:
        check_report(read_report(args.report))


if __name__ == "__main__":
    main()
