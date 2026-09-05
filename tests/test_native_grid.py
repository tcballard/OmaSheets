from contextlib import redirect_stdout
from io import StringIO
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from omasheets.cli import main as cli_main
from omasheets.errors import EngineError, PolicyError
from omasheets.native_grid import open_grid, status


class NativeGridTests(unittest.TestCase):
    def test_status_reports_the_production_install_boundary(self):
        with patch("omasheets.native_grid.grid_executable", return_value=Path("/usr/bin/omasheets-grid")):
            result = status()
        self.assertFalse(result["experimental"])
        self.assertTrue(result["ready"])

    def test_launch_starts_service_then_grid_without_a_shell(self):
        with tempfile.TemporaryDirectory() as temporary:
            document = Path(temporary) / "budget $(touch nope).omasheets"
            document.write_bytes(b"native")
            runtime = Path(temporary) / "runtime"
            grid_process = Mock(pid=91)
            with patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(runtime)}, clear=False), patch(
                "omasheets.native_grid.grid_executable", return_value=Path("/usr/bin/omasheets-grid"),
            ), patch(
                "omasheets.native_grid.service_executable", return_value=Path("/usr/bin/omasheets-service"),
            ), patch(
                "omasheets.native_grid.subprocess.Popen", return_value=grid_process,
            ) as popen:
                self.assertEqual(open_grid(document), 91)
            self.assertEqual(popen.call_args.args[0][1:4], [
                "-m", "omasheets.native_grid", "--host",
            ])
            self.assertEqual(popen.call_args.args[0][4], str(document.resolve()))
            self.assertEqual(popen.call_args.kwargs["env"]["OMASHEETS_DOCUMENT"], str(document.resolve()))
            self.assertEqual(popen.call_args.kwargs["env"]["OMASHEETS_GRID"], "/usr/bin/omasheets-grid")
            self.assertEqual(
                popen.call_args.kwargs["env"]["OMASHEETS_NATIVE_SERVICE"],
                "/usr/bin/omasheets-service",
            )
            self.assertTrue(popen.call_args.kwargs["close_fds"])
            self.assertTrue(popen.call_args.kwargs["start_new_session"])
            self.assertNotIn("shell", popen.call_args.kwargs)

    def test_launch_rejects_non_native_documents(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "book.xlsx"
            workbook.write_bytes(b"xlsx")
            with self.assertRaises(PolicyError):
                open_grid(workbook)

    def test_cli_routes_native_documents_to_the_grid(self):
        output = StringIO()
        with patch(
            "omasheets.native_grid.open_grid", return_value=93,
        ) as launch, redirect_stdout(output):
            self.assertEqual(cli_main(["launch", "book.omasheets"]), 0)
        launch.assert_called_once_with(Path("book.omasheets"))
        self.assertEqual(output.getvalue(), '{"pid": 93, "window": "native-grid"}\n')

    def test_launcher_defers_runtime_validation_to_the_supervisor(self):
        with tempfile.TemporaryDirectory() as temporary:
            document = Path(temporary) / "book.omasheets"
            document.write_bytes(b"native")
            with patch.dict(os.environ, {}, clear=True), patch(
                "omasheets.native_grid.grid_executable", return_value=Path("/usr/bin/omasheets-grid"),
            ), patch(
                "omasheets.native_grid.service_executable", return_value=Path("/usr/bin/omasheets-service"),
            ), patch(
                "omasheets.native_grid.subprocess.Popen", return_value=Mock(pid=92),
            ):
                pid = open_grid(document)
            self.assertEqual(pid, 92)

    def test_host_requires_a_runtime_directory(self):
        from omasheets.native_grid import _run_host

        with tempfile.TemporaryDirectory() as temporary:
            document = Path(temporary) / "book.omasheets"
            document.write_bytes(b"native")
            with patch.dict(os.environ, {}, clear=True), self.assertRaisesRegex(EngineError, "XDG_RUNTIME_DIR"):
                _run_host(document)

    def test_host_stops_the_transient_service_when_the_grid_closes(self):
        from omasheets.native_grid import _run_host

        service = Mock()
        service.poll.return_value = None
        grid = Mock()
        grid.wait.return_value = 0
        with tempfile.TemporaryDirectory() as temporary:
            document = Path(temporary) / "book.omasheets"
            document.write_bytes(b"native")
            with patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(Path(temporary) / "runtime")}), patch(
                "omasheets.native_grid._ensure_native_service", return_value=service,
            ), patch(
                "omasheets.native_grid.grid_executable", return_value=Path("/usr/bin/omasheets-grid"),
            ), patch("omasheets.native_grid.subprocess.Popen", return_value=grid):
                self.assertEqual(_run_host(document), 0)
        service.terminate.assert_called_once_with()
        service.wait.assert_called_once_with(timeout=5)

    def test_host_stops_the_transient_service_when_grid_start_fails(self):
        from omasheets.native_grid import _run_host

        service = Mock()
        service.poll.return_value = None
        with tempfile.TemporaryDirectory() as temporary:
            document = Path(temporary) / "book.omasheets"
            document.write_bytes(b"native")
            with patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(Path(temporary) / "runtime")}), patch(
                "omasheets.native_grid._ensure_native_service", return_value=service,
            ), patch(
                "omasheets.native_grid.grid_executable", return_value=Path("/usr/bin/omasheets-grid"),
            ), patch(
                "omasheets.native_grid.subprocess.Popen", side_effect=OSError("cannot execute"),
            ), self.assertRaisesRegex(OSError, "cannot execute"):
                _run_host(document)
        service.terminate.assert_called_once_with()
        service.wait.assert_called_once_with(timeout=5)

    def test_transient_service_cleanup_removes_only_a_dead_endpoint(self):
        from omasheets.native_grid import _stop_native_service

        service = Mock()
        service.poll.return_value = 0
        with tempfile.TemporaryDirectory() as temporary:
            runtime = Path(temporary)
            directory = runtime / "omasheets"
            directory.mkdir(mode=0o700)
            socket_path = directory / "native.sock"
            token_path = directory / "native.token"
            socket_path.write_text("stale")
            token_path.write_text("stale")
            with patch("omasheets.native_grid._service_socket_ready", return_value=False):
                _stop_native_service(runtime, service)
            self.assertFalse(socket_path.exists())
            self.assertFalse(token_path.exists())

    def test_transient_service_cleanup_preserves_a_live_replacement(self):
        from omasheets.native_grid import _stop_native_service

        service = Mock()
        service.poll.return_value = 0
        with tempfile.TemporaryDirectory() as temporary:
            runtime = Path(temporary)
            directory = runtime / "omasheets"
            directory.mkdir(mode=0o700)
            socket_path = directory / "native.sock"
            token_path = directory / "native.token"
            socket_path.write_text("replacement")
            token_path.write_text("replacement")
            with patch("omasheets.native_grid._service_socket_ready", return_value=True):
                _stop_native_service(runtime, service)
            self.assertTrue(socket_path.exists())
            self.assertTrue(token_path.exists())

    def test_launch_reports_missing_service_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            document = Path(temporary) / "book.omasheets"
            document.write_bytes(b"native")
            with patch(
                "omasheets.native_grid.grid_executable", return_value=Path("/usr/bin/omasheets-grid"),
            ), patch("omasheets.native_grid.service_executable", return_value=None), self.assertRaisesRegex(
                EngineError, "omasheets-service",
            ):
                open_grid(document)


if __name__ == "__main__":
    unittest.main()
