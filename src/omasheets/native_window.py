"""Launcher for the experimental OmaSheets-owned workbook window."""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path
from typing import Any

from .errors import EngineError
from .identity import identify_regular_file
from .policy import workbook_format


def window_executable() -> Path | None:
    configured = os.environ.get("OMASHEETS_WINDOW")
    if configured:
        candidate = Path(configured).expanduser()
        return candidate if candidate.is_file() and os.access(candidate, os.X_OK) else None
    discovered = shutil.which("omasheets-window")
    return Path(discovered) if discovered else None


def status() -> dict[str, Any]:
    executable = window_executable()
    return {
        "experimental": True,
        "ready": executable is not None,
        "executable": str(executable) if executable else None,
        "detail": str(executable) if executable else "build and install spikes/libreofficekit",
    }


def open_window(
    path: Path,
    *,
    context_path: Path | None = None,
    session_id: str | None = None,
    revision: int | None = None,
) -> int:
    source = path.expanduser().resolve(strict=True)
    workbook_format(source)
    identify_regular_file(source)
    executable = window_executable()
    if executable is None:
        raise EngineError("omasheets-window is not installed; see spikes/libreofficekit/README.md")
    argv = [str(executable)]
    supplied = (context_path is not None, session_id is not None, revision is not None)
    if any(supplied) and not all(supplied):
        raise EngineError("window context requires path, session and revision")
    if context_path is not None:
        argv.extend(["--context", str(context_path), "--session", session_id, "--revision", str(revision)])
    argv.append(str(source))
    process = subprocess.Popen(
        argv,
        close_fds=True,
        start_new_session=True,
    )
    return process.pid
