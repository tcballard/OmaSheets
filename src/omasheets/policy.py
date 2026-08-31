"""Central file-format and authority policy.

The MCP layer and local CLI both call this module. Keeping the rules here avoids
accidentally granting an agent more authority through a second entry point.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from pathlib import Path

from .errors import PolicyError


class WorkbookFormat(str, Enum):
    XLS = "xls"
    XLSX = "xlsx"
    XLSM = "xlsm"
    ODS = "ods"


class Actor(str, Enum):
    AGENT = "agent"
    LOCAL = "local"


class PublishMode(str, Enum):
    COPY = "copy"
    REPLACE = "replace"


@dataclass(frozen=True, slots=True)
class FormatPolicy:
    human_open: bool
    agent_read: bool
    agent_stage: bool
    publish: bool
    conversion_only: bool = False


POLICIES: dict[WorkbookFormat, FormatPolicy] = {
    WorkbookFormat.XLS: FormatPolicy(True, True, False, False, True),
    WorkbookFormat.XLSX: FormatPolicy(True, True, True, True),
    WorkbookFormat.XLSM: FormatPolicy(True, True, False, False),
    WorkbookFormat.ODS: FormatPolicy(True, True, True, True),
}


def workbook_format(path: Path) -> WorkbookFormat:
    """Return a supported format from a filename without opening it."""

    suffix = path.suffix.lower().removeprefix(".")
    try:
        return WorkbookFormat(suffix)
    except ValueError as exc:
        raise PolicyError(f"unsupported workbook format: {path.suffix or '<none>'}") from exc


def require_agent_readable(path: Path) -> WorkbookFormat:
    fmt = workbook_format(path)
    if not POLICIES[fmt].agent_read:
        raise PolicyError(f"{fmt.value} is not agent-readable")
    return fmt


def require_stageable(path: Path, *, actor: Actor) -> WorkbookFormat:
    fmt = workbook_format(path)
    if actor is Actor.AGENT and not POLICIES[fmt].agent_stage:
        raise PolicyError(f"agents cannot stage changes to .{fmt.value} workbooks")
    if not POLICIES[fmt].publish:
        raise PolicyError(f".{fmt.value} workbooks are read-only in v0.0.1")
    return fmt


def require_publish_authority(*, actor: Actor, mode: PublishMode) -> None:
    if actor is Actor.AGENT:
        raise PolicyError("agents cannot publish workbook bytes")
    if mode is PublishMode.REPLACE and actor is not Actor.LOCAL:
        raise PolicyError("replace requires local authority")


def conversion_destination(source: Path) -> Path:
    """Return the only permitted default conversion destination for `.xls`."""

    if workbook_format(source) is not WorkbookFormat.XLS:
        raise PolicyError("only .xls inputs use the legacy conversion flow")
    return source.with_suffix(".xlsx")
