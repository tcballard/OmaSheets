from pathlib import Path
import json
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ElementTree
from unittest.mock import patch

from omasheets.performance import (
    FixtureSpec,
    MemorySample,
    ProcProcessGroupSampler,
    _SampleAccumulator,
    bounded_json,
    fixture_manifest,
    generate_fixture,
    generate_fixture_suite,
    measure_command,
    write_bounded_json,
)


class _FixedSampler:
    def __init__(self):
        self.calls = 0

    def sample(self, process_group, at_seconds):
        del process_group
        self.calls += 1
        return MemorySample(at_seconds, 2, 2, 30_000, 20_000, 10_000, "smaps_rollup")


class PerformanceTests(unittest.TestCase):
    def test_proc_sampler_sums_complete_process_group_memory(self):
        with tempfile.TemporaryDirectory() as temporary:
            proc = Path(temporary)
            for pid, rss, pss, clean, dirty in ((101, 100, 60, 10, 20), (102, 80, 50, 5, 15)):
                process = proc / str(pid)
                process.mkdir()
                (process / "stat").write_text(f"{pid} (worker name) S 1 77 77 0 0\n")
                (process / "smaps_rollup").write_text(
                    f"Rss: {rss} kB\nPss: {pss} kB\nPrivate_Clean: {clean} kB\n"
                    f"Private_Dirty: {dirty} kB\nPrivate_Hugetlb: 0 kB\n"
                )
            other = proc / "103"
            other.mkdir()
            (other / "stat").write_text("103 (other) S 1 88 88 0 0\n")
            (other / "smaps_rollup").write_text("Rss: 999 kB\nPss: 999 kB\n")

            sample = ProcProcessGroupSampler(proc).sample(77, 0.25)

        self.assertEqual(sample.process_count, 2)
        self.assertEqual(sample.rss_bytes, 180 * 1024)
        self.assertEqual(sample.pss_bytes, 110 * 1024)
        self.assertEqual(sample.uss_bytes, 50 * 1024)
        self.assertEqual(sample.source, "smaps_rollup")

    def test_missing_smaps_reports_rss_without_inventing_pss_or_uss(self):
        with tempfile.TemporaryDirectory() as temporary:
            proc = Path(temporary)
            process = proc / "201"
            process.mkdir()
            (process / "stat").write_text("201 (worker) S 1 91 91 0 0\n")
            (process / "status").write_text("Name:\tworker\nVmRSS:\t123 kB\n")
            sample = ProcProcessGroupSampler(proc).sample(91, 0)
        self.assertEqual(sample.rss_bytes, 123 * 1024)
        self.assertIsNone(sample.pss_bytes)
        self.assertIsNone(sample.uss_bytes)
        self.assertEqual(sample.source, "status")

    def test_proc_sampler_keeps_descendant_that_starts_another_session(self):
        with tempfile.TemporaryDirectory() as temporary:
            proc = Path(temporary)
            for pid, parent, group, rss in (
                (301, 1, 77, 100),
                (302, 301, 302, 80),
            ):
                process = proc / str(pid)
                process.mkdir()
                (process / "stat").write_text(
                    f"{pid} (worker name) S {parent} {group} {group} 0 0\n"
                )
                (process / "smaps_rollup").write_text(
                    f"Rss: {rss} kB\nPss: {rss} kB\nPrivate_Clean: 0 kB\n"
                    f"Private_Dirty: {rss} kB\nPrivate_Hugetlb: 0 kB\n"
                )

            sample = ProcProcessGroupSampler(proc).sample(77, 0)

        self.assertEqual(sample.process_count, 2)
        self.assertEqual(sample.rss_bytes, 180 * 1024)
        self.assertEqual(sample.pss_bytes, 180 * 1024)
        self.assertEqual(sample.uss_bytes, 180 * 1024)

    def test_sample_retention_is_bounded_but_peaks_cover_all_observations(self):
        samples = _SampleAccumulator(2)
        for index, rss in enumerate((1, 9, 3)):
            samples.add(MemorySample(index, 1, 1, rss, rss, rss, "smaps_rollup"))
        self.assertEqual(len(samples.samples), 2)
        self.assertEqual(samples.samples[-1].rss_bytes, 3)
        self.assertEqual(samples.report()["peak_rss_bytes"], 9)
        self.assertEqual(samples.observed, 3)

    def test_command_measurement_records_wall_and_kernel_samples_but_hides_argv(self):
        result = measure_command(
            "unit-command",
            [sys.executable, "-c", "pass"],
            interval_seconds=0.01,
            max_samples=2,
            sampler=_FixedSampler(),
        )
        self.assertEqual(result["schema"], "OMASHEETS_PERFORMANCE_V1")
        self.assertEqual(result["exit_code"], 0)
        self.assertGreaterEqual(result["wall_seconds"], 0)
        self.assertEqual(result["memory"]["peak_pss_bytes"], 20_000)
        self.assertFalse(result["command"]["argv_recorded"])
        self.assertNotIn("argv", result["command"])
        self.assertLessEqual(result["sampling"]["retained_samples"], 2)

    def test_command_timeout_terminates_only_its_isolated_group(self):
        result = measure_command(
            "unit-timeout",
            [sys.executable, "-c", "import time; time.sleep(5)"],
            interval_seconds=0.01,
            timeout_seconds=0.05,
            max_samples=8,
            sampler=_FixedSampler(),
        )
        self.assertTrue(result["timed_out"])
        self.assertNotEqual(result["exit_code"], 0)
        self.assertLess(result["wall_seconds"], 2)

    def test_standard_fixture_manifest_states_actual_population(self):
        manifest = fixture_manifest("standard")
        dense, sparse, formula = manifest["fixtures"]
        self.assertEqual(dense["value_cells"], 5_000_000)
        self.assertEqual(dense["data_density"], 1.0)
        self.assertEqual(sparse["value_cells"], 60_006)
        self.assertEqual(sparse["logical_data_cells"], 50_000_000)
        self.assertEqual(formula["formula_cells"], 800_000)
        self.assertEqual(formula["value_cells"], 200_000)

    def test_ci_fixture_profile_fits_the_agent_analysis_limits(self):
        manifest = fixture_manifest("ci")
        dense, sparse, formula = manifest["fixtures"]
        self.assertEqual([item["name"] for item in manifest["fixtures"]], [
            "dense-ci", "sparse-ci", "formula-ci",
        ])
        self.assertTrue(all(item["used_range_cells"] <= 250_000 for item in manifest["fixtures"]))
        self.assertEqual(dense["used_range_cells"], 240_020)
        self.assertEqual(dense["data_density"], 1.0)
        self.assertEqual(sparse["used_range_cells"], 240_020)
        self.assertLess(sparse["data_density"], 0.01)
        self.assertEqual(formula["formula_cells"], 16_000)
        self.assertLessEqual(formula["formula_cells"], 20_000)

    def test_performance_cli_accepts_ci_fixture_profile(self):
        command = [
            sys.executable,
            str(Path(__file__).resolve().parents[1] / "scripts" / "performance.py"),
            "specs", "--profile", "ci",
        ]
        completed = subprocess.run(command, check=True, capture_output=True, text=True)
        self.assertEqual(json.loads(completed.stdout)["profile"], "ci")

    def test_fods_generation_is_deterministic_and_formula_counts_are_exact(self):
        spec = FixtureSpec("formula-test", "formula", 3, 5)
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            one = Path(first) / "formula-test.fods"
            two = Path(second) / "formula-test.fods"
            first_result = generate_fixture(one, spec)
            second_result = generate_fixture(two, spec)
            self.assertEqual(one.read_bytes(), two.read_bytes())
            self.assertEqual(first_result["sha256"], second_result["sha256"])
            self.assertEqual(first_result["formula_cells"], 9)
            self.assertEqual(first_result["value_cells"], 6)
            self.assertEqual(one.read_text().count('table:formula="'), 9)
            self.assertIn('table:formula="of:=[.A2]+[.B2]"', one.read_text())
            ElementTree.parse(one)

    def test_suite_preflights_every_output_before_writing(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            occupied = directory / "sparse-smoke.fods"
            occupied.write_text("keep")
            with self.assertRaises(FileExistsError):
                generate_fixture_suite(directory, "smoke")
            self.assertEqual(occupied.read_text(), "keep")
            self.assertFalse((directory / "dense-smoke.fods").exists())

    def test_failed_fixture_generation_exposes_no_partial_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "dense-test.fods"
            with patch("omasheets.performance._write_dense", side_effect=OSError("disk full")):
                with self.assertRaisesRegex(OSError, "disk full"):
                    generate_fixture(destination, FixtureSpec("dense-test", "dense", 2, 2))
            self.assertFalse(destination.exists())

    def test_bounded_json_rejects_oversized_evidence(self):
        with self.assertRaises(ValueError):
            bounded_json({"value": "x" * 100}, maximum_bytes=50)
        self.assertEqual(json.loads(bounded_json({"ok": True})), {"ok": True})

    def test_oversized_json_does_not_leave_an_empty_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "result.json"
            with self.assertRaises(ValueError):
                write_bounded_json(destination, {"value": "x" * 1_048_576})
            self.assertFalse(destination.exists())


if __name__ == "__main__":
    unittest.main()
