"""Provider-neutral launch boundary for a local OmaSheets agent session."""

from __future__ import annotations

import shutil
import subprocess
from typing import Callable

from .errors import EngineError

AGENT_SESSION_PROMPT = (
    "Use the installed OmaSheets agent integration to help with my locally selected workbook. "
    "Start from omasheets://session. If OmaSheets MCP tools are unavailable, use "
    "`omasheets agent-session resource`, `omasheets agent-session tools`, and the bounded "
    "`omasheets agent-session call` bridge. Inspect the evidence you need, clarify material "
    "ambiguity, and propose a verified plan. For workbook-wide analysis or a management "
    "summary, run analyze_workbook first, cite its findings, and use typed chart and pivot "
    "operations where useful. Never publish workbook bytes."
)


def launch_agent_session(
    *,
    which: Callable[[str], str | None] = shutil.which,
    launcher: Callable[..., subprocess.Popen[bytes]] = subprocess.Popen,
) -> int:
    """Ask Omarchy to launch its configured default agent with a fixed prompt."""

    omarchy = which("omarchy")
    if not omarchy:
        raise EngineError("Omarchy's default-agent launcher is unavailable")
    process = launcher(
        [omarchy, "agent", "prompt", AGENT_SESSION_PROMPT],
        close_fds=True,
        start_new_session=True,
    )
    return process.pid
