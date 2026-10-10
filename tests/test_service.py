import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from omasheets.errors import ConflictError, EngineError
from omasheets.diff_overlay import decode_overlay, overlay_path
from omasheets.paths import AppPaths
from omasheets.service import OmaSheetsService


class FakeEngine:
    def __init__(self):
        self.query_calls = []

    def describe(self, source, *, include_formulas):
        return {"sheets": [{"name": "Sheet1"}], "formulas": ["=1+1"]}

    def read_range(self, source, **arguments):
        return {"values": [[1]], **arguments}

    def search(self, source, **arguments):
        return {"matches": [], **arguments}

    def trace(self, source, **arguments):
        return {"nodes": [], **arguments}

    def query(self, source, queries):
        self.query_calls.append((source, queries))
        return {"items": [
            {"id": query["id"], "tool": query["tool"], "result": {"ok": query["id"]}}
            for query in queries
        ]}

    def analyze(self, source, **arguments):
        return {
            "summary": {"sheet_count": 1, "finding_count": 1},
            "findings": [{"id": "F001", "severity": "warning", "category": "duplicate_rows", "sheet": "Sheet1", "range": "A1:B3", "message": "Duplicate rows found."}],
            **arguments,
        }

    def render(self, source, *, output):
        output.write_bytes(b"%PDF-preview")
        return {"format": "pdf"}

    def stage(self, source, operations, *, output, preview):
        output.write_bytes(source.read_bytes() + b"-staged")
        preview.write_bytes(b"%PDF-staged-preview")
        return {
            "semantic_diff": {"operation_count": len(operations)},
            "verification": {"reopened": True, "formula_errors": []},
            "warnings": [],
            "engine": {"name": "fake"},
        }

    def convert_legacy(self, source, *, destination=None, preview):
        destination.write_bytes(b"xlsx-converted")
        preview.write_bytes(b"%PDF-conversion-preview")
        return {
            "comparison": {"sheet_count_before": 1, "sheet_count_after": 1},
            "warnings": ["manual review required"],
            "engine": {"name": "fake"},
        }


class ServiceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.paths = AppPaths(root / "state", root / "cache", root / "runtime")
        self.source = root / "book.xlsx"
        self.source.write_bytes(b"workbook")
        self.engine = FakeEngine()
        self.service = OmaSheetsService(self.paths, self.engine)

    def tearDown(self):
        self.temporary.cleanup()

    def test_selection_does_not_expose_source_path(self):
        session = self.service.select_workbook(self.source)
        self.assertNotIn("source", session)
        self.assertNotIn(str(self.source), str(session))

    def test_local_status_is_bounded_and_path_free(self):
        session = self.service.select_workbook(self.source)
        plan = self.service.plan_changes(
            session["session_id"], 1,
            [{"type": "set_value", "sheet": "Sheet1", "range": "A1", "value": 2}],
        )
        status = self.service.local_status()
        self.assertEqual(status["review"]["plan_id"], plan["plan_id"])
        self.assertEqual(status["review"]["operation_count"], 1)
        self.assertFalse(status["agent_commit_authority"])
        self.assertNotIn(str(self.source.parent), str(status))

    def test_workbook_audit_is_sealed_as_reviewable_evidence(self):
        session = self.service.select_workbook(self.source)
        audit = self.service.analyze_workbook(session["session_id"], focus="all", max_findings=10)
        self.assertEqual(audit["summary"]["finding_count"], 1)
        plan = self.service.plan_changes(
            session["session_id"], 1,
            [{"type": "set_value", "sheet": "Sheet1", "range": "C1", "value": "Reviewed"}],
            {"goal": "Resolve the audit", "summary": "Mark the reviewed result.", "assumptions": [],
             "evidence_ids": [audit["evidence_id"]],
             "groups": [{"title": "Record review", "purpose": "Make the audit follow-up visible.", "operation_indexes": [0]}]},
        )
        self.assertEqual(plan["workflow"]["evidence"][0]["result"]["findings"][0]["id"], "F001")

    def test_capabilities_identify_the_owned_engine_and_supported_formats(self):
        capabilities = self.service.capabilities_resource()
        self.assertEqual(capabilities["document_engine"]["name"], "OmaSheets")
        self.assertEqual(capabilities["document_engine"]["adapter"], "isolated_rust_kit")
        self.assertEqual(capabilities["document_engine"]["interactive_adapter"], "owned_document_service_qt_grid")
        self.assertEqual(capabilities["formats"], ["omasheets", "xlsx"])
        self.assertEqual(capabilities["unsupported_formats"], ["xls", "xlsm", "ods"])
        self.assertNotIn("upsert_pivot", capabilities["agent_operations"])
        self.assertFalse(capabilities["agent_publish_authority"])

    def test_window_context_reads_the_owned_path_free_selection(self):
        selection = {"sheet": "sheet-id", "row": 2, "column": 3, "rows": 1, "columns": 1}
        private = {"session_id": "a" * 32, "path": "/private/workbook.omasheets", "selection": selection}
        with patch("omasheets.native_agent.context", return_value=private):
            context = self.service.window_context_resource()
        self.assertTrue(context["active"])
        self.assertEqual(context["selection"], selection)
        self.assertEqual(context["source"], "owned_native_document_service")
        self.assertNotIn("/private", str(context))
        self.assertFalse(context["agent_control"])

    def test_selected_file_query_uses_one_engine_call_and_one_evidence_record(self):
        session = self.service.select_workbook(self.source)
        queries = [
            {"id": "structure", "tool": "describe_workbook", "arguments": {"include_formulas": False}},
            {"id": "cells", "tool": "read_range", "arguments": {"sheet": "Sheet1", "range": "A1:B2", "include_formulas": True, "include_styles": False}},
        ]
        result = self.service.query_workbook(session["session_id"], queries)
        self.assertEqual(self.engine.query_calls, [(self.source, queries)])
        self.assertEqual([item["id"] for item in result["items"]], ["structure", "cells"])
        self.assertEqual(result["document_source"], "selected_file")
        self.assertEqual(len(list(self.service.evidence.glob("*.json"))), 1)
        record = self.service._load_evidence(result["evidence_id"])
        self.assertEqual(record["tool"], "query_workbook")
        self.assertEqual(record["arguments"], {"queries": queries})

    def test_failed_read_query_batch_creates_no_evidence_or_plan(self):
        session = self.service.select_workbook(self.source)
        queries = [{
            "id": "cells", "tool": "read_range",
            "arguments": {
                "sheet": "Sheet1", "range": "A1",
                "include_formulas": True, "include_styles": False,
            },
        }]
        with patch.object(self.engine, "query", side_effect=EngineError("subquery failed")):
            with self.assertRaisesRegex(EngineError, "subquery failed"):
                self.service.query_workbook(session["session_id"], queries)
        self.assertEqual(list(self.service.evidence.glob("*.json")), [])
        self.assertEqual(list(self.service.plans.glob("*.json")), [])

    def test_mismatched_read_query_worker_output_is_not_sealed(self):
        session = self.service.select_workbook(self.source)
        queries = [
            {"id": "first", "tool": "describe_workbook", "arguments": {"include_formulas": False}},
            {"id": "second", "tool": "describe_workbook", "arguments": {"include_formulas": False}},
        ]
        reversed_items = {"items": [
            {"id": "second", "tool": "describe_workbook", "result": {}},
            {"id": "first", "tool": "describe_workbook", "result": {}},
        ]}
        with patch.object(self.engine, "query", return_value=reversed_items):
            with self.assertRaisesRegex(EngineError, "mismatched query batch"):
                self.service.query_workbook(session["session_id"], queries)
        self.assertEqual(list(self.service.evidence.glob("*.json")), [])

    def test_plan_is_sealed_and_handoff_is_non_mutating(self):
        session = self.service.select_workbook(self.source)
        plan = self.service.plan_changes(
            session["session_id"],
            session["revision"],
            [{"type": "set_value", "sheet": "Sheet1", "range": "A1", "value": 2}],
        )
        handoff = self.service.apply_plan_handoff(plan["plan_id"], plan["revision"])
        self.assertEqual(handoff["status"], "local_review_required")
        self.assertEqual(self.service.get_plan(plan["plan_id"])["status"], "verified")

    def test_read_evidence_is_sealed_into_an_explainable_plan(self):
        session = self.service.select_workbook(self.source)
        observation = self.service.describe_workbook(session["session_id"])
        operations = [{"type": "set_value", "sheet": "Sheet1", "range": "A1", "value": 2}]
        plan = self.service.plan_changes(session["session_id"], 1, operations, {
            "goal": "Correct the selected total",
            "summary": "Replace the stale value after inspecting workbook structure.",
            "assumptions": ["The requested value is authoritative."],
            "evidence_ids": [observation["evidence_id"]],
            "groups": [{
                "title": "Correct total", "purpose": "Apply the requested scalar correction.",
                "operation_indexes": [0],
            }],
        })
        self.assertEqual(plan["workflow"]["goal"], "Correct the selected total")
        self.assertEqual(plan["workflow"]["evidence"][0]["tool"], "describe_workbook")
        self.assertNotIn(str(self.source), str(plan["workflow"]))

    def test_revising_a_plan_supersedes_but_does_not_mutate_it(self):
        session = self.service.select_workbook(self.source)
        observation = self.service.describe_workbook(session["session_id"])

        def context(goal):
            return {
                "goal": goal,
                "summary": "Use the inspected workbook state.",
                "evidence_ids": [observation["evidence_id"]],
                "groups": [{
                    "title": "Update value", "purpose": "Implement the revised instruction.",
                    "operation_indexes": [0],
                }],
            }

        first = self.service.plan_changes(
            session["session_id"], 1,
            [{"type": "set_value", "sheet": "Sheet1", "range": "A1", "value": 2}],
            context("Set the total to two"),
        )
        second = self.service.revise_plan(
            first["plan_id"], 1,
            [{"type": "set_value", "sheet": "Sheet1", "range": "A1", "value": 3}],
            context("Use three instead"),
        )
        superseded = self.service.get_plan(first["plan_id"])
        self.assertEqual(superseded["status"], "superseded")
        self.assertEqual(superseded["superseded_by"], second["plan_id"])
        self.assertEqual(second["supersedes_plan_id"], first["plan_id"])
        with self.assertRaises(ConflictError):
            self.service.apply_plan_handoff(first["plan_id"], 1)

    def test_changed_source_invalidates_session(self):
        session = self.service.select_workbook(self.source)
        self.source.write_bytes(b"changed")
        with self.assertRaises(ConflictError):
            self.service.describe_workbook(session["session_id"])

    def test_tampered_plan_is_rejected(self):
        session = self.service.select_workbook(self.source)
        plan = self.service.plan_changes(
            session["session_id"], 1,
            [{"type": "clear_range", "sheet": "Sheet1", "range": "A1"}],
        )
        path = self.service.plans / f"{plan['plan_id']}.json"
        text = path.read_text()
        path.write_text(text.replace('"status":"verified"', '"status":"approved"'))
        with self.assertRaises(ConflictError):
            self.service.get_plan(plan["plan_id"])

    def test_identifiers_cannot_traverse_state_directories(self):
        malicious = "../" * 10 + "xx"
        self.assertEqual(len(malicious), 32)
        with self.assertRaises(ConflictError):
            self.service.get_plan(malicious)
        with self.assertRaises(ConflictError):
            self.service._session(malicious)
        with self.assertRaises(ConflictError):
            self.service.undo_receipt(malicious, f"UNDO {malicious}")

    def test_preview_and_staged_hashes_are_rechecked(self):
        session = self.service.select_workbook(self.source)
        plan = self.service.plan_changes(
            session["session_id"], 1,
            [{"type": "set_value", "sheet": "Sheet1", "range": "A1", "value": 2}],
        )
        private = self.service._load_plan(plan["plan_id"])
        Path(private["preview_artifact"]).write_bytes(b"tampered")
        with self.assertRaises(ConflictError):
            self.service.apply_plan_handoff(plan["plan_id"], 1)

    def _plan(self):
        session = self.service.select_workbook(self.source)
        plan = self.service.plan_changes(
            session["session_id"], 1,
            [{"type": "set_value", "sheet": "Sheet1", "range": "A1", "value": 2}],
        )
        return session, plan

    def test_copy_publication_never_clobbers(self):
        _, plan = self._plan()
        destination = self.source.with_name("result.xlsx")
        destination.write_bytes(b"someone-else")
        self.service.prepare_local_review(plan["plan_id"], 1, destination=destination)
        with self.assertRaises(ConflictError):
            self.service.commit_local_review(plan["plan_id"], 1, f"APPLY {plan['plan_id']}")
        self.assertEqual(destination.read_bytes(), b"someone-else")

    def test_wrong_approval_token_writes_nothing(self):
        _, plan = self._plan()
        review = self.service.prepare_local_review(plan["plan_id"], 1)
        with self.assertRaises(ConflictError):
            self.service.commit_local_review(plan["plan_id"], 1, "APPLY something-else")
        self.assertFalse(Path(review["destination"]).exists())

    def test_replace_creates_receipt_and_undo_restores_source(self):
        _, plan = self._plan()
        original = self.source.read_bytes()
        self.service.prepare_local_review(plan["plan_id"], 1, mode="replace")
        receipt = self.service.commit_local_review(plan["plan_id"], 1, f"APPLY {plan['plan_id']}")
        self.assertNotEqual(self.source.read_bytes(), original)
        self.assertEqual(receipt["target_mode"], "replace")
        undo = self.service.undo_receipt(receipt["receipt_id"], f"UNDO {receipt['receipt_id']}")
        self.assertEqual(undo["kind"], "undo")
        self.assertEqual(self.source.read_bytes(), original)

    def test_receipts_form_a_hash_chain(self):
        _, first_plan = self._plan()
        first_review = self.service.prepare_local_review(first_plan["plan_id"], 1)
        first = self.service.commit_local_review(first_plan["plan_id"], 1, f"APPLY {first_plan['plan_id']}")
        self.assertIsNone(first["previous_receipt_hash"])
        Path(first_review["destination"]).unlink()
        # Reselect because the first plan intentionally leaves the source unchanged.
        _, second_plan = self._plan()
        second_review = self.service.prepare_local_review(second_plan["plan_id"], 1)
        second = self.service.commit_local_review(second_plan["plan_id"], 1, f"APPLY {second_plan['plan_id']}")
        self.assertEqual(second["previous_receipt_hash"], first["receipt_hash"])
        Path(second_review["destination"]).unlink()

    def test_retry_finishes_receipt_after_post_publish_failure(self):
        _, plan = self._plan()
        review = self.service.prepare_local_review(plan["plan_id"], 1)
        original_record = self.service.publisher.receipts.record
        attempts = 0

        def fail_once(receipt):
            nonlocal attempts
            attempts += 1
            if attempts == 1:
                raise OSError("simulated receipt storage interruption")
            return original_record(receipt)

        self.service.publisher.receipts.record = fail_once
        token = f"APPLY {plan['plan_id']}"
        with self.assertRaises(OSError):
            self.service.commit_local_review(plan["plan_id"], 1, token)
        self.assertTrue(Path(review["destination"]).exists())
        self.assertEqual(self.service.get_plan(plan["plan_id"])["status"], "approved")
        receipt = self.service.commit_local_review(plan["plan_id"], 1, token)
        self.assertEqual(receipt["kind"], "publish")
        self.assertEqual(self.service.get_plan(plan["plan_id"])["status"], "committed")


if __name__ == "__main__":
    unittest.main()
