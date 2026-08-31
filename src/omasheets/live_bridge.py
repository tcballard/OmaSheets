"""Private client for snapshots of the running LibreOfficeKit document."""

from __future__ import annotations

import json
import hashlib
import os
import secrets
import socket
import stat
import zipfile
from xml.etree import ElementTree
from dataclasses import dataclass
from pathlib import Path

from .errors import EngineError
from .identity import FileIdentity, identify_regular_file
from .paths import AppPaths

MAX_RESPONSE_BYTES = 4096


@dataclass(frozen=True, slots=True)
class LiveSnapshot:
    path: Path
    identity: FileIdentity
    format: str
    semantic_sha256: str


def bridge_path(paths: AppPaths) -> Path:
    return paths.runtime / "window-bridge.sock"


def _validate_socket(path: Path) -> None:
    try:
        status = path.stat(follow_symlinks=False)
    except OSError as error:
        raise EngineError("live workbook bridge is unavailable") from error
    if not stat.S_ISSOCK(status.st_mode) or status.st_uid != os.getuid():
        raise EngineError("live workbook bridge is not a same-user Unix socket")
    if stat.S_IMODE(status.st_mode) & 0o077:
        raise EngineError("live workbook bridge permissions are too broad")


def _without_view_state(content: bytes) -> bytes:
    try:
        root = ElementTree.fromstring(content)
    except ElementTree.ParseError:
        return content
    for parent in root.iter():
        for child in list(parent):
            local_name = child.tag.rsplit("}", 1)[-1]
            if local_name in {"sheetViews", "bookViews"}:
                parent.remove(child)
    return ElementTree.tostring(root, encoding="utf-8")


def semantic_snapshot_hash(path: Path, format_name: str) -> str:
    """Hash workbook semantics while excluding save/view metadata churn."""

    digest = hashlib.sha256()
    try:
        with zipfile.ZipFile(path) as archive:
            if format_name == "ods":
                names = [name for name in ("content.xml", "styles.xml") if name in archive.namelist()]
            else:
                names = [
                    name for name in archive.namelist()
                    if name.startswith("xl/")
                    and name not in {"xl/calcChain.xml"}
                    and not name.startswith("xl/printerSettings/")
                ]
            if not names:
                raise EngineError("live workbook snapshot has no semantic content")
            for name in sorted(names):
                content = archive.read(name)
                if name.endswith(".xml"):
                    content = _without_view_state(content)
                digest.update(name.encode("utf-8"))
                digest.update(b"\0")
                digest.update(content)
                digest.update(b"\0")
    except (OSError, zipfile.BadZipFile) as error:
        raise EngineError("live workbook snapshot is not a valid workbook package") from error
    return digest.hexdigest()


def request_live_snapshot(paths: AppPaths, session_id: str, suffix: str) -> LiveSnapshot:
    """Ask the window for an immutable save-copy of its in-memory document."""

    if len(session_id) != 32 or any(character not in "0123456789abcdef" for character in session_id):
        raise EngineError("invalid live workbook session")
    format_name = suffix.lower().removeprefix(".")
    if format_name not in {"xls", "xlsx", "xlsm", "ods"}:
        raise EngineError("unsupported live workbook format")
    nonce = secrets.token_hex(16)
    socket_path = bridge_path(paths)
    _validate_socket(socket_path)
    expected = paths.runtime / f"snapshot-{session_id}-{nonce}.{format_name}"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(10.0)
        connection.connect(str(socket_path))
        connection.sendall(f"SNAPSHOT {session_id} {nonce}\n".encode("ascii"))
        response = bytearray()
        while not response.endswith(b"\n"):
            chunk = connection.recv(MAX_RESPONSE_BYTES + 1 - len(response))
            if not chunk:
                raise EngineError("live workbook bridge closed without a response")
            response.extend(chunk)
            if len(response) > MAX_RESPONSE_BYTES:
                raise EngineError("live workbook bridge response exceeded its limit")
    try:
        payload = json.loads(response)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise EngineError("live workbook bridge returned invalid JSON") from error
    if payload != {"ok": True, "format": format_name}:
        detail = payload.get("error", "snapshot failed") if isinstance(payload, dict) else "snapshot failed"
        raise EngineError(f"live workbook snapshot failed: {detail}")
    identity = identify_regular_file(expected)
    status = expected.stat(follow_symlinks=False)
    if status.st_uid != os.getuid() or stat.S_IMODE(status.st_mode) != 0o600:
        expected.unlink(missing_ok=True)
        raise EngineError("live workbook snapshot permissions are invalid")
    return LiveSnapshot(expected, identity, format_name, semantic_snapshot_hash(expected, format_name))
