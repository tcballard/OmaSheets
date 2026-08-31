"""Local launcher for the installed LibreOfficeKit rendering engine."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import Any

from .errors import EngineError
from .identity import identify_regular_file
from .policy import workbook_format

DEFAULT_PROGRAM = Path("/usr/lib/libreoffice/program")
DEFAULT_HEADERS = Path("/usr/include/libreoffice/LibreOfficeKit/LibreOfficeKit.hxx")
MAX_RENDER_DIMENSION = 4096


def _configured_path(variable: str) -> Path | None:
    value = os.environ.get(variable)
    return Path(value).expanduser() if value else None


def renderer_executable() -> Path | None:
    configured = _configured_path("OMASHEETS_LOK_RENDERER")
    if configured is not None:
        return configured if configured.is_file() and os.access(configured, os.X_OK) else None
    discovered = shutil.which("omasheets-lok-render")
    return Path(discovered) if discovered else None


def status() -> dict[str, Any]:
    program = _configured_path("OMASHEETS_LOK_PROGRAM") or DEFAULT_PROGRAM
    renderer = renderer_executable()
    checks = [
        {"name": "libreofficekit-program", "ok": program.is_dir(), "detail": str(program)},
        {"name": "libreofficekit-headers", "ok": DEFAULT_HEADERS.is_file(), "detail": str(DEFAULT_HEADERS)},
        {
            "name": "omasheets-lok-render",
            "ok": renderer is not None,
            "detail": str(renderer) if renderer else "run the OmaSheets user-local installer",
        },
    ]
    return {"experimental": False, "ready": all(check["ok"] for check in checks), "checks": checks}


def render_workbook(source: Path, destination: Path, *, width: int = 1024, height: int = 640) -> dict[str, Any]:
    resolved_source = source.expanduser().resolve(strict=True)
    workbook_format(resolved_source)
    identify_regular_file(resolved_source)
    if not 1 <= width <= MAX_RENDER_DIMENSION or not 1 <= height <= MAX_RENDER_DIMENSION:
        raise EngineError("render dimensions must be between 1 and 4096")

    resolved_destination = destination.expanduser().resolve(strict=False)
    if resolved_destination.suffix != ".ppm":
        raise EngineError("LibreOfficeKit output must use the .ppm extension")
    if resolved_destination.exists():
        raise EngineError("LibreOfficeKit output already exists")
    if not resolved_destination.parent.is_dir():
        raise EngineError("LibreOfficeKit output directory does not exist")

    renderer = renderer_executable()
    if renderer is None:
        raise EngineError("omasheets-lok-render is not installed; run OmaSheets setup from the Omarchy widget")
    completed = subprocess.run(
        [str(renderer), str(resolved_source), str(resolved_destination), str(width), str(height)],
        check=False,
        capture_output=True,
        close_fds=True,
        text=True,
        timeout=120,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip()[:2000] or "renderer exited without an error message"
        raise EngineError(f"LibreOfficeKit renderer failed: {detail}")
    try:
        report = json.loads(completed.stdout)
    except (json.JSONDecodeError, TypeError) as exc:
        raise EngineError("LibreOfficeKit renderer returned an invalid report") from exc
    if report.get("engine") != "libreofficekit" or not resolved_destination.is_file():
        raise EngineError("LibreOfficeKit renderer did not produce a verified tile")
    return report
