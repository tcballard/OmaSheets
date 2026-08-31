"""Private client for snapshots of the running LibreOfficeKit document."""

from __future__ import annotations

import hashlib
import json
import os
import secrets
import socket
import stat
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from xml.etree import ElementTree

from .errors import EngineError
from .paths import AppPaths

MAX_RESPONSE_BYTES = 4096
_STREAM_CHUNK_BYTES = 1024 * 1024
_IGNORED_VIEW_ELEMENTS = frozenset({"sheetViews", "bookViews"})


@dataclass(frozen=True, slots=True)
class LiveSnapshot:
    path: Path
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


def _hash_field(digest: Any, marker: bytes, value: str) -> None:
    encoded = value.encode("utf-8")
    digest.update(marker)
    digest.update(len(encoded).to_bytes(8, "big"))
    digest.update(encoded)


def _stream_digest(handle: Any) -> bytes:
    digest = hashlib.sha256()
    for chunk in iter(lambda: handle.read(_STREAM_CHUNK_BYTES), b""):
        digest.update(chunk)
    return digest.digest()


def _xml_semantic_digest(handle: Any) -> bytes:
    """Hash one XML member without retaining its document tree or serialization."""

    digest = hashlib.sha256()
    stack: list[tuple[ElementTree.Element, bool]] = []
    for event, element in ElementTree.iterparse(handle, events=("start", "end")):
        if event == "start":
            local_name = element.tag.rsplit("}", 1)[-1]
            ignored = (stack[-1][1] if stack else False) or local_name in _IGNORED_VIEW_ELEMENTS
            stack.append((element, ignored))
            if not ignored:
                _hash_field(digest, b"S", element.tag)
                digest.update(len(element.attrib).to_bytes(8, "big"))
                for key, value in sorted(element.attrib.items()):
                    _hash_field(digest, b"K", key)
                    _hash_field(digest, b"V", value)
            continue

        current, ignored = stack.pop()
        if current is not element:
            raise ElementTree.ParseError("invalid XML element stack")
        if not ignored:
            _hash_field(digest, b"E", element.tag)
            _hash_field(digest, b"T", element.text or "")
            _hash_field(digest, b"L", element.tail or "")

        # iterparse otherwise leaves every completed row/cell attached to its
        # parent until the entire worksheet has been parsed. Detaching here
        # keeps hashing memory proportional to XML nesting depth.
        if stack:
            stack[-1][0].remove(element)
        element.clear()
    if stack:
        raise ElementTree.ParseError("incomplete XML element stack")
    return digest.digest()


def _member_digest(archive: zipfile.ZipFile, name: str) -> tuple[bytes, bytes]:
    if not name.endswith(".xml"):
        with archive.open(name) as handle:
            return b"B", _stream_digest(handle)
    try:
        with archive.open(name) as handle:
            return b"X", _xml_semantic_digest(handle)
    except ElementTree.ParseError:
        # Match the old tolerant behaviour: malformed XML still participates
        # byte-for-byte in conflict detection instead of making the snapshot
        # unreadable solely for hashing purposes.
        with archive.open(name) as handle:
            return b"B", _stream_digest(handle)


def _status_identity(status: os.stat_result) -> tuple[int, ...]:
    """Return the metadata that must remain stable across one fingerprint."""

    return (
        status.st_dev,
        status.st_ino,
        status.st_size,
        status.st_mtime_ns,
        status.st_ctime_ns,
        status.st_uid,
        stat.S_IMODE(status.st_mode),
    )


def _validate_private_snapshot(status: os.stat_result) -> None:
    if status.st_uid != os.getuid() or stat.S_IMODE(status.st_mode) != 0o600:
        raise EngineError("live workbook snapshot permissions are invalid")


def _hash_opened_snapshot(handle: Any, format_name: str) -> str:
    if format_name == "xls":
        return _stream_digest(handle).hex()

    digest = hashlib.sha256()
    digest.update(b"OMASHEETS_SEMANTIC_HASH_V2\0")
    with zipfile.ZipFile(handle) as archive:
        archive_names = archive.namelist()
        if format_name == "ods":
            available = set(archive_names)
            names = [name for name in ("content.xml", "styles.xml") if name in available]
        else:
            names = [
                name for name in archive_names
                if name.startswith("xl/")
                and name not in {"xl/calcChain.xml"}
                and not name.startswith("xl/printerSettings/")
            ]
        if not names:
            raise EngineError("live workbook snapshot has no semantic content")
        for name in sorted(names):
            _hash_field(digest, b"N", name)
            kind, member = _member_digest(archive, name)
            digest.update(kind)
            digest.update(member)
    return digest.hexdigest()


def semantic_snapshot_hash(
    path: Path,
    format_name: str,
    *,
    require_private: bool = False,
) -> str:
    """Stream workbook semantics while excluding save/view metadata churn.

    XML members are reduced to a bounded event stream rather than read into a
    bytes object, parsed into a full DOM, and serialized into a second bytes
    object. Attribute order and namespace prefixes are normalized by the XML
    parser; cell text and structural order remain conflict-sensitive.
    """

    try:
        before = path.stat(follow_symlinks=False)
        if not stat.S_ISREG(before.st_mode):
            raise EngineError("live workbook snapshot is not a regular file")
        if require_private:
            _validate_private_snapshot(before)

        flags = (
            os.O_RDONLY
            | getattr(os, "O_CLOEXEC", 0)
            | getattr(os, "O_NOFOLLOW", 0)
            | getattr(os, "O_NONBLOCK", 0)
        )
        descriptor = os.open(path, flags)
        with os.fdopen(descriptor, "rb") as handle:
            opened = os.fstat(handle.fileno())
            if not stat.S_ISREG(opened.st_mode) or _status_identity(opened) != _status_identity(before):
                raise EngineError("live workbook snapshot changed while it was opened")
            if require_private:
                _validate_private_snapshot(opened)
            try:
                result = _hash_opened_snapshot(handle, format_name)
            finally:
                after = os.fstat(handle.fileno())
                try:
                    final = path.stat(follow_symlinks=False)
                except OSError as error:
                    raise EngineError("live workbook snapshot changed while it was hashed") from error
                if (
                    not stat.S_ISREG(after.st_mode)
                    or not stat.S_ISREG(final.st_mode)
                    or _status_identity(after) != _status_identity(opened)
                    or _status_identity(final) != _status_identity(opened)
                ):
                    raise EngineError("live workbook snapshot changed while it was hashed")
                if require_private:
                    _validate_private_snapshot(after)
                    _validate_private_snapshot(final)
        return result
    except EngineError:
        raise
    except zipfile.BadZipFile as error:
        raise EngineError("live workbook snapshot is not a valid workbook package") from error
    except OSError as error:
        raise EngineError("live workbook snapshot could not be hashed") from error


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
    try:
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
        semantic_sha256 = semantic_snapshot_hash(
            expected,
            format_name,
            require_private=True,
        )
    except Exception:
        try:
            expected.unlink(missing_ok=True)
        except OSError:
            pass
        raise
    return LiveSnapshot(expected, format_name, semantic_sha256)
