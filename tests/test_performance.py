from pathlib import Path
import fcntl
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import xml.etree.ElementTree as ElementTree
from unittest.mock import patch

from omasheets.performance import (
    FixtureSpec,
    MemorySample,
    ProcProcessGroupSampler,
    _SampleAccumulator,
    _proc_group_has_live_members,
    _proc_process_state_and_group,
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
    @staticmethod
    def _lock_is_available(path: Path) -> bool:
        with path.open("a+") as handle:
            try:
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return False
            fcntl.flock(handle, fcntl.LOCK_UN)
        return True

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

            sampler = ProcProcessGroupSampler(proc)
            sample = sampler.sample(77, 0.25)

        self.assertEqual(sample.process_count, 2)
        self.assertEqual(sample.rss_bytes, 180 * 1024)
        self.assertEqual(sample.pss_bytes, 110 * 1024)
        self.assertEqual(sample.uss_bytes, 50 * 1024)
        self.assertEqual(sample.source, "smaps_rollup")
        self.assertEqual(sampler.last_members, (101, 102))

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

    def test_proc_liveness_ignores_zombie_only_process_group(self):
        with tempfile.TemporaryDirectory() as temporary:
            proc = Path(temporary)
            zombie = proc / "401"
            zombie.mkdir()
            (zombie / "stat").write_text("401 (worker) Z 1 77 77 0 0\n")

            self.assertEqual(_proc_process_state_and_group(401, proc), ("Z", 77))
            self.assertFalse(_proc_group_has_live_members(77, proc))

            live = proc / "402"
            live.mkdir()
            (live / "stat").write_text("402 (worker) S 1 77 77 0 0\n")
            self.assertTrue(_proc_group_has_live_members(77, proc))

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
        self.assertIsNone(result["termination_complete"])
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
        self.assertTrue(result["termination_complete"])
        self.assertNotEqual(result["exit_code"], 0)
        self.assertLess(result["wall_seconds"], 2)

    def test_timeout_kills_descendant_that_ignores_term_after_leader_exits(self):
        with tempfile.TemporaryDirectory() as temporary:
            child_pid_path = Path(temporary) / "child.pid"
            child_lock_path = Path(temporary) / "child.lock"
            child_code = (
                "import fcntl,os,signal,sys,time; from pathlib import Path; "
                "signal.signal(signal.SIGTERM, signal.SIG_IGN); "
                "lock=Path(sys.argv[2]).open('w'); fcntl.flock(lock, fcntl.LOCK_EX); "
                "Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(30)"
            )
            parent_code = (
                "import subprocess,sys,time; from pathlib import Path; "
                "subprocess.Popen([sys.executable, '-c', sys.argv[1], sys.argv[2], sys.argv[3]]); "
                "path=Path(sys.argv[2]); deadline=time.monotonic()+5; "
                "exec('while not path.exists() and time.monotonic() < deadline:\\n time.sleep(0.005)'); "
                "time.sleep(30)"
            )
            result = measure_command(
                "term-resistant-descendant",
                [
                    sys.executable, "-c", parent_code, child_code,
                    str(child_pid_path), str(child_lock_path),
                ],
                interval_seconds=0.01,
                timeout_seconds=1.0,
                max_samples=16,
                sampler=_FixedSampler(),
            )
            self.assertTrue(result["timed_out"])
            self.assertTrue(result["termination_complete"])
            if not self._lock_is_available(child_lock_path):
                os.kill(int(child_pid_path.read_text()), signal.SIGKILL)
                self.fail("timed-out descendant was left running")

    @unittest.skipUnless(Path("/proc").is_dir(), "Linux /proc is required")
    def test_timeout_kills_observed_descendant_that_creates_a_new_session(self):
        with tempfile.TemporaryDirectory() as temporary:
            child_pid_path = Path(temporary) / "setsid-child.pid"
            child_lock_path = Path(temporary) / "setsid-child.lock"
            child_code = (
                "import fcntl,os,signal,sys,time; from pathlib import Path; "
                "signal.signal(signal.SIGTERM, signal.SIG_IGN); "
                "lock=Path(sys.argv[2]).open('w'); fcntl.flock(lock, fcntl.LOCK_EX); "
                "Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(30)"
            )
            parent_code = (
                "import subprocess,sys,time; from pathlib import Path; "
                "subprocess.Popen([sys.executable, '-c', sys.argv[1], sys.argv[2], sys.argv[3]], "
                "start_new_session=True); path=Path(sys.argv[2]); deadline=time.monotonic()+5; "
                "exec('while not path.exists() and time.monotonic() < deadline:\\n time.sleep(0.005)'); "
                "time.sleep(30)"
            )
            result = measure_command(
                "setsid-descendant",
                [
                    sys.executable, "-c", parent_code, child_code,
                    str(child_pid_path), str(child_lock_path),
                ],
                interval_seconds=0.02,
                timeout_seconds=1.0,
                max_samples=64,
            )

            self.assertTrue(result["timed_out"])
            self.assertTrue(result["termination_complete"])
            self.assertGreaterEqual(result["memory"]["peak_process_count"], 2)
            if not self._lock_is_available(child_lock_path):
                os.kill(int(child_pid_path.read_text()), signal.SIGKILL)
                self.fail("session-changing descendant was left running")

    def test_timeout_allows_cooperative_descendant_to_use_grace_period(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            ready = root / "ready"
            graceful = root / "graceful"
            child_code = (
                "import signal,sys,time; from pathlib import Path; "
                "target=Path(sys.argv[2]); "
                "signal.signal(signal.SIGTERM, lambda *_: "
                "(time.sleep(0.2), target.write_text('done'), sys.exit(0))); "
                "Path(sys.argv[1]).write_text('ready'); time.sleep(30)"
            )
            parent_code = (
                "import subprocess,sys,time; from pathlib import Path; "
                "subprocess.Popen([sys.executable, '-c', sys.argv[1], sys.argv[2], sys.argv[3]]); "
                "path=Path(sys.argv[2]); deadline=time.monotonic()+5; "
                "exec('while not path.exists() and time.monotonic() < deadline:\\n time.sleep(0.005)'); "
                "time.sleep(30)"
            )
            result = measure_command(
                "cooperative-descendant",
                [sys.executable, "-c", parent_code, child_code, str(ready), str(graceful)],
                interval_seconds=0.02,
                timeout_seconds=1.0,
                max_samples=64,
            )

            self.assertTrue(result["timed_out"])
            self.assertTrue(result["termination_complete"])
            self.assertEqual(graceful.read_text(), "done")

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
