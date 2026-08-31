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


if __name__ == "__main__":
    unittest.main()
