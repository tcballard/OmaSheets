from pathlib import Path
import tempfile
import unittest

from omasheets.diff_overlay import MAX_ITEMS, build_overlay, decode_overlay, encode_overlay, publish_overlay


class DiffOverlayTests(unittest.TestCase):
    def _plan(self):
        return {
            "plan_id": "b" * 32,
            "session_id": "a" * 32,
            "revision": 1,
            "status": "verified",
            "operations": [
                {"type": "set_range_values", "sheet": "Sheet1", "range": "B2:C3", "values": [[2, 3], [4, 5]]},
                {"type": "format_cells", "sheet": "Sheet1", "range": "B2:C3", "bold": True},
            ],
            "destructive_operations": [],
            "warnings": [],
            "workflow": {
                "goal": "Prepare the monthly variance view",
                "summary": "Correct two values and emphasise the reviewed range.",
                "assumptions": ["The source amounts use the same currency."],
                "groups": [
                    {"title": "Correct values", "purpose": "Use the inspected source figures.", "operation_indexes": [0]},
                    {"title": "Emphasise review", "purpose": "Make the checked cells visible.", "operation_indexes": [1]},
                ],
                "evidence": [{
                    "tool": "analyze_workbook",
                    "result": {"findings": [{"severity": "warning", "category": "duplicate_rows", "sheet": "Sheet1", "range": "A2:C4", "message": "Duplicate rows may distort totals."}]},
                }],
            },
            "semantic_diff": {
                "target_changes": [{
                    "sheet": "Sheet1", "range": "B2:C3",
                    "before": {
                        "values": [[1, 3], [4, ""]], "formulas": [["", ""], ["", ""]],
                        "format": {"bold": False},
                    },
                    "after": {
                        "values": [[2, 3], [4, 5]], "formulas": [["", ""], ["", ""]],
                        "format": {"bold": True},
                    },
                }],
            },
        }

    def test_builds_verified_cell_and_format_changes(self):
        overlay = build_overlay(self._plan())
        self.assertEqual(overlay["total_changes"], 3)
        self.assertEqual(overlay["items"][0], {
            "kind": "set_range_values", "group": "Correct values", "sheet": "Sheet1", "range": "B2", "before": "1", "after": "2",
        })
        self.assertEqual(overlay["items"][1]["range"], "C3")
        self.assertIn("bold", overlay["items"][2]["after"])
        self.assertEqual(overlay["goal"], "Prepare the monthly variance view")
        self.assertEqual(overlay["groups"][0]["title"], "Correct values")
        self.assertEqual(overlay["findings"][0]["category"], "duplicate_rows")

    def test_round_trip_is_path_free_and_percent_encoded(self):
        overlay = build_overlay(self._plan())
        payload = encode_overlay(overlay)
        self.assertNotIn("/tmp/", payload)
        self.assertIn("B2%3AC3", payload)
        self.assertEqual(decode_overlay(payload), overlay)

    def test_publish_is_atomic_and_private(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "window-diff.overlay"
            expected = publish_overlay(path, self._plan())
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(decode_overlay(path.read_text()), expected)

    def test_large_diff_is_visibly_truncated(self):
        plan = self._plan()
        size = MAX_ITEMS + 3
        plan["operations"] = [{
            "type": "set_range_values", "sheet": "Sheet1", "range": f"A1:A{size}",
            "values": [[index] for index in range(size)],
        }]
        plan["semantic_diff"]["target_changes"] = [{
            "sheet": "Sheet1", "range": f"A1:A{size}",
            "before": {"values": [[""] for _ in range(size)], "formulas": [[""] for _ in range(size)]},
            "after": {"values": [[index] for index in range(size)], "formulas": [[""] for _ in range(size)]},
        }]
        overlay = build_overlay(plan)
        self.assertEqual(len(overlay["items"]), MAX_ITEMS)
        self.assertTrue(overlay["truncated"])
        self.assertEqual(overlay["total_changes"], size)


if __name__ == "__main__":
    unittest.main()
