import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from scripts import update_corpus_summary


class UpdateCorpusSummaryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.manifest = root / "sample.jsonl"
        self.manifest.write_text('{"id":"private","path":"secret.xlsx","sha256":"00"}\n')
        self.score = root / "score.json"
        owned = {
            "files": 2, "opened": 1, "failed": 1, "timed_out": 0,
            "formula_cells_observed": 10, "formula_cells_loaded": 8,
            "formula_cells_compared": 8, "stored_values_matched": 7,
            "stored_values_mismatched": 1, "unsupported_formulas": 2,
            "formula_parse_rate": 0.8, "comparison_coverage": 0.8,
            "stored_value_match_rate": 0.875,
            "unsupported_functions": {
                "SMALL": {"formula_cells": 1, "workbooks": 1},
                "BIG": {"formula_cells": 2, "workbooks": 1},
            },
            "unsupported_reasons": {"syntax": 2},
            "workbooks_with_skipped_sheets": 0, "skipped_sheets": 0,
            "peak_rss_bytes_max": 123,
        }
        self.score.write_text(json.dumps({
            "schema": 2, "engine": "candidate", "owned_engine": "owned",
            "summary": {"files": 2, "failed": 1}, "owned_summary": owned,
            "entries": [
                {"id": "private", "status": "failed", "error": "memory allocation failed",
                 "owned_status": "failed", "owned_error": "1904 date system rejected"},
                {"id": "also-private", "status": "ok", "owned_status": "ok"},
            ],
        }))
        self.performance = root / "performance.json"
        self.performance.write_text(json.dumps({
            "schema": "OMASHEETS_PERFORMANCE_V1", "wall_seconds": 12.5,
            "exit_code": 0, "timed_out": False,
            "memory": {"peak_rss_bytes": 456},
        }))
        self.baseline = root / "baseline.json"
        baseline_owned = dict(owned)
        baseline_owned.pop("unsupported_functions")
        baseline_owned["formula_cells_loaded"] = 7
        self.baseline.write_text(json.dumps({
            "source": "sample", "manifest": "sample.jsonl",
            "manifest_sha256": hashlib.sha256(self.manifest.read_bytes()).hexdigest(),
            "engine_commit": "1" * 40, "wall_seconds": 20,
            "process_tree_peak_rss_bytes": 999, "owned_summary": baseline_owned,
        }))
        self.summary = root / "summary.json"
        self.delta = root / "delta.json"

    def tearDown(self):
        self.temporary.cleanup()

    def arguments(self):
        return [
            "--score", str(self.score), "--performance", str(self.performance),
            "--manifest", str(self.manifest), "--baseline-summary", str(self.baseline),
            "--summary", str(self.summary), "--delta", str(self.delta),
            "--runner", "test runner", "--scored", "2026-09-04",
            "--engine-commit", "2" * 40, "--resolved-function", "FROB",
            "--resolved-open-failure", "repaired=3", "--note", "synthetic run",
        ]

    def test_writes_aggregate_summary_and_delta(self):
        self.assertEqual(update_corpus_summary.main(self.arguments()), 0)
        summary = json.loads(self.summary.read_text())
        delta = json.loads(self.delta.read_text())
        self.assertEqual(list(summary["owned_unsupported_functions"]), ["BIG", "SMALL"])
        self.assertEqual(summary["candidate_failure_kinds"], {"memory_allocation_failed": 1})
        self.assertEqual(summary["owned_failure_kinds"], {"date_system_1904_rejected": 1})
        self.assertNotIn("unsupported_functions", summary["owned_summary"])
        self.assertEqual(delta["owned_lane"]["formula_cells_loaded"], {"before": 7, "after": 8})
        self.assertEqual(delta["resolved_open_failures"], {"repaired": 3})
        emitted = self.summary.read_text() + self.delta.read_text()
        self.assertNotIn("secret.xlsx", emitted)
        self.assertNotIn('"entries"', emitted)
        self.assertNotIn('"id"', emitted)

    def test_rejects_failed_performance_evidence_before_writing(self):
        payload = json.loads(self.performance.read_text())
        payload["exit_code"] = 1
        self.performance.write_text(json.dumps(payload))
        with self.assertRaisesRegex(ValueError, "successful completed"):
            update_corpus_summary.main(self.arguments())
        self.assertFalse(self.summary.exists())
        self.assertFalse(self.delta.exists())

    def test_rejects_a_different_baseline_manifest(self):
        payload = json.loads(self.baseline.read_text())
        payload["manifest_sha256"] = "f" * 64
        self.baseline.write_text(json.dumps(payload))
        with self.assertRaisesRegex(ValueError, "baseline manifest_sha256"):
            update_corpus_summary.main(self.arguments())

    def test_rejects_invalid_evidence_metadata(self):
        arguments = self.arguments()
        arguments[arguments.index("2026-09-04")] = "September 4"
        with self.assertRaisesRegex(ValueError, "ISO 8601"):
            update_corpus_summary.main(arguments)


if __name__ == "__main__":
    unittest.main()
