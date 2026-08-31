"""Local desktop launch helpers.

These helpers never pass workbook names through a shell. The Omarchy plugin
uses only the fixed ``open-current`` command, so untrusted workbook names do
not become command text in the long-lived shell process.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

from .errors import EngineError
from .identity import identify_regular_file
from .policy import workbook_format


def calc_executable() -> str:
    executable = shutil.which("libreoffice") or shutil.which("soffice")
    if executable is None:
        raise EngineError("LibreOffice Calc is not installed or not on PATH")
    return executable


def open_workbooks(paths: list[Path]) -> int:
    if not paths:
        raise EngineError("at least one workbook is required")
    resolved: list[Path] = []
    for candidate in paths:
        path = candidate.expanduser().resolve(strict=True)
        workbook_format(path)
        identify_regular_file(path)
        resolved.append(path)
    process = subprocess.Popen(
        [calc_executable(), "--calc", "--", *(str(path) for path in resolved)],
        close_fds=True,
        start_new_session=True,
    )
    return process.pid
