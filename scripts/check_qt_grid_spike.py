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
    service_client = (ROOT / "spikes/qt-grid/src/service_client.rs").read_text()
    theme = (ROOT / "spikes/qt-grid/src/theme.rs").read_text()
    service = (ROOT / "crates/omasheets-service/src/lib.rs").read_text()

    require('"spikes/qt-grid"' in workspace, "Qt spike must remain outside the release workspace")
    require((ROOT / "spikes/qt-grid/Cargo.lock").is_file(), "isolated Qt spike needs a lockfile")
    require('cxx = "=1.0.199"' in manifest, "CXX runtime must match the bridge generator ABI")
    require('cxx-qt = "=0.10.0"' in manifest, "CXX-Qt must be pinned exactly")
    require('cxx-qt-build = { version = "=0.10.0"' in manifest, "CXX-Qt build must be pinned exactly")
    require("const ROWS: i32 = 1_000_000;" in model, "fixture must contain one million rows")
    require("const COLUMNS: i32 = 64;" in model, "fixture must contain 64 columns")
    require('cxx_name = "rowCount"' in model and 'cxx_name = "columnCount"' in model,
            "multiword Rust properties need explicit QML names")
    require("grid.visibleDelegates" in qml, "delegates must be bounded to the viewport")
    require("Accessible.role" in qml and "Accessible.name" in qml, "grid needs accessibility metadata")
    require("FrameAnimation" in qml and "measuredFrames: 180" in qml, "scroll smoke needs measured frames")
    require("omarchy/current/theme/colors.toml" in theme, "theme must use Omarchy's semantic palette")
    for key in ("background", "foreground", "accent", "red", "green", "yellow", "blue", "magenta"):
        require(f'"{key}"' in theme, f"Omarchy theme mapping missing {key}")
    require("backend.themeBackground" in qml and "backend.themeAccent" in qml,
            "QML palette must be supplied by the Rust theme adapter")
    require("onTriggered: backend.refreshTheme()" in qml and 'cxx_name = "refreshTheme"' in model,
            "active theme changes must refresh while the window is open")
    require("Request::GridPage" in service and "MAX_GRID_PAGE_CELLS" in service,
            "local service needs a bounded rectangular grid page")
    require("MAX_CACHED_PAGES" in service_client and '"kind": "grid_page"' in service_client,
            "Qt adapter must cache bounded service pages")
    require("backend.cellInput" in qml and 'cxx_name = "cellInput"' in model,
            "editing must preserve formula input instead of calculated display text")
    require("backend.sheetCount" in qml and 'cxx_name = "selectSheet"' in model,
            "native documents need stable-ID sheet switching")
    require("backend.currentSheet - 1" in qml and "backend.currentSheet + 1" in qml,
            "Ctrl+Page Up/Down must switch native sheets")
    require("frameNumber === 1 && backend.documentMode" in qml,
            "native headless smoke must exercise edits through the grid")
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


def check_report(
    report: dict[str, object],
    require_omarchy_theme: bool = False,
    require_native_document: bool = False,
    require_multi_sheet: bool = False,
) -> None:
    require(report.get("schema") == 1, "unexpected benchmark schema")
    source = report.get("source")
    require(source in ("synthetic", "native-document", "document-error"), "unexpected grid source")
    if source == "synthetic":
        require(report.get("fixture") == "synthetic-1000000x64", "unexpected benchmark fixture")
        require(report.get("rows") == 1_000_000, "benchmark did not exercise one million rows")
        require(report.get("columns") == 64, "benchmark did not exercise 64 columns")
    else:
        require(report.get("fixture") == "native-document-grid", "unexpected native fixture")
        require(isinstance(report.get("rows"), int) and report["rows"] > 0, "native document needs rows")
        require(isinstance(report.get("columns"), int) and report["columns"] > 0, "native document needs columns")
    require(isinstance(report.get("frames"), int) and report["frames"] >= 180, "benchmark needs 180 measured frames")
    for field in ("elapsed_seconds", "p95_frame_ms", "worst_frame_ms", "startup_to_report_ms"):
        require(isinstance(report.get(field), (int, float)) and report[field] > 0, f"{field} must be positive")
    delegates = report.get("visible_delegates")
    require(isinstance(delegates, int) and 0 < delegates <= 1_000, "visible delegates must remain bounded")
    require(isinstance(report.get("cell_reads"), int) and report["cell_reads"] > 0, "benchmark must read cells")
    require(report.get("theme_source") in ("omarchy", "fallback"), "benchmark must identify its theme source")
    requests = report.get("service_requests")
    require(isinstance(requests, int) and requests >= 0, "benchmark must count local-service requests")
    sheet_count = report.get("sheet_count")
    current_sheet = report.get("current_sheet")
    require(isinstance(sheet_count, int) and sheet_count >= 0, "benchmark must count sheets")
    require(isinstance(current_sheet, int) and current_sheet >= 0, "benchmark must identify its sheet")
    if require_omarchy_theme:
        require(report["theme_source"] == "omarchy", "benchmark did not load the injected Omarchy theme")
    if require_native_document:
        require(source == "native-document", "benchmark did not load a native document")
        require(0 < requests < report["cell_reads"], "grid must page rather than call the service per cell")
        require(requests <= 256, "native grid made an unexpectedly large number of service requests")
    if require_multi_sheet:
        require(source == "native-document", "multi-sheet proof requires a native document")
        require(sheet_count >= 2, "native document did not expose multiple sheets")
        require(current_sheet == 1, "headless grid did not switch to the second sheet")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", type=Path, help="stdout captured from the headless grid smoke")
    parser.add_argument("--require-omarchy-theme", action="store_true",
                        help="require the benchmark to have loaded an Omarchy colors.toml")
    parser.add_argument("--require-native-document", action="store_true",
                        help="require a real native document loaded through the local service")
    parser.add_argument("--require-multi-sheet", action="store_true",
                        help="require the native smoke to switch to its second sheet")
    args = parser.parse_args()

    check_static_contract()
    if args.report:
        check_report(
            read_report(args.report),
            args.require_omarchy_theme,
            args.require_native_document,
            args.require_multi_sheet,
        )


if __name__ == "__main__":
    main()
