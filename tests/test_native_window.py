from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from omasheets.errors import EngineError, PolicyError
from omasheets.native_window import open_window, status


class NativeWindowTests(unittest.TestCase):
    def test_status_reports_the_production_install_boundary(self):
        with patch("omasheets.native_window.window_executable", return_value=Path("/usr/bin/omasheets-window")):
            result = status()
        self.assertFalse(result["experimental"])
        self.assertTrue(result["ready"])

    def test_launch_uses_fixed_argv_without_a_shell(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "budget $(touch nope).xlsx"
            workbook.write_bytes(b"xlsx")
            with patch("omasheets.native_window.window_executable", return_value=Path("/usr/bin/omasheets-window")), patch(
                "omasheets.native_window.subprocess.Popen"
            ) as popen:
                popen.return_value.pid = 84
                self.assertEqual(open_window(workbook), 84)
            self.assertEqual(popen.call_args.args[0], ["/usr/bin/omasheets-window", str(workbook.resolve())])
            self.assertNotIn("shell", popen.call_args.kwargs)
            self.assertTrue(popen.call_args.kwargs["close_fds"])

    def test_launch_passes_private_agent_context_as_fixed_arguments(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "book.xlsx"
            context = Path(temporary) / "window-context.json"
            workbook.write_bytes(b"xlsx")
            with patch("omasheets.native_window.window_executable", return_value=Path("/usr/bin/omasheets-window")), patch(
                "omasheets.native_window.subprocess.Popen"
            ) as popen:
                popen.return_value.pid = 85
                bridge = Path(temporary) / "bridge.sock"
                diff = Path(temporary) / "window-diff.overlay"
                cli = Path("/usr/bin/omasheets")
                open_window(
                    workbook, context_path=context, bridge_path=bridge,
                    diff_path=diff, cli_path=cli, session_id="a" * 32, revision=3,
                )
            self.assertEqual(popen.call_args.args[0], [
                "/usr/bin/omasheets-window", "--context", str(context),
                "--bridge", str(bridge), "--diff", str(diff), "--cli", str(cli),
                "--session", "a" * 32, "--revision", "3", str(workbook.resolve()),
            ])

    def test_launch_rejects_unsupported_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            document = Path(temporary) / "notes.txt"
            document.write_text("not a workbook")
            with self.assertRaises(PolicyError):
                open_window(document)

    def test_launch_reports_missing_native_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "book.ods"
            workbook.write_bytes(b"ods")
            with patch("omasheets.native_window.window_executable", return_value=None), self.assertRaises(EngineError):
                open_window(workbook)


if __name__ == "__main__":
    unittest.main()
