"""Bounded runtime checks for an Omarchy workstation."""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path
from typing import Any

from .integration import DESKTOP_ID, IntegrationPaths
from .lok_spike import status as lok_status


def _executable(name: str, expected: Path | None = None) -> dict[str, Any]:
    found = str(expected) if expected and expected.is_file() else shutil.which(name)
    return {"name": name, "ok": found is not None, "detail": found or "not found on PATH"}


def diagnose() -> dict[str, Any]:
    checks = [
        _executable("bwrap", Path("/usr/bin/bwrap")),
        _executable("soffice", Path("/usr/bin/soffice")),
        _executable("python", Path("/usr/bin/python")),
    ]
    python = checks[2]["detail"] if checks[2]["ok"] else None
    uno_ok = False
    if python:
        completed = subprocess.run(
            [python, "-c", "import uno"],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=10,
        )
        uno_ok = completed.returncode == 0
    checks.append({"name": "python-uno", "ok": uno_ok, "detail": "importable" if uno_ok else "import uno failed"})

    integration = IntegrationPaths.discover()
    desktop_ok = integration.desktop.is_file() and integration.journal.is_file()
    checks.append({
        "name": "desktop-integration",
        "ok": desktop_ok,
        "detail": DESKTOP_ID if desktop_ok else "run: omasheets integrate install",
        "required": False,
    })
    plugin = Path.home() / ".config/omarchy/plugins/io.github.tcballard.omasheets/manifest.json"
    checks.append({
        "name": "omarchy-plugin",
        "ok": plugin.is_file(),
        "detail": str(plugin) if plugin.is_file() else "install with: omarchy plugin add <repository> --enable",
        "required": False,
    })
    lok = lok_status()
    checks.append({
        "name": "libreofficekit-spike",
        "ok": lok["ready"],
        "detail": "ready" if lok["ready"] else "optional: run omasheets lok status",
        "required": False,
    })
    required = [check for check in checks if check.get("required", True)]
    return {"ready": all(check["ok"] for check in required), "checks": checks}
