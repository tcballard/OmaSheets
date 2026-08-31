"""Workbook selection and sealed-plan service."""

from __future__ import annotations

import hashlib
import json
import secrets
from dataclasses import asdict
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Protocol

from .errors import ConflictError, EngineError
from .identity import FileIdentity, identify_regular_file
from .operations import destructive_operations, validate_operations
from .paths import AppPaths
from .policy import Actor, require_agent_readable, require_stageable, workbook_format
from .store import read_json, write_json_atomic


class Engine(Protocol):
    def describe(self, source: Path, *, include_formulas: bool) -> dict[str, Any]: ...
    def read_range(self, source: Path, **arguments: Any) -> dict[str, Any]: ...
    def search(self, source: Path, **arguments: Any) -> dict[str, Any]: ...
    def trace(self, source: Path, **arguments: Any) -> dict[str, Any]: ...
    def render(self, source: Path, *, output: Path) -> dict[str, Any]: ...
    def stage(self, source: Path, operations: list[dict[str, Any]], *, output: Path, preview: Path) -> dict[str, Any]: ...


def _now() -> str:
    return datetime.now(UTC).isoformat()


def _canonical_hash(value: dict[str, Any]) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(encoded).hexdigest()


class OmaSheetsService:
    def __init__(self, paths: AppPaths, engine: Engine):
        self.paths = paths
        self.engine = engine
        self.paths.ensure()
        self.sessions = self.paths.state / "sessions"
        self.plans = self.paths.state / "plans"
        self.staging = self.paths.cache / "staging"
        for directory in (self.sessions, self.plans, self.staging):
            directory.mkdir(mode=0o700, parents=True, exist_ok=True)

    @property
    def current_path(self) -> Path:
        return self.paths.state / "current.json"

    def select_workbook(self, path: Path) -> dict[str, Any]:
        """Local-only selection entry point; never exposed over MCP."""

        resolved = path.expanduser().resolve(strict=True)
        fmt = require_agent_readable(resolved)
        identity = identify_regular_file(resolved)
        session_id = secrets.token_hex(16)
        session = {
            "session_id": session_id,
            "source": str(resolved),
            "display_name": resolved.name,
            "format": fmt.value,
            "revision": 1,
            "source_identity": asdict(identity),
            "selected_at": _now(),
        }
        write_json_atomic(self.sessions / f"{session_id}.json", session)
        write_json_atomic(self.current_path, session)
        return self._public_session(session)

    def _session(self, session_id: str) -> dict[str, Any]:
        if not isinstance(session_id, str) or len(session_id) != 32:
            raise ConflictError("invalid workbook session")
        path = self.sessions / f"{session_id}.json"
        if not path.exists():
            raise ConflictError("workbook session not found")
        session = read_json(path)
        current = read_json(self.current_path) if self.current_path.exists() else {}
        if current.get("session_id") != session_id:
            raise ConflictError("workbook is no longer selected")
        self._revalidate_source(session)
        return session

    def _revalidate_source(self, session: dict[str, Any]) -> FileIdentity:
        identity = identify_regular_file(Path(session["source"]))
        if asdict(identity) != session["source_identity"]:
            raise ConflictError("selected workbook changed; select it again")
        return identity

    @staticmethod
    def _public_session(session: dict[str, Any]) -> dict[str, Any]:
        return {
            "session_id": session["session_id"],
            "display_name": session["display_name"],
            "format": session["format"],
            "revision": session["revision"],
            "source_sha256": session["source_identity"]["sha256"],
        }

    def current_resource(self) -> dict[str, Any]:
        if not self.current_path.exists():
            return {"selected": False}
        session = read_json(self.current_path)
        return {"selected": True, **self._public_session(session)}

    def pending_resource(self) -> dict[str, Any]:
        if not self.current_path.exists():
            return {"pending": False}
        session = read_json(self.current_path)
        candidates = []
        for path in self.plans.glob("*.json"):
            plan = read_json(path)
            if plan.get("session_id") == session["session_id"] and plan.get("status") == "verified":
                candidates.append(plan)
        if not candidates:
            return {"pending": False}
        latest = max(candidates, key=lambda plan: plan["created_at"])
        return {"pending": True, **self._public_plan(latest)}

    def describe_workbook(self, session_id: str, include_formulas: bool = False) -> dict[str, Any]:
        session = self._session(session_id)
        result = self.engine.describe(Path(session["source"]), include_formulas=include_formulas)
        result.update(self._public_session(session))
        result["formula_records_included"] = include_formulas
        if not include_formulas:
            result.pop("formulas", None)
        return result

    def read_range(self, session_id: str, **arguments: Any) -> dict[str, Any]:
        session = self._session(session_id)
        return self.engine.read_range(Path(session["source"]), **arguments)

    def search_workbook(self, session_id: str, **arguments: Any) -> dict[str, Any]:
        session = self._session(session_id)
        return self.engine.search(Path(session["source"]), **arguments)

    def trace_formula(self, session_id: str, **arguments: Any) -> dict[str, Any]:
        session = self._session(session_id)
        return self.engine.trace(Path(session["source"]), **arguments)

    def render_workbook(self, session_id: str, format: str = "pdf") -> dict[str, Any]:
        del format
        session = self._session(session_id)
        output = self.paths.cache / "previews" / f"{session['source_identity']['sha256']}.pdf"
        output.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        result = self.engine.render(Path(session["source"]), output=output)
        preview_identity = identify_regular_file(output)
        return {**result, "artifact": str(output), "artifact_sha256": preview_identity.sha256}

    def plan_changes(self, session_id: str, expected_revision: int, operations: list[dict[str, Any]]) -> dict[str, Any]:
        session = self._session(session_id)
        if session["revision"] != expected_revision:
            raise ConflictError("workbook revision is stale")
        require_stageable(Path(session["source"]), actor=Actor.AGENT)
        normalized = validate_operations(operations)
        plan_id = secrets.token_hex(16)
        plan_dir = self.staging / plan_id
        plan_dir.mkdir(mode=0o700, parents=True, exist_ok=False)
        extension = Path(session["source"]).suffix.lower()
        staged = plan_dir / f"staged{extension}"
        preview = plan_dir / "preview.pdf"
        evidence = self.engine.stage(Path(session["source"]), normalized, output=staged, preview=preview)
        staged_identity = identify_regular_file(staged)
        preview_identity = identify_regular_file(preview)
        plan = {
            "plan_id": plan_id,
            "session_id": session_id,
            "revision": expected_revision,
            "status": "verified",
            "created_at": _now(),
            "source_sha256": session["source_identity"]["sha256"],
            "staged_artifact": str(staged),
            "staged_sha256": staged_identity.sha256,
            "preview_artifact": str(preview),
            "preview_sha256": preview_identity.sha256,
            "operations": normalized,
            "destructive_operations": destructive_operations(normalized),
            "semantic_diff": evidence.get("semantic_diff", {}),
            "verification": evidence.get("verification", {}),
            "warnings": evidence.get("warnings", []),
            "engine": evidence.get("engine", {}),
        }
        plan["seal"] = _canonical_hash(plan)
        write_json_atomic(self.plans / f"{plan_id}.json", plan)
        return self._public_plan(plan)

    def get_plan(self, plan_id: str) -> dict[str, Any]:
        return self._public_plan(self._load_plan(plan_id))

    def _load_plan(self, plan_id: str) -> dict[str, Any]:
        if not isinstance(plan_id, str) or len(plan_id) != 32:
            raise ConflictError("invalid plan identifier")
        path = self.plans / f"{plan_id}.json"
        if not path.exists():
            raise ConflictError("plan not found")
        plan = read_json(path)
        sealed = dict(plan)
        seal = sealed.pop("seal", None)
        if not isinstance(seal, str) or not secrets.compare_digest(seal, _canonical_hash(sealed)):
            raise ConflictError("plan seal is invalid")
        return plan

    def apply_plan_handoff(self, plan_id: str, expected_revision: int) -> dict[str, Any]:
        plan = self._load_plan(plan_id)
        if plan["status"] != "verified" or plan["revision"] != expected_revision:
            raise ConflictError("plan is stale or not eligible for review")
        session = self._session(plan["session_id"])
        if session["revision"] != expected_revision:
            raise ConflictError("workbook revision changed")
        if identify_regular_file(Path(plan["staged_artifact"])).sha256 != plan["staged_sha256"]:
            raise ConflictError("staged artifact changed")
        if identify_regular_file(Path(plan["preview_artifact"])).sha256 != plan["preview_sha256"]:
            raise ConflictError("preview artifact changed")
        return {
            "plan_id": plan_id,
            "status": "local_review_required",
            "review_command": ["omasheets", "plan", "approve", plan_id],
            "expected_revision": expected_revision,
            "seal": plan["seal"],
        }

    def change_history(self, session_id: str, since_revision: int | None = None, include_receipts: bool = False) -> dict[str, Any]:
        self._session(session_id)
        plans = []
        for path in self.plans.glob("*.json"):
            plan = self._load_plan(path.stem)
            if plan["session_id"] != session_id:
                continue
            if since_revision is not None and plan["revision"] < since_revision:
                continue
            plans.append(self._public_plan(plan))
        plans.sort(key=lambda plan: plan["created_at"])
        return {"plans": plans, "receipts": [] if include_receipts else None}

    @staticmethod
    def _public_plan(plan: dict[str, Any]) -> dict[str, Any]:
        return {key: value for key, value in plan.items() if key not in {
            "staged_artifact", "preview_artifact"
        }}
