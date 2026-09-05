from contextlib import redirect_stdout
from io import StringIO
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch

from omasheets.cli import main as cli_main
from omasheets.errors import EngineError, PolicyError
from omasheets.native_grid import open_grid, status


class NativeGridTests(unittest.TestCase):
    def test_app_launcher_accepts_no_document(self):
        for arguments in ([], ["launch"]):
            with self.subTest(arguments=arguments), patch(
                "omasheets.native_grid.open_app", return_value=123,
            ) as launch, redirect_stdout(StringIO()) as output:
                self.assertEqual(cli_main(arguments), 0)
                launch.assert_called_once_with()
                self.assertIn('"pid": 123', output.getvalue())

    def test_home_supervisor_clears_inherited_document(self):
        from omasheets.native_grid import open_app

        with patch.dict(os.environ, {"OMASHEETS_DOCUMENT": "/old.omasheets"}), patch(
            "omasheets.native_grid.grid_executable", return_value=Path("/grid"),
        ), patch("omasheets.native_grid.service_executable", return_value=Path("/service")), patch(
            "omasheets.native_grid.subprocess.Popen", return_value=Mock(pid=42),
        ) as spawn:
            self.assertEqual(open_app(), 42)
        self.assertEqual(spawn.call_args.args[0][-1], "--host")
        self.assertNotIn("OMASHEETS_DOCUMENT", spawn.call_args.kwargs["env"])

    def test_startup_timeout_stops_the_owned_service(self):
        from omasheets.native_grid import _ensure_native_service

        service = Mock()
        service.poll.return_value = None
        with tempfile.TemporaryDirectory() as temporary, patch(
            "omasheets.native_grid._service_socket_ready", return_value=False,
        ), patch(
            "omasheets.native_grid.service_executable", return_value=Path("/service"),
        ), patch(
            "omasheets.native_grid.subprocess.Popen", return_value=service,
        ), patch("omasheets.native_grid.time.sleep"), self.assertRaisesRegex(
            EngineError, "did not become ready",
        ):
            _ensure_native_service(Path(temporary))
        service.terminate.assert_called_once_with()
        service.wait.assert_called_once_with(timeout=5)

    def test_missing_grid_does_not_start_a_service(self):
        from omasheets.native_grid import _run_host

        with patch.dict(os.environ, {"XDG_RUNTIME_DIR": "/runtime"}), patch(
            "omasheets.native_grid.grid_executable", return_value=None,
        ), patch("omasheets.native_grid._ensure_native_service") as ensure:
            with self.assertRaisesRegex(EngineError, "omasheets-grid"):
                _run_host(Path("book.omasheets"))
        ensure.assert_not_called()

    def test_owner_keeps_service_alive_until_other_window_closes(self):
        from omasheets.native_grid import _run_host

        owner_ready = threading.Event()
        peer_ready = threading.Event()
        close_owner = threading.Event()
        close_peer = threading.Event()
        owner_window_closed = threading.Event()
        stopped = threading.Event()
        errors = []
        service = Mock()

        def ensure(_runtime):
            return service if threading.current_thread().name == "owner" else None

        def spawn(*args, **kwargs):
            grid = Mock()
            is_owner = threading.current_thread().name == "owner"
            (owner_ready if is_owner else peer_ready).set()

            def wait():
                if not (close_owner if is_owner else close_peer).wait(5):
                    raise AssertionError("test window was not closed")
                if is_owner:
                    owner_window_closed.set()
                return 0

            grid.wait.side_effect = wait
            return grid

        def host():
            try:
                _run_host(Path("book.omasheets"))
            except BaseException as error:
                errors.append(error)

        with tempfile.TemporaryDirectory() as temporary, patch.dict(
            os.environ, {"XDG_RUNTIME_DIR": temporary},
        ), patch("omasheets.native_grid.grid_executable", return_value=Path("/grid")), patch(
            "omasheets.native_grid._ensure_native_service", side_effect=ensure,
        ), patch("omasheets.native_grid.subprocess.Popen", side_effect=spawn), patch(
            "omasheets.native_grid._stop_native_service", side_effect=lambda *_: stopped.set(),
        ) as stop:
            owner = threading.Thread(target=host, name="owner")
            peer = threading.Thread(target=host, name="peer")
            owner.start()
            try:
                self.assertTrue(owner_ready.wait(3))
                peer.start()
                self.assertTrue(peer_ready.wait(3))
                close_owner.set()
                self.assertTrue(owner_window_closed.wait(3))
                self.assertFalse(stopped.wait(0.1))
            finally:
                close_owner.set()
                close_peer.set()
                owner.join(5)
                if peer.ident is not None:
                    peer.join(5)
            self.assertFalse(owner.is_alive())
            self.assertFalse(peer.is_alive())
            self.assertEqual(errors, [])
            stop.assert_called_once_with(Path(temporary), service)

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
