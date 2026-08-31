"""Reversible, user-local desktop and MIME integration."""

from __future__ import annotations

import base64
import hashlib
import os
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .errors import ConflictError
from .store import read_json, write_json_atomic

DESKTOP_ID = "io.github.tcballard.OmaSheets.desktop"
MIME_TYPES = (
    "application/vnd.ms-excel",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.ms-excel.sheet.macroEnabled.12",
    "application/vnd.oasis.opendocument.spreadsheet",
)

DESKTOP_ENTRY = """[Desktop Entry]
Type=Application
Name=OmaSheets
Comment=Open spreadsheets in native LibreOffice Calc
Exec=omasheets open %U
Icon=x-office-spreadsheet
Terminal=false
StartupNotify=true
Categories=Office;Spreadsheet;
MimeType=application/vnd.ms-excel;application/vnd.openxmlformats-officedocument.spreadsheetml.sheet;application/vnd.ms-excel.sheet.macroEnabled.12;application/vnd.oasis.opendocument.spreadsheet;
Keywords=spreadsheet;xls;xlsx;ods;calc;
"""


@dataclass(frozen=True, slots=True)
class IntegrationPaths:
    desktop: Path
    mimeapps: Path
    journal: Path

    @classmethod
    def discover(cls) -> "IntegrationPaths":
        home = Path.home()
        data = Path(os.environ.get("XDG_DATA_HOME", home / ".local/share"))
        config = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
        state = Path(os.environ.get("XDG_STATE_HOME", home / ".local/state"))
        return cls(
            desktop=data / "applications" / DESKTOP_ID,
            mimeapps=config / "mimeapps.list",
            journal=state / "omasheets" / "desktop-integration.json",
        )


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _atomic_bytes(path: Path, data: bytes, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.fchmod(descriptor, mode)
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def _mime_values(text: str, section: str, key: str) -> list[str] | None:
    active = ""
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            active = stripped[1:-1]
        elif active == section and "=" in line and not stripped.startswith(("#", ";")):
            found, raw = line.split("=", 1)
            if found.strip() == key:
                return [item for item in raw.split(";") if item]
    return None


def _set_mime_values(text: str, section: str, key: str, values: list[str] | None) -> str:
    lines = text.splitlines()
    section_start = None
    section_end = len(lines)
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped == f"[{section}]":
            section_start = index
            continue
        if section_start is not None and index > section_start and stripped.startswith("[") and stripped.endswith("]"):
            section_end = index
            break
    if section_start is None:
        if values is None:
            return text
        if lines and lines[-1] != "":
            lines.append("")
        lines.extend((f"[{section}]", f"{key}={';'.join(values)};"))
        return "\n".join(lines) + "\n"
    for index in range(section_start + 1, section_end):
        line = lines[index]
        if "=" in line and not line.lstrip().startswith(("#", ";")) and line.split("=", 1)[0].strip() == key:
            if values is None:
                del lines[index]
            else:
                lines[index] = f"{key}={';'.join(values)};"
            return "\n".join(lines) + "\n"
    if values is not None:
        lines.insert(section_end, f"{key}={';'.join(values)};")
    return "\n".join(lines) + "\n"


def _integrated_mimeapps(before: bytes) -> bytes:
    text = before.decode("utf-8") if before else ""
    for section in ("Default Applications", "Added Associations"):
        for mime in MIME_TYPES:
            values = _mime_values(text, section, mime) or []
            values = [value for value in values if value != DESKTOP_ID]
            if section == "Default Applications":
                values.insert(0, DESKTOP_ID)
            else:
                values.append(DESKTOP_ID)
            text = _set_mime_values(text, section, mime, values)
    return text.encode("utf-8")


def _refresh_desktop_database(desktop: Path) -> None:
    executable = shutil.which("update-desktop-database")
    if executable:
        subprocess.run([executable, str(desktop.parent)], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def install(paths: IntegrationPaths | None = None) -> dict[str, Any]:
    paths = paths or IntegrationPaths.discover()
    desired_desktop = DESKTOP_ENTRY.encode()
    if paths.journal.exists():
        journal = read_json(paths.journal)
        desktop_ok = paths.desktop.exists() and _sha(paths.desktop.read_bytes()) == journal.get("desktop_after_sha256")
        mime_ok = paths.mimeapps.exists() and _sha(paths.mimeapps.read_bytes()) == journal.get("mimeapps_after_sha256")
        if desktop_ok and mime_ok:
            return {"installed": True, "changed": False, "desktop_id": DESKTOP_ID}
        raise ConflictError("desktop integration changed since installation; uninstall or resolve it first")
    if paths.desktop.exists() and paths.desktop.read_bytes() != desired_desktop:
        raise ConflictError(f"refusing to overwrite existing desktop entry: {paths.desktop}")

    desktop_before = paths.desktop.read_bytes() if paths.desktop.exists() else None
    mime_before = paths.mimeapps.read_bytes() if paths.mimeapps.exists() else None
    mime_after = _integrated_mimeapps(mime_before or b"")
    _atomic_bytes(paths.desktop, desired_desktop, 0o644)
    _atomic_bytes(paths.mimeapps, mime_after)
    journal = {
        "schema": 1,
        "desktop_id": DESKTOP_ID,
        "desktop_before": base64.b64encode(desktop_before).decode() if desktop_before is not None else None,
        "desktop_after_sha256": _sha(desired_desktop),
        "mimeapps_before": base64.b64encode(mime_before).decode() if mime_before is not None else None,
        "mimeapps_after_sha256": _sha(mime_after),
    }
    write_json_atomic(paths.journal, journal)
    _refresh_desktop_database(paths.desktop)
    return {"installed": True, "changed": True, "desktop_id": DESKTOP_ID}


def uninstall(paths: IntegrationPaths | None = None) -> dict[str, Any]:
    paths = paths or IntegrationPaths.discover()
    if not paths.journal.exists():
        return {"installed": False, "changed": False, "desktop_id": DESKTOP_ID}
    journal = read_json(paths.journal)
    conflicts: list[str] = []
    if paths.desktop.exists():
        if _sha(paths.desktop.read_bytes()) == journal["desktop_after_sha256"]:
            previous = journal.get("desktop_before")
            if previous is None:
                paths.desktop.unlink()
            else:
                _atomic_bytes(paths.desktop, base64.b64decode(previous), 0o644)
        else:
            conflicts.append(str(paths.desktop))
    if paths.mimeapps.exists():
        current = paths.mimeapps.read_bytes()
        if _sha(current) == journal["mimeapps_after_sha256"]:
            previous = journal.get("mimeapps_before")
            if previous is None:
                paths.mimeapps.unlink()
            else:
                _atomic_bytes(paths.mimeapps, base64.b64decode(previous))
        else:
            text = current.decode("utf-8")
            previous_bytes = base64.b64decode(journal["mimeapps_before"]) if journal.get("mimeapps_before") else b""
            previous_text = previous_bytes.decode("utf-8")
            for section in ("Default Applications", "Added Associations"):
                for mime in MIME_TYPES:
                    values = _mime_values(text, section, mime)
                    previous_values = _mime_values(previous_text, section, mime) or []
                    if values is not None and DESKTOP_ID in values and DESKTOP_ID not in previous_values:
                        remaining = [value for value in values if value != DESKTOP_ID]
                        text = _set_mime_values(text, section, mime, remaining or None)
            _atomic_bytes(paths.mimeapps, text.encode())
    if conflicts:
        raise ConflictError("modified integration file was preserved: " + ", ".join(conflicts))
    paths.journal.unlink()
    _refresh_desktop_database(paths.desktop)
    return {"installed": False, "changed": True, "desktop_id": DESKTOP_ID}
