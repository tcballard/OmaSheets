"""Explicit, reversible systemd user-service integration for Omarchy."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from typing import Any

from .errors import ConflictError
from .store import read_json, write_json_atomic

UNIT_NAME = "omasheets-native.service"


@dataclass(frozen=True, slots=True)
class UserServicePaths:
    binary: Path
    unit: Path
    journal: Path

    @classmethod
    def discover(cls) -> "UserServicePaths":
        home = Path.home()
        data = Path(os.environ.get("XDG_DATA_HOME", home / ".local/share"))
        config = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
        state = Path(os.environ.get("XDG_STATE_HOME", home / ".local/state"))
        return cls(
            binary=data / "omasheets/app/bin/omasheets-service",
            unit=config / "systemd/user" / UNIT_NAME,
            journal=state / "omasheets/user-service.json",
        )


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _systemd_argument(path: Path) -> str:
    raw = str(path)
    if any(character in "\r\n\0" for character in raw):
        raise ValueError("service binary path contains a control character")
    escaped = raw.replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%")
    return f'"{escaped}"'


def unit_file(binary: Path) -> bytes:
    command = _systemd_argument(binary.resolve(strict=True))
    return (
        "[Unit]\n"
        "Description=OmaSheets native document service\n\n"
        "[Service]\n"
        "Type=simple\n"
        f"ExecStart={command} serve\n"
        "Restart=on-failure\n"
        "RestartSec=1s\n"
        "UMask=0077\n\n"
        "[Install]\n"
        "WantedBy=default.target\n"
    ).encode()


def _write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.fchmod(descriptor, 0o644)
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


def _systemctl(*arguments: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    executable = shutil.which("systemctl")
    if executable is None:
        raise RuntimeError("systemctl is required for Omarchy user-service setup")
    return subprocess.run(
        [executable, "--user", *arguments],
        text=True,
        capture_output=True,
        check=check,
    )


def install(paths: UserServicePaths | None = None, *, enable: bool = False) -> dict[str, Any]:
    paths = paths or UserServicePaths.discover()
    if not paths.binary.is_file() or not os.access(paths.binary, os.X_OK):
        raise RuntimeError("the installed omasheets-service binary is missing or not executable")
    desired = unit_file(paths.binary)
    if paths.journal.is_file():
        journal = read_json(paths.journal)
        if not paths.unit.is_file() or _sha(paths.unit.read_bytes()) != journal["unit_sha256"]:
            raise ConflictError("OmaSheets user-service unit changed since setup")
        if enable:
            _systemctl("enable", "--now", UNIT_NAME)
        return {
            "installed": True,
            "changed": False,
            "enabled": enable,
            "unit": str(paths.unit),
        }
    if paths.unit.exists():
        raise ConflictError(f"refusing to overwrite existing user-service unit: {paths.unit}")

    try:
        _write(paths.unit, desired)
        write_json_atomic(paths.journal, {
            "schema": 1,
            "unit": UNIT_NAME,
            "unit_sha256": _sha(desired),
            "binary": str(paths.binary.resolve(strict=True)),
        })
        _systemctl("daemon-reload")
        if enable:
            _systemctl("enable", "--now", UNIT_NAME)
    except Exception:
        if enable:
            try:
                _systemctl("disable", "--now", UNIT_NAME, check=False)
            except Exception:
                pass
        paths.unit.unlink(missing_ok=True)
        paths.journal.unlink(missing_ok=True)
        try:
            _systemctl("daemon-reload", check=False)
        except Exception:
            pass
        raise
    return {
        "installed": True,
        "changed": True,
        "enabled": enable,
        "unit": str(paths.unit),
    }


def uninstall(paths: UserServicePaths | None = None) -> dict[str, Any]:
    paths = paths or UserServicePaths.discover()
    if not paths.journal.is_file():
        return {"installed": False, "changed": False, "conflicts": []}
    journal = read_json(paths.journal)
    stopped = _systemctl("disable", "--now", UNIT_NAME, check=False)
    if stopped.returncode != 0:
        raise ConflictError("could not stop the OmaSheets user service before uninstalling")
    unit_bytes = paths.unit.read_bytes() if paths.unit.exists() else None
    if unit_bytes is not None and _sha(unit_bytes) != journal["unit_sha256"]:
        raise ConflictError(f"modified user-service unit was preserved: {paths.unit}")
    paths.unit.unlink(missing_ok=True)
    try:
        _systemctl("daemon-reload")
    except Exception:
        if unit_bytes is not None:
            _write(paths.unit, unit_bytes)
        raise ConflictError("could not reload the user service manager during uninstall")
    paths.journal.unlink()
    return {"installed": False, "changed": True, "conflicts": []}
