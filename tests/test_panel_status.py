import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

from omasheets.bounded_process import run_bounded


ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / "scripts/panel_status.py"


def _alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    try:
        # A zombie still answers kill(0); reap-check through /proc when available.
        state = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
        return state not in {"Z", "X"}
    except (OSError, IndexError):
        return True


class BoundedProcessTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def script(self, body: str) -> Path:
        path = self.root / "launcher"
        path.write_text("#!/bin/bash\n" + body)
        path.chmod(0o755)
        return path

    def test_complete_output_within_limits_is_returned(self):
        launcher = self.script('printf \'{"ok":true}\'\n')
        result = run_bounded([str(launcher)], byte_limit=64, timeout_seconds=5)
        self.assertEqual(result.status, "ok")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.output, b'{"ok":true}')
        self.assertTrue(result.ok)

    def test_overflow_stops_reading_and_terminates_the_group(self):
        pid_file = self.root / "pid"
        launcher = self.script(
            f"echo $$ > {pid_file}\n"
            "while :; do printf '%01000d' 0 || exit 0; done\n"
        )
        started = time.monotonic()
        result = run_bounded([str(launcher)], byte_limit=4096, timeout_seconds=5)
        self.assertEqual(result.status, "overflow")
        self.assertLessEqual(len(result.output), 4096)
        self.assertLess(time.monotonic() - started, 4)
        self.assertFalse(_alive(int(pid_file.read_text())))
        self.assertFalse(result.ok)

    def test_deadline_terminates_the_whole_process_group(self):
        pid_file = self.root / "pids"
        launcher = self.script(
            f"sleep 30 &\necho $! > {pid_file}\necho $$ >> {pid_file}\nwait\n"
        )
        started = time.monotonic()
        result = run_bounded([str(launcher)], byte_limit=1024, timeout_seconds=0.5, grace_seconds=0.5)
        elapsed = time.monotonic() - started
        self.assertEqual(result.status, "timeout")
        self.assertLess(elapsed, 3)
        deadline = time.monotonic() + 2
        pids = [int(value) for value in pid_file.read_text().split()]
        while time.monotonic() < deadline and any(_alive(pid) for pid in pids):
            time.sleep(0.05)
        self.assertEqual([pid for pid in pids if _alive(pid)], [])
        self.assertFalse(result.ok)

    def test_term_ignoring_helper_is_killed(self):
        launcher = self.script("trap '' TERM\nsleep 30\n")
        started = time.monotonic()
        result = run_bounded([str(launcher)], byte_limit=1024, timeout_seconds=0.3, grace_seconds=0.3)
        self.assertEqual(result.status, "timeout")
        self.assertLess(time.monotonic() - started, 3)
        self.assertEqual(result.returncode, -signal.SIGKILL)

    def test_output_that_arrives_after_exit_is_still_bounded(self):
        launcher = self.script("printf 'x%.0s' {1..3000}\n")
        result = run_bounded([str(launcher)], byte_limit=2048, timeout_seconds=5)
        self.assertEqual(result.status, "overflow")
        self.assertEqual(len(result.output), 2048)

    def test_limits_must_be_positive(self):
        with self.assertRaises(ValueError):
            run_bounded(["true"], byte_limit=0, timeout_seconds=1)
        with self.assertRaises(ValueError):
            run_bounded(["true"], byte_limit=1, timeout_seconds=0)


class PanelStatusHelperTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def launcher(self, body: str) -> Path:
        path = self.root / "omasheets"
        path.write_text("#!/bin/bash\n" + body)
        path.chmod(0o755)
        return path

    def run_helper(self, launcher: Path) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, str(HELPER), str(launcher)], text=True, capture_output=True, timeout=30,
        )

    def test_missing_launcher_reports_not_installed(self):
        completed = self.run_helper(self.root / "absent")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(
            json.loads(completed.stdout),
            {"installed": False, "current": {"selected": False}, "review": {"pending": False}},
        )

    def test_valid_status_is_re_emitted_compactly(self):
        launcher = self.launcher(
            'test "$1 $2" = "status --json" || exit 3\n'
            'printf \'{\\n  "current": {"selected": false},\\n  "review": {"pending": false}\\n}\\n\'\n'
        )
        completed = self.run_helper(launcher)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stdout, '{"current":{"selected":false},"review":{"pending":false}}\n')

    def test_oversized_or_invalid_status_fails_closed(self):
        for body, message in (
            ("head -c 20000 /dev/zero | tr '\\0' 'x'\n", "overflow"),
            ("printf 'not json'\n", "not valid JSON"),
            ("printf '[1]'\n", "not a JSON object"),
            ("printf '{}'\nexit 4\n", "exit 4"),
        ):
            with self.subTest(message=message):
                completed = self.run_helper(self.launcher(body))
                self.assertEqual(completed.returncode, 1)
                self.assertEqual(completed.stdout, "")
                self.assertIn(message, completed.stderr)

    def test_hanging_launcher_hits_the_helper_deadline(self):
        launcher = self.launcher("sleep 60\n")
        started = time.monotonic()
        completed = self.run_helper(launcher)
        self.assertEqual(completed.returncode, 1)
        self.assertIn("timeout", completed.stderr)
        self.assertLess(time.monotonic() - started, 12)

    def test_helper_rejects_bad_usage(self):
        completed = subprocess.run([sys.executable, str(HELPER)], text=True, capture_output=True, timeout=30)
        self.assertEqual(completed.returncode, 2)


if __name__ == "__main__":
    unittest.main()
