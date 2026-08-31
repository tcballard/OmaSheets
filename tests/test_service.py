import json
from pathlib import Path
import tempfile
import unittest

from omasheets.errors import ConflictError
from omasheets.paths import AppPaths
from omasheets.service import OmaSheetsService


class FakeEngine:
    def describe(self, source, *, include_formulas):
        return {"sheets": [{"name": "Sheet1"}], "formulas": ["=1+1"]}

    def read_range(self, source, **arguments):
        return {"values": [[1]], **arguments}

    def search(self, source, **arguments):
        return {"matches": [], **arguments}

    def trace(self, source, **arguments):
        return {"nodes": [], **arguments}

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
        self.service = OmaSheetsService(self.paths, FakeEngine())

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

    def test_capabilities_do_not_misrepresent_libreoffice_as_a_fork(self):
        capabilities = self.service.capabilities_resource()
        self.assertFalse(capabilities["libreoffice_fork"])
        self.assertEqual(capabilities["document_engine"]["adapter"], "isolated_uno_worker")
        self.assertIn("set_range_values", capabilities["agent_operations"])
        self.assertIn("format_cells", capabilities["agent_operations"])
        self.assertEqual(capabilities["live_window_context"]["resource"], "omasheets://window")
        self.assertFalse(capabilities["live_window_context"]["agent_control"])

    def test_live_window_context_is_path_free_and_session_bound(self):
        session = self.service.select_workbook(self.source)
        path = self.service.prepare_window_context(session["session_id"])
        context = self.service.window_context_resource()
        self.assertFalse(context["active"])
        self.assertEqual(context["session_id"], session["session_id"])
        self.assertNotIn(str(self.source), str(context))
        payload = json.loads(path.read_text())
        payload.update({"active": True, "address": "C9", "formula": "=SUM(A1:A8)", "updated_at_ms": 4})
        path.write_text(json.dumps(payload))
        context = self.service.window_context_resource()
        self.assertTrue(context["active"])
        self.assertEqual(context["address"], "C9")
        self.assertFalse(context["agent_control"])

    def test_agent_staging_refuses_dirty_native_window_state(self):
        session = self.service.select_workbook(self.source)
        path = self.service.prepare_window_context(session["session_id"])
        payload = json.loads(path.read_text())
        payload.update({"active": True, "dirty": True, "updated_at_ms": 4})
        path.write_text(json.dumps(payload))
        with self.assertRaisesRegex(ConflictError, "unsaved changes"):
            self.service.plan_changes(
                session["session_id"], 1,
                [{"type": "set_value", "sheet": "Sheet1", "range": "A1", "value": 2}],
            )

    def test_legacy_conversion_creates_a_receipt_and_preserves_source(self):
        legacy = self.source.with_name("legacy.xls")
        legacy.write_bytes(b"legacy-source")
        receipt = self.service.convert_legacy_local(legacy)
        self.assertEqual(legacy.read_bytes(), b"legacy-source")
        self.assertEqual(legacy.with_suffix(".xlsx").read_bytes(), b"xlsx-converted")
        self.assertEqual(receipt["kind"], "conversion")
        self.assertTrue(receipt["manual_review_required"])
        self.assertFalse(receipt["excel_equivalence_claimed"])

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
