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


class NativeWindowSourceContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = (Path(__file__).parents[1] / "native/libreofficekit/window.cpp").read_text()

    def _function(self, name: str) -> str:
        return self.source.split(name, 1)[1].split("\n}\n", 1)[0]

    def test_diff_overlay_uses_directory_events_with_failure_only_fallback(self):
        watcher = self._function("void start_diff_overlay_watch")
        self.assertIn("state->diff_path.parent_path()", watcher)
        self.assertIn("g_file_monitor_directory", watcher)
        self.assertIn("G_FILE_MONITOR_WATCH_MOVES", watcher)
        self.assertIn('g_signal_connect(\n            state->diff_monitor, "changed"', watcher)
        self.assertIn("if (state->diff_monitor != nullptr)", watcher)
        self.assertIn("} else {", watcher)
        self.assertIn("g_timeout_add_seconds(\n            kDiffFallbackSeconds", watcher)
        self.assertIn("constexpr guint kDiffFallbackSeconds = 5;", self.source)
        self.assertNotIn("g_timeout_add(200, poll_diff_overlay", self.source)

    def test_diff_overlay_monitor_handles_atomic_replacement_and_filters_noise(self):
        events = self._function("bool diff_monitor_event_is_relevant")
        for event in (
            "G_FILE_MONITOR_EVENT_CHANGED",
            "G_FILE_MONITOR_EVENT_CHANGES_DONE_HINT",
            "G_FILE_MONITOR_EVENT_DELETED",
            "G_FILE_MONITOR_EVENT_CREATED",
            "G_FILE_MONITOR_EVENT_RENAMED",
            "G_FILE_MONITOR_EVENT_MOVED_IN",
            "G_FILE_MONITOR_EVENT_MOVED_OUT",
        ):
            self.assertIn(event, events)
        target_filter = self._function("bool diff_monitor_event_targets_overlay")
        self.assertIn("g_file_equal(file, state->diff_file)", target_filter)
        self.assertIn("g_file_equal(other_file, state->diff_file)", target_filter)
        callback = self._function("void on_diff_overlay_changed")
        self.assertIn("diff_monitor_event_is_relevant(event)", callback)
        self.assertIn("diff_monitor_event_targets_overlay(state, file, other_file)", callback)
        self.assertGreater(
            callback.index("refresh_diff_overlay(state)"),
            callback.index("diff_monitor_event_targets_overlay"),
        )

    def test_diff_overlay_watch_loads_initial_state_and_cleans_up(self):
        watcher = self._function("void start_diff_overlay_watch")
        self.assertGreater(
            watcher.rindex("refresh_diff_overlay(state)"),
            watcher.index("g_signal_connect"),
        )
        cleanup = self._function("void stop_diff_overlay_watch")
        self.assertIn("g_source_remove(state->diff_fallback_source)", cleanup)
        self.assertIn("g_signal_handler_disconnect", cleanup)
        self.assertIn("g_file_monitor_cancel", cleanup)
        self.assertIn("g_object_unref(state->diff_monitor)", cleanup)
        self.assertIn("g_object_unref(state->diff_file)", cleanup)
        destroy = self._function("void on_destroy")
        self.assertIn("stop_diff_overlay_watch(state)", destroy)

    def test_identical_context_state_is_not_rewritten(self):
        writer = self._function("void write_context_now")
        self.assertIn("const std::string context_state = serialize_context(state, 0)", writer)
        self.assertIn("context_state == state->last_context_state", writer)
        self.assertGreater(
            writer.index("state->last_context_state = context_state"),
            writer.index("fs::rename(temporary, state->context_path)"),
        )


if __name__ == "__main__":
    unittest.main()
