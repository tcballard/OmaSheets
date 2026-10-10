"""Local desktop launch helpers.

These helpers never pass workbook names through a shell. The Omarchy plugin
uses only the fixed ``open-current`` command, so untrusted workbook names do
not become command text in the long-lived shell process.
"""

from __future__ import annotations

from pathlib import Path

from .errors import EngineError
from .native_grid import open_workbook



def open_workbooks(paths: list[Path]) -> int:
    if not paths:
        raise EngineError("at least one workbook is required")
    # Each document has its own supervisor and shares the owned service lease.
    return [open_workbook(path) for path in paths][-1]
