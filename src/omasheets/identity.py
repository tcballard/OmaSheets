"""Stable regular-file identity helpers."""

from __future__ import annotations

import hashlib
import os
import stat
from dataclasses import dataclass
from pathlib import Path

from .errors import ConflictError


@dataclass(frozen=True, slots=True)
class FileIdentity:
    device: int
    inode: int
    size: int
    mtime_ns: int
    sha256: str


def identify_regular_file(path: Path) -> FileIdentity:
    """Hash a stable regular file and reject swaps during the read."""

    before = path.stat(follow_symlinks=False)
    if not stat.S_ISREG(before.st_mode):
        raise ConflictError("workbook must be a regular file, not a link or device")

    digest = hashlib.sha256()
    with path.open("rb") as handle:
        opened = os.fstat(handle.fileno())
        if (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino):
            raise ConflictError("workbook changed while it was opened")
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
        after = os.fstat(handle.fileno())

    observed_before = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
    observed_after = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
    if observed_before != observed_after:
        raise ConflictError("workbook changed while it was hashed")

    return FileIdentity(
        device=after.st_dev,
        inode=after.st_ino,
        size=after.st_size,
        mtime_ns=after.st_mtime_ns,
        sha256=digest.hexdigest(),
    )
