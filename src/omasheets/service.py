"""Workbook selection and sealed-plan service."""

from __future__ import annotations

import hashlib
import json
import os
import re
import secrets
from contextlib import contextmanager
from dataclasses import asdict
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Protocol

from . import __version__
from .diff_overlay import decode_overlay, overlay_path, publish_overlay
from .errors import ConflictError, EngineError
from .identity import FileIdentity, identify_regular_file
from .live_bridge import request_live_snapshot
from .operations import SUPPORTED_OPERATIONS, destructive_operations, validate_operations
from .paths import AppPaths
from .policy import Actor, require_agent_readable, require_stageable, workbook_format
from .store import read_json, write_json_atomic
from .transactions import Publisher, plan_lock
from .workflow import validate_workflow

_IDENTIFIER = re.compile(r"^[0-9a-f]{32}$")


class Engine(Protocol):
    def describe(self, source: Path, *, include_formulas: bool) -> dict[str, Any]: ...
    def read_range(self, source: Path, **arguments: Any) -> dict[str, Any]: ...
    def search(self, source: Path, **arguments: Any) -> dict[str, Any]: ...
    def trace(self, source: Path, **arguments: Any) -> dict[str, Any]: ...
    def analyze(self, source: Path, **arguments: Any) -> dict[str, Any]: ...
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
        self.evidence = self.paths.state / "evidence"
        self.staging = self.paths.cache / "staging"
        for directory in (self.sessions, self.plans, self.evidence, self.staging):
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
            "live_document_bridge": {
                "transport": "private_same_user_unix_socket",
                "source": "libreofficekit_save_copy",
                "unsaved_state_visible": True,
                "agent_mutates_open_document": False,
            },
            "agent_diff_overlay": {
                "native": True,
                "verified_before_after_values": True,
                "cited_audit_findings": True,
                "maximum_visible_changes": 200,
                "mutates_open_document": False,
            },
            "agent_operations": list(SUPPORTED_OPERATIONS),
            "agent_publish_authority": False,
            "local_review_required": True,
        }

    def agent_session_resource(self) -> dict[str, Any]:
        """Give a newly opened agent a path-free, selection-aware starting point."""

        current = self.current_resource()
        if not current.get("selected"):
            return {
                "ready": False,
                "instruction": "Select a workbook locally in OmaSheets before starting an agent session.",
                "agent_publish_authority": False,
            }
        window = self.window_context_resource()
        focus = None
        if window.get("active"):
            focus = {
                "sheet_index": window["sheet"],
                "address": window["address"],
                "formula": window["formula"],
                "visible": window["visible"],
                "dirty": window["dirty"],
                "live_document_bridge": window["live_document_bridge"],
            }
        return {
            "ready": True,
            "workbook": current,
            "focus": focus,
            "suggested_workflows": [
                {"id": "analyse", "label": "Audit the whole workbook"},
                {"id": "management", "label": "Build a management summary with pivots and charts"},
                {"id": "explain", "label": "Explain this selection or formula"},
                {"id": "clean", "label": "Clean and standardise a data range"},
                {"id": "variance", "label": "Build or explain a variance analysis"},
                {"id": "reconcile", "label": "Reconcile values across sheets"},
                {"id": "summarise", "label": "Create a checked summary"},
                {"id": "format", "label": "Standardise presentation without changing values"},
            ],
            "workflow_contract": {
                "inspect_before_planning": True,
                "cite_returned_evidence_ids": True,
                "group_operations_by_purpose": True,
                "revise_instead_of_mutating_verified_plans": True,
                "local_review_required": True,
                "agent_publish_authority": False,
            },
        }

    @property
    def window_context_path(self) -> Path:
        return self.paths.runtime / "window-context.json"

    def prepare_window_context(self, session_id: str) -> Path:
        """Create the private, path-free handoff consumed by the native window."""

        session = self._session(session_id)
        overlay_path(self.paths.runtime).unlink(missing_ok=True)
        write_json_atomic(self.window_context_path, {
            "version": 1,
            "active": False,
            "session_id": session_id,
            "revision": session["revision"],
            "sheet": 0,
            "address": "",
            "formula": "",
            "live_document_bridge": False,
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
            live_document_bridge = context.get("live_document_bridge", False)
            if not isinstance(sheet, int) or isinstance(sheet, bool) or not 0 <= sheet <= 1024:
                raise ValueError("invalid sheet")
            if not isinstance(zoom, (int, float)) or isinstance(zoom, bool) or not 0.25 <= zoom <= 5.0:
                raise ValueError("invalid zoom")
            if not isinstance(updated_at_ms, int) or isinstance(updated_at_ms, bool) or updated_at_ms < 0:
                raise ValueError("invalid timestamp")
            if not isinstance(live_document_bridge, bool):
                raise ValueError("invalid bridge status")
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
                "live_document_bridge": live_document_bridge,
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

    @contextmanager
    def _agent_source(self, session: dict[str, Any]):
        """Yield the exact live or on-disk bytes agents are allowed to inspect."""

        window = self.window_context_resource()
        if window.get("active") and window.get("session_id") == session["session_id"]:
            if not window.get("live_document_bridge"):
                raise EngineError("native window is active but its live document bridge is unavailable")
            snapshot = request_live_snapshot(
                self.paths, session["session_id"], Path(session["source"]).suffix,
            )
            try:
                yield snapshot.path, {"kind": "live_window", "sha256": snapshot.semantic_sha256}
            finally:
                snapshot.path.unlink(missing_ok=True)
            return
        yield Path(session["source"]), {
            "kind": "selected_file",
            "sha256": session["source_identity"]["sha256"],
        }

    def _revalidate_plan_base(self, plan: dict[str, Any], session: dict[str, Any]) -> None:
        base = plan.get("base_source") or {
            "kind": "selected_file", "sha256": plan["source_sha256"],
        }
        with self._agent_source(session) as (_, current):
            if current != base:
                raise ConflictError("workbook state changed after the agent plan was verified")

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
        with self._agent_source(session) as (source, base):
            result = self.engine.describe(source, include_formulas=include_formulas)
        evidence_id = self._record_evidence(
            session, base, "describe_workbook", {"include_formulas": include_formulas}, result,
        )
        result["document_source"] = base["kind"]
        result["evidence_id"] = evidence_id
        result.update(self._public_session(session))
        result["formula_records_included"] = include_formulas
        if not include_formulas:
            result.pop("formulas", None)
        return result

    def read_range(self, session_id: str, **arguments: Any) -> dict[str, Any]:
        session = self._session(session_id)
        with self._agent_source(session) as (source, base):
            result = self.engine.read_range(source, **arguments)
        evidence_id = self._record_evidence(session, base, "read_range", arguments, result)
        return {**result, "document_source": base["kind"], "evidence_id": evidence_id}

    def search_workbook(self, session_id: str, **arguments: Any) -> dict[str, Any]:
        session = self._session(session_id)
        with self._agent_source(session) as (source, base):
            result = self.engine.search(source, **arguments)
        evidence_id = self._record_evidence(session, base, "search_workbook", arguments, result)
        return {**result, "document_source": base["kind"], "evidence_id": evidence_id}

    def trace_formula(self, session_id: str, **arguments: Any) -> dict[str, Any]:
        session = self._session(session_id)
        with self._agent_source(session) as (source, base):
            result = self.engine.trace(source, **arguments)
        evidence_id = self._record_evidence(session, base, "trace_formula", arguments, result)
        return {**result, "document_source": base["kind"], "evidence_id": evidence_id}

    def analyze_workbook(self, session_id: str, **arguments: Any) -> dict[str, Any]:
        session = self._session(session_id)
        with self._agent_source(session) as (source, base):
            result = self.engine.analyze(source, **arguments)
        evidence_id = self._record_evidence(session, base, "analyze_workbook", arguments, result)
        return {**result, "document_source": base["kind"], "evidence_id": evidence_id}

    def render_workbook(self, session_id: str, format: str = "pdf") -> dict[str, Any]:
        del format
        session = self._session(session_id)
        output = self.paths.cache / "previews" / f"{secrets.token_hex(16)}.pdf"
        output.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        with self._agent_source(session) as (source, base):
            result = self.engine.render(source, output=output)
        preview_identity = identify_regular_file(output)
        public_result = {**result, "artifact_sha256": preview_identity.sha256}
        evidence_id = self._record_evidence(session, base, "render_workbook", {"format": "pdf"}, public_result)
        return {
            **result, "artifact": str(output), "artifact_sha256": preview_identity.sha256,
            "document_source": base["kind"], "evidence_id": evidence_id,
        }

    def _record_evidence(
        self,
        session: dict[str, Any],
        base: dict[str, str],
        tool: str,
        arguments: dict[str, Any],
        result: dict[str, Any],
    ) -> str:
        evidence_id = secrets.token_hex(16)
        record = {
            "evidence_id": evidence_id,
            "session_id": session["session_id"],
            "revision": session["revision"],
            "base_source": base,
            "tool": tool,
            "arguments": arguments,
            "result_sha256": _canonical_hash(result),
            "observed_at": _now(),
        }
        if tool == "analyze_workbook":
            # Native review needs cited findings, not a second persisted copy
            # of every profiled cell/column. Keep the evidence projection
            # deliberately small while the digest still seals the full result.
            record["result"] = {
                "summary": result.get("summary", {}),
                "findings": result.get("findings", [])[:100],
                "method": result.get("method"),
            }
        record["seal"] = _canonical_hash(record)
        write_json_atomic(self.evidence / f"{evidence_id}.json", record)
        return evidence_id

    def _load_evidence(self, evidence_id: str) -> dict[str, Any]:
        if not isinstance(evidence_id, str) or _IDENTIFIER.fullmatch(evidence_id) is None:
            raise ConflictError("invalid workflow evidence")
        path = self.evidence / f"{evidence_id}.json"
        if not path.exists():
            raise ConflictError("workflow evidence was not found")
        record = read_json(path)
        sealed = dict(record)
        seal = sealed.pop("seal", None)
        if not isinstance(seal, str) or not secrets.compare_digest(seal, _canonical_hash(sealed)):
            raise ConflictError("workflow evidence seal is invalid")
        return record

    def _resolve_evidence(
        self, session_id: str, revision: int, base: dict[str, str], evidence_ids: list[str],
    ) -> list[dict[str, Any]]:
        result = []
        for evidence_id in evidence_ids:
            record = self._load_evidence(evidence_id)
            if (
                record.get("session_id") != session_id
                or record.get("revision") != revision
                or record.get("base_source") != base
            ):
                raise ConflictError("workflow evidence does not match this workbook revision")
            result.append({key: value for key, value in record.items() if key not in {"session_id", "seal"}})
        return result

    def plan_changes(
        self,
        session_id: str,
        expected_revision: int,
        operations: list[dict[str, Any]],
        workflow: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        return self._create_plan(session_id, expected_revision, operations, workflow)

    def revise_plan(
        self,
        plan_id: str,
        expected_revision: int,
        operations: list[dict[str, Any]],
        workflow: dict[str, Any],
    ) -> dict[str, Any]:
        with plan_lock(self.paths, plan_id):
            previous = self._load_plan(plan_id)
            if previous["status"] != "verified" or previous["revision"] != expected_revision:
                raise ConflictError("only a current verified plan can be revised")
            replacement = self._create_plan(
                previous["session_id"], expected_revision, operations, workflow,
                supersedes_plan_id=plan_id,
            )
            previous["status"] = "superseded"
            previous["superseded_by"] = replacement["plan_id"]
            previous["superseded_at"] = _now()
            self._save_plan(previous)
            return replacement

    def _create_plan(
        self,
        session_id: str,
        expected_revision: int,
        operations: list[dict[str, Any]],
        workflow: dict[str, Any] | None,
        *,
        supersedes_plan_id: str | None = None,
    ) -> dict[str, Any]:
        session = self._session(session_id)
        if session["revision"] != expected_revision:
            raise ConflictError("workbook revision is stale")
        require_stageable(Path(session["source"]), actor=Actor.AGENT)
        normalized = validate_operations(operations)
        normalized_workflow = validate_workflow(workflow, len(normalized)) if workflow is not None else {
            "goal": "Propose workbook changes",
            "summary": "Typed workbook operations staged through the local service.",
            "assumptions": [],
            "evidence_ids": [],
            "groups": [{
                "title": "Workbook changes",
                "purpose": "Apply the requested typed operations.",
                "operation_indexes": list(range(len(normalized))),
            }],
        }
        plan_id = secrets.token_hex(16)
        plan_dir = self.staging / plan_id
        plan_dir.mkdir(mode=0o700, parents=True, exist_ok=False)
        extension = Path(session["source"]).suffix.lower()
        staged = plan_dir / f"staged{extension}"
        preview = plan_dir / "preview.pdf"
        with self._agent_source(session) as (source, base):
            workflow_evidence = self._resolve_evidence(
                session_id, expected_revision, base, normalized_workflow["evidence_ids"],
            ) if normalized_workflow["evidence_ids"] else []
            evidence = self.engine.stage(source, normalized, output=staged, preview=preview)
        staged_identity = identify_regular_file(staged)
        preview_identity = identify_regular_file(preview)
        plan = {
            "plan_id": plan_id,
            "session_id": session_id,
            "revision": expected_revision,
            "status": "verified",
            "created_at": _now(),
            "source_sha256": session["source_identity"]["sha256"],
            "base_source": base,
            "staged_artifact": str(staged),
            "staged_sha256": staged_identity.sha256,
            "preview_artifact": str(preview),
            "preview_sha256": preview_identity.sha256,
            "operations": normalized,
            "workflow": {**normalized_workflow, "evidence": workflow_evidence},
            "supersedes_plan_id": supersedes_plan_id,
            "destructive_operations": destructive_operations(normalized),
            "semantic_diff": evidence.get("semantic_diff", {}),
            "verification": evidence.get("verification", {}),
            "warnings": evidence.get("warnings", []),
            "engine": evidence.get("engine", {}),
        }
        plan["seal"] = _canonical_hash(plan)
        self._save_plan(plan)
        self._publish_plan_overlay(plan)
        return self._public_plan(plan)

    def _save_plan(self, plan: dict[str, Any]) -> None:
        sealed = dict(plan)
        sealed.pop("seal", None)
        plan["seal"] = _canonical_hash(sealed)
        write_json_atomic(self.plans / f"{plan['plan_id']}.json", plan)

    def _publish_plan_overlay(self, plan: dict[str, Any]) -> None:
        window = self.window_context_resource()
        if window.get("active") and window.get("session_id") == plan["session_id"]:
            publish_overlay(overlay_path(self.paths.runtime), plan)

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
        self._revalidate_plan_base(plan, session)
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
            self._revalidate_plan_base(plan, session)
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
                if (plan.get("base_source") or {}).get("kind") == "live_window":
                    raise ConflictError("live-window agent plans can publish only to a new copy")
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
            self._publish_plan_overlay(plan)
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
            self._revalidate_plan_base(plan, session)
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
            overlay_path(self.paths.runtime).unlink(missing_ok=True)
            return receipt

    def commit_native_overlay(self, plan_id: str, expected_revision: int, destination: Path) -> dict[str, Any]:
        """Publish only when the active native window presents this exact plan."""

        window = self.window_context_resource()
        path = overlay_path(self.paths.runtime)
        try:
            details = path.stat()
            if details.st_uid != os.getuid() or details.st_mode & 0o077 or details.st_size > 256 * 1024:
                raise ValueError("unsafe overlay file")
            overlay = decode_overlay(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, ValueError) as exc:
            raise ConflictError("native review overlay is unavailable") from exc
        if (
            not window.get("active")
            or window.get("session_id") != overlay["session_id"]
            or overlay["plan_id"] != plan_id
            or overlay["revision"] != expected_revision
        ):
            raise ConflictError("native review overlay is stale")
        review = self.prepare_local_review(
            plan_id, expected_revision, mode="copy", destination=destination,
        )
        return self.commit_local_review(plan_id, expected_revision, review["approval_token"])

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
