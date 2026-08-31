"""Workbook selection and sealed-plan service."""

from __future__ import annotations

import hashlib
import json
import re
import secrets
from dataclasses import asdict
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Protocol

from . import __version__
from .errors import ConflictError, EngineError
from .identity import FileIdentity, identify_regular_file
from .operations import SUPPORTED_OPERATIONS, destructive_operations, validate_operations
from .paths import AppPaths
from .policy import Actor, require_agent_readable, require_stageable, workbook_format
from .store import read_json, write_json_atomic
from .transactions import Publisher, plan_lock

_IDENTIFIER = re.compile(r"^[0-9a-f]{32}$")


class Engine(Protocol):
    def describe(self, source: Path, *, include_formulas: bool) -> dict[str, Any]: ...
    def read_range(self, source: Path, **arguments: Any) -> dict[str, Any]: ...
    def search(self, source: Path, **arguments: Any) -> dict[str, Any]: ...
    def trace(self, source: Path, **arguments: Any) -> dict[str, Any]: ...
    def render(self, source: Path, *, output: Path) -> dict[str, Any]: ...
    def stage(self, source: Path, operations: list[dict[str, Any]], *, output: Path, preview: Path) -> dict[str, Any]: ...
    def convert_legacy(self, source: Path, *, destination: Path | None = None, preview: Path) -> dict[str, Any]: ...


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
        self.publisher = Publisher(paths)

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
        if not isinstance(session_id, str) or _IDENTIFIER.fullmatch(session_id) is None:
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

    def capabilities_resource(self) -> dict[str, Any]:
        """Describe the stable product/engine boundary without probing a workbook."""

        return {
            "product": "OmaSheets",
            "version": __version__,
            "product_model": "native_omarchy_shell_with_replaceable_document_engine",
            "libreoffice_fork": False,
            "document_engine": {
                "name": "LibreOffice Calc",
                "adapter": "isolated_uno_worker",
                "interactive_adapter": "libreofficekitgtk",
                "network_access": False,
                "macro_execution": False,
            },
            "live_window_context": {
                "resource": "omasheets://window",
                "agent_control": False,
                "selection_and_viewport_visible": True,
            },
            "agent_operations": list(SUPPORTED_OPERATIONS),
            "agent_publish_authority": False,
            "local_review_required": True,
        }

    @property
    def window_context_path(self) -> Path:
        return self.paths.runtime / "window-context.json"

    def prepare_window_context(self, session_id: str) -> Path:
        """Create the private, path-free handoff consumed by the native window."""

        session = self._session(session_id)
        write_json_atomic(self.window_context_path, {
            "version": 1,
            "active": False,
            "session_id": session_id,
            "revision": session["revision"],
            "sheet": 0,
            "address": "",
            "formula": "",
            "zoom": 1.0,
            "dirty": False,
            "visible": {"x": 0, "y": 0, "width": 0, "height": 0},
            "updated_at_ms": 0,
        })
        return self.window_context_path

    def window_context_resource(self) -> dict[str, Any]:
        """Return bounded live UI context without granting UI or write control."""

        current = self.current_resource()
        if not current.get("selected"):
            return {"active": False, "selected": False}
        if not self.window_context_path.exists():
            return {"active": False, "selected": True, "session_id": current["session_id"]}
        try:
            context = read_json(self.window_context_path)
            if context.get("session_id") != current["session_id"] or context.get("version") != 1:
                raise ValueError("stale context")
            visible = context.get("visible")
            if not isinstance(visible, dict) or set(visible) != {"x", "y", "width", "height"}:
                raise ValueError("invalid visible area")
            if not all(isinstance(visible[name], int) and 0 <= visible[name] <= 2_147_483_647 for name in visible):
                raise ValueError("invalid visible coordinate")
            address = context.get("address")
            formula = context.get("formula")
            if not isinstance(address, str) or len(address) > 64 or not isinstance(formula, str) or len(formula) > 8192:
                raise ValueError("invalid selection")
            sheet = context.get("sheet")
            zoom = context.get("zoom")
            updated_at_ms = context.get("updated_at_ms")
            if not isinstance(sheet, int) or isinstance(sheet, bool) or not 0 <= sheet <= 1024:
                raise ValueError("invalid sheet")
            if not isinstance(zoom, (int, float)) or isinstance(zoom, bool) or not 0.25 <= zoom <= 5.0:
                raise ValueError("invalid zoom")
            if not isinstance(updated_at_ms, int) or isinstance(updated_at_ms, bool) or updated_at_ms < 0:
                raise ValueError("invalid timestamp")
            return {
                "active": context.get("active") is True,
                "selected": True,
                "session_id": current["session_id"],
                "revision": current["revision"],
                "sheet": sheet,
                "address": address,
                "formula": formula,
                "zoom": float(zoom),
                "dirty": context.get("dirty") is True,
                "visible": visible,
                "updated_at_ms": updated_at_ms,
                "agent_control": False,
            }
        except (OSError, ValueError, TypeError):
            return {"active": False, "selected": True, "session_id": current["session_id"], "unavailable": True}

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

    def local_status(self) -> dict[str, Any]:
        """Return a bounded, path-free status record for the Omarchy panel."""

        current = self.current_resource()
        actionable: list[dict[str, Any]] = []
        if current.get("selected"):
            for path in self.plans.glob("*.json"):
                plan = self._load_plan(path.stem)
                if plan.get("session_id") == current["session_id"] and plan.get("status") in {
                    "verified", "review_pending", "approved"
                }:
                    actionable.append(plan)
        latest = max(actionable, key=lambda plan: plan["created_at"]) if actionable else None
        pending = {"pending": False}
        if latest:
            verification = latest.get("verification") or {}
            pending = {
                "pending": True,
                "plan_id": latest["plan_id"],
                "revision": latest["revision"],
                "status": latest["status"],
                "operation_count": len(latest.get("operations", [])),
                "destructive_count": len(latest.get("destructive_operations", [])),
                "warning_count": len(latest.get("warnings", [])),
                "formula_error_count": len(verification.get("formula_errors", [])),
            }
        return {
            "version": __version__,
            "current": current,
            "review": pending,
            "agent_commit_authority": False,
        }

    def current_local_path(self) -> Path:
        """Return and revalidate the locally selected source without exposing it to MCP."""

        if not self.current_path.exists():
            raise ConflictError("no workbook is selected")
        session = read_json(self.current_path)
        self._revalidate_source(session)
        return Path(session["source"])

    def convert_legacy_local(self, source: Path) -> dict[str, Any]:
        """Convert an explicitly chosen `.xls` to an adjacent, new `.xlsx`."""

        from .policy import conversion_destination

        resolved = source.expanduser().resolve(strict=True)
        source_identity = identify_regular_file(resolved)
        destination = conversion_destination(resolved)
        receipt_id = secrets.token_hex(16)
        preview = self.paths.cache / "conversions" / f"{receipt_id}.pdf"
        preview.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        result = self.engine.convert_legacy(resolved, destination=destination, preview=preview)
        destination_identity = identify_regular_file(destination)
        preview_identity = identify_regular_file(preview)
        receipt = self.publisher.receipts.record({
            "receipt_id": receipt_id,
            "kind": "conversion",
            "source": str(resolved),
            "source_sha256": source_identity.sha256,
            "target": str(destination),
            "result_sha256": destination_identity.sha256,
            "preview": str(preview),
            "preview_sha256": preview_identity.sha256,
            "manual_review_required": True,
            "excel_equivalence_claimed": False,
            "engine": result.get("engine", {}),
            "comparison": result.get("comparison", {}),
            "warnings": result.get("warnings", []),
        })
        return receipt

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
        window = self.window_context_resource()
        if window.get("active") and window.get("session_id") == session_id and window.get("dirty"):
            raise ConflictError("native window has unsaved changes; save a copy or close it before staging agent changes")
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
        self._save_plan(plan)
        return self._public_plan(plan)

    def _save_plan(self, plan: dict[str, Any]) -> None:
        sealed = dict(plan)
        sealed.pop("seal", None)
        plan["seal"] = _canonical_hash(sealed)
        write_json_atomic(self.plans / f"{plan['plan_id']}.json", plan)

    def get_plan(self, plan_id: str) -> dict[str, Any]:
        return self._public_plan(self._load_plan(plan_id))

    def _load_plan(self, plan_id: str) -> dict[str, Any]:
        if not isinstance(plan_id, str) or _IDENTIFIER.fullmatch(plan_id) is None:
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

    def prepare_local_review(
        self,
        plan_id: str,
        expected_revision: int,
        *,
        mode: str = "copy",
        destination: Path | None = None,
    ) -> dict[str, Any]:
        """Seal a local publication target before asking for approval."""

        with plan_lock(self.paths, plan_id):
            plan = self._load_plan(plan_id)
            if plan["status"] not in ("verified", "review_pending"):
                raise ConflictError("plan is not eligible for local review")
            if plan["revision"] != expected_revision:
                raise ConflictError("plan revision is stale")
            session = self._session(plan["session_id"])
            source = Path(session["source"])
            receipt_id = plan.get("receipt_id") or secrets.token_hex(16)
            if mode == "copy":
                target = destination or source.with_name(f"{source.stem}-omasheets{source.suffix.lower()}")
                target = target.expanduser().resolve(strict=False)
                if target == source:
                    raise ConflictError("copy target cannot be the selected workbook")
                if target.suffix.lower() != source.suffix.lower():
                    raise ConflictError("copy target must retain the workbook format")
                backup = None
            elif mode == "replace":
                if destination is not None and destination.expanduser().resolve(strict=False) != source:
                    raise ConflictError("replace target must be the selected workbook")
                target = source
                backup = self.publisher.backups / f"{receipt_id}{source.suffix.lower()}"
            else:
                raise ConflictError("publication mode must be copy or replace")
            plan.update({
                "status": "review_pending",
                "target_mode": mode,
                "target_destination": str(target),
                "backup_artifact": str(backup) if backup else None,
                "receipt_id": receipt_id,
                "review_prepared_at": _now(),
            })
            self._save_plan(plan)
            return {
                "plan_id": plan_id,
                "expected_revision": expected_revision,
                "target_mode": mode,
                "destination": str(target),
                "source_sha256": plan["source_sha256"],
                "staged_sha256": plan["staged_sha256"],
                "preview_sha256": plan["preview_sha256"],
                "semantic_diff": plan["semantic_diff"],
                "verification": plan["verification"],
                "warnings": plan["warnings"],
                "destructive_operations": plan["destructive_operations"],
                "seal": plan["seal"],
                "approval_token": f"APPLY {plan_id}",
            }

    def commit_local_review(self, plan_id: str, expected_revision: int, token: str) -> dict[str, Any]:
        with plan_lock(self.paths, plan_id):
            plan = self._load_plan(plan_id)
            if plan["status"] == "committed":
                return self.publisher.receipts.get(plan["receipt_id"])
            if plan["status"] not in ("review_pending", "approved"):
                raise ConflictError("plan is not awaiting local approval")
            if token != f"APPLY {plan_id}":
                raise ConflictError("approval token did not match the plan")
            if plan["revision"] != expected_revision:
                raise ConflictError("plan revision is stale")
            session = self._session(plan["session_id"])
            plan["status"] = "approved"
            plan["approved_at"] = _now()
            self._save_plan(plan)
            try:
                receipt = self.publisher.publish(plan, Path(session["source"]))
            except ConflictError:
                plan["status"] = "conflicted"
                plan["conflicted_at"] = _now()
                self._save_plan(plan)
                raise
            except Exception:
                # Keep the durable approved journal recoverable. A retry can
                # recognize already-published bytes and finish the receipt.
                plan["last_publish_error_at"] = _now()
                self._save_plan(plan)
                raise
            plan["status"] = "committed"
            plan["committed_at"] = _now()
            self._save_plan(plan)
            return receipt

    def undo_receipt(self, receipt_id: str, token: str) -> dict[str, Any]:
        """Local-only undo entry point; never exposed through AgentService."""

        return self.publisher.undo(receipt_id, token)

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
            "staged_artifact", "preview_artifact", "target_destination", "backup_artifact"
        }}
