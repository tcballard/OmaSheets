"""Bounded runtime checks for an Omarchy workstation."""

from __future__ import annotations

import shutil
from pathlib import Path
from typing import Any

from .integration import DESKTOP_ID, IntegrationPaths
from .installation import dependency_report
from .native_grid import status as grid_status
from .package_install import is_package_managed


def _executable(name: str, expected: Path | None = None) -> dict[str, Any]:
    found = str(expected) if expected and expected.is_file() else shutil.which(name)
    return {"name": name, "ok": found is not None, "detail": found or "not found on PATH"}


def diagnose() -> dict[str, Any]:
    package_managed = is_package_managed()
    checks = dependency_report(include_setup=not package_managed)["checks"]
    checks.extend(_executable(name) for name in ("omasheets-kit", "omasheets-service"))

    integration = IntegrationPaths.discover()
    desktop_ok = integration.desktop.is_file() and integration.journal.is_file()
    if package_managed:
        desktop_ok = Path("/usr/share/applications", DESKTOP_ID).is_file()
    checks.append({
        "name": "desktop-integration",
        "ok": desktop_ok,
        "detail": DESKTOP_ID if desktop_ok else "run: omasheets integrate install",
        "required": True,
    })
    plugin = Path.home() / ".config/omarchy/plugins/io.github.tcballard.omasheets/manifest.json"
    checks.append({
        "name": "omarchy-plugin",
        "ok": plugin.is_file(),
        "detail": str(plugin) if plugin.is_file() else "optional bar widget is not installed",
        "required": False,
    })
    grid = grid_status()
    checks.append({
        "name": "omasheets-grid",
        "ok": grid["ready"],
        "detail": grid["detail"],
        "required": True,
    })
    required = [check for check in checks if check.get("required", True)]
    return {"ready": all(check["ok"] for check in required), "checks": checks}
