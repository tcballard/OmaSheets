"""Local-only publication, receipt chaining, recovery, and undo."""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import re
import secrets
import shutil
from contextlib import contextmanager
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Iterator

from .calc_engine import _copy_no_clobber
from .errors import ConflictError
from .identity import identify_regular_file
from .paths import AppPaths
from .store import read_json, write_json_atomic

_RECEIPT_ID = re.compile(r"^(?:[0-9a-f]{32}|undo-[0-9a-f]{32})$")
_PUBLISH_RECEIPT_ID = re.compile(r"^[0-9a-f]{32}$")


def _now() -> str:
    return datetime.now(UTC).isoformat()


def _canonical_hash(value: dict[str, Any]) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(encoded).hexdigest()


@contextmanager
def exclusive_lock(path: Path) -> Iterator[None]:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        fcntl.flock(descriptor, fcntl.LOCK_UN)
        os.close(descriptor)


def plan_lock(paths: AppPaths, plan_id: str) -> Iterator[None]:
    if not isinstance(plan_id, str) or _PUBLISH_RECEIPT_ID.fullmatch(plan_id) is None:
        raise ConflictError("invalid plan identifier")
    return exclusive_lock(paths.state / "locks" / f"plan-{plan_id}.lock")


def _fsync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _atomic_replace_from(source: Path, destination: Path) -> None:
    temporary = destination.with_name(f".{destination.name}.omasheets-{secrets.token_hex(8)}")
    _copy_no_clobber(source, temporary)
    try:
        os.replace(temporary, destination)
        _fsync_directory(destination.parent)
    finally:
        temporary.unlink(missing_ok=True)


class ReceiptStore:
    def __init__(self, paths: AppPaths):
        self.directory = paths.state / "receipts"
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.head = self.directory / "chain-head.json"
        self.lock = self.directory / ".chain.lock"

    def path(self, receipt_id: str) -> Path:
        if not isinstance(receipt_id, str) or _RECEIPT_ID.fullmatch(receipt_id) is None:
            raise ConflictError("invalid receipt identifier")
        return self.directory / f"{receipt_id}.json"

    def get(self, receipt_id: str) -> dict[str, Any]:
        path = self.path(receipt_id)
        if not path.is_file():
            raise ConflictError("receipt not found")
        receipt = read_json(path)
        sealed = dict(receipt)
        receipt_hash = sealed.pop("receipt_hash", None)
        if not isinstance(receipt_hash, str) or not secrets.compare_digest(receipt_hash, _canonical_hash(sealed)):
            raise ConflictError("receipt integrity check failed")
        return receipt

    def record(self, receipt: dict[str, Any]) -> dict[str, Any]:
        with exclusive_lock(self.lock):
            existing_path = self.path(receipt["receipt_id"])
            if existing_path.exists():
                return self.get(receipt["receipt_id"])
            previous = read_json(self.head) if self.head.exists() else {}
            completed = dict(receipt)
            completed["previous_receipt_hash"] = previous.get("receipt_hash")
            completed["recorded_at"] = _now()
            completed["receipt_hash"] = _canonical_hash(completed)
            write_json_atomic(existing_path, completed)
            write_json_atomic(self.head, {
                "receipt_id": completed["receipt_id"],
                "receipt_hash": completed["receipt_hash"],
            })
            return completed


class Publisher:
    def __init__(self, paths: AppPaths):
        self.paths = paths
        self.receipts = ReceiptStore(paths)
        self.backups = paths.state / "backups"
        self.backups.mkdir(mode=0o700, parents=True, exist_ok=True)

    def publish(self, plan: dict[str, Any], source: Path) -> dict[str, Any]:
        receipt_id = plan["receipt_id"]
        if self.receipts.path(receipt_id).exists():
            return self.receipts.get(receipt_id)
        staged = Path(plan["staged_artifact"])
        target = Path(plan["target_destination"])
        staged_hash = identify_regular_file(staged).sha256
        if staged_hash != plan["staged_sha256"]:
            raise ConflictError("staged artifact changed before publication")

        backup_path: Path | None = None
        if plan["target_mode"] == "copy":
            if target.exists():
                if identify_regular_file(target).sha256 != staged_hash:
                    raise ConflictError("copy destination already contains different bytes")
            else:
                _copy_no_clobber(staged, target)
                _fsync_directory(target.parent)
        elif plan["target_mode"] == "replace":
            if target != source:
                raise ConflictError("replace target must be the selected workbook")
            backup_path = Path(plan["backup_artifact"])
            current_hash = identify_regular_file(source).sha256
            if current_hash == staged_hash:
                if not backup_path.is_file():
                    raise ConflictError("replacement recovery is missing its backup")
            else:
                if current_hash != plan["source_sha256"]:
                    raise ConflictError("source changed before replacement")
                _copy_no_clobber(source, backup_path)
                if identify_regular_file(backup_path).sha256 != plan["source_sha256"]:
                    raise ConflictError("backup verification failed")
                with source.open("rb") as locked:
                    fcntl.flock(locked.fileno(), fcntl.LOCK_EX)
                    if identify_regular_file(source).sha256 != plan["source_sha256"]:
                        raise ConflictError("source changed while acquiring its publication lock")
                    _atomic_replace_from(staged, source)
                    fcntl.flock(locked.fileno(), fcntl.LOCK_UN)
        else:
            raise ConflictError("unknown publication mode")

        result_hash = identify_regular_file(target).sha256
        if result_hash != staged_hash:
            raise ConflictError("published workbook failed hash verification")
        receipt = {
            "receipt_id": receipt_id,
            "kind": "publish",
            "plan_id": plan["plan_id"],
            "session_id": plan["session_id"],
            "revision": plan["revision"],
            "target_mode": plan["target_mode"],
            "target": str(target),
            "source_sha256": plan["source_sha256"],
            "result_sha256": result_hash,
            "backup": str(backup_path) if backup_path else None,
            "backup_sha256": identify_regular_file(backup_path).sha256 if backup_path else None,
            "plan_seal": plan["seal"],
        }
        return self.receipts.record(receipt)

    def undo(self, receipt_id: str, token: str) -> dict[str, Any]:
        if not isinstance(receipt_id, str) or _PUBLISH_RECEIPT_ID.fullmatch(receipt_id) is None:
            raise ConflictError("invalid receipt identifier")
        if token != f"UNDO {receipt_id}":
            raise ConflictError("undo token did not match the receipt")
        original = self.receipts.get(receipt_id)
        if original["kind"] != "publish" or original["target_mode"] != "replace":
            raise ConflictError("only replacement receipts can be undone")
        undo_id = f"undo-{receipt_id}"
        if self.receipts.path(undo_id).exists():
            return self.receipts.get(undo_id)
        target = Path(original["target"])
        backup = Path(original["backup"])
        if identify_regular_file(target).sha256 != original["result_sha256"]:
            raise ConflictError("published workbook changed after the receipt; undo refused")
        if identify_regular_file(backup).sha256 != original["backup_sha256"]:
            raise ConflictError("backup changed after the receipt; undo refused")
        _atomic_replace_from(backup, target)
        restored_hash = identify_regular_file(target).sha256
        if restored_hash != original["source_sha256"]:
            raise ConflictError("undo restoration failed hash verification")
        return self.receipts.record({
            "receipt_id": undo_id,
            "kind": "undo",
            "undoes_receipt_id": receipt_id,
            "target": str(target),
            "before_sha256": original["result_sha256"],
            "result_sha256": restored_hash,
        })
