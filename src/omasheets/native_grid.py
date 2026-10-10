"""Launch the production Qt grid against the authenticated native service."""

from __future__ import annotations

import fcntl
import json
import os
from pathlib import Path
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import time
from typing import Any

from .errors import EngineError, PolicyError
from .identity import identify_regular_file
from .policy import require_agent_readable
from .bounded_process import run_bounded


def kit_executable() -> Path | None:
    configured = os.environ.get("OMASHEETS_KIT")
    if configured:
        candidate = Path(configured).expanduser()
        return candidate if candidate.is_file() and os.access(candidate, os.X_OK) else None
    discovered = shutil.which("omasheets-kit")
    return Path(discovered) if discovered else None


def open_workbook(path: Path) -> int:
    """Admit XLSX before opening a durable native copy in the owned grid."""

    source = path.expanduser().resolve(strict=True)
    require_agent_readable(source)
    identify_regular_file(source)
    kit = kit_executable()
    if kit is None:
        raise EngineError("omasheets-kit is not installed; run OmaSheets setup")
    if source.suffix.lower() == ".xlsx":
        data = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share"))
        root = data / "omasheets/native-kit"
        root.mkdir(mode=0o700, parents=True, exist_ok=True)
        working = Path(tempfile.mkdtemp(prefix="workbook-", dir=root))
        document = working / f"{source.stem}.omasheets"
        # The Rust kit stages immutable bytes, refuses known losses, publishes
        # without clobbering, and checks native replay before returning.
        try:
            result = run_bounded(
                [str(kit), "import", str(source), str(document)],
                byte_limit=4 * 1024 * 1024,
                timeout_seconds=90,
            )
            if not result.ok:
                try:
                    detail = json.loads(result.output).get("error", "Workbook import was refused")
                except (ValueError, AttributeError):
                    detail = f"Workbook import did not complete ({result.status})"
                raise EngineError(str(detail)[:2000])
        except BaseException:
            shutil.rmtree(working, ignore_errors=True)
            raise
        source = document
    return _open_kit(source, kit)


def _open_kit(source: Path, kit: Path) -> int:
    grid = grid_executable()
    service = service_executable()
    if grid is None or service is None:
        raise EngineError("The OmaSheets grid and document service must be installed")
    environment = os.environ.copy()
    environment.pop("OMASHEETS_DOCUMENT", None)
    environment["OMASHEETS_GRID"] = str(grid)
    environment["OMASHEETS_NATIVE_SERVICE"] = str(service)
    return subprocess.Popen(
        [str(kit), "open", str(source)],
        env=environment,
        close_fds=True,
        start_new_session=True,
    ).pid


def grid_executable() -> Path | None:
    configured = os.environ.get("OMASHEETS_GRID")
    if configured:
        candidate = Path(configured).expanduser()
        return candidate if candidate.is_file() and os.access(candidate, os.X_OK) else None
    discovered = shutil.which("omasheets-grid")
    return Path(discovered) if discovered else None


def service_executable() -> Path | None:
    configured = os.environ.get("OMASHEETS_NATIVE_SERVICE")
    if configured:
        candidate = Path(configured).expanduser()
        return candidate if candidate.is_file() and os.access(candidate, os.X_OK) else None
    discovered = shutil.which("omasheets-service")
    return Path(discovered) if discovered else None


def status() -> dict[str, Any]:
    executable = grid_executable()
    return {
        "experimental": False,
        "ready": executable is not None,
        "executable": str(executable) if executable else None,
        "detail": str(executable) if executable else "run the OmaSheets user-local installer",
    }


def _runtime_base() -> Path:
    value = os.environ.get("XDG_RUNTIME_DIR")
    if not value:
        raise EngineError("XDG_RUNTIME_DIR is not set; cannot start the native document service")
    return Path(value)


def _service_socket_ready(path: Path) -> bool:
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
            probe.settimeout(0.1)
            probe.connect(str(path))
        return True
    except OSError:
        return False


def _service_directory(runtime: Path) -> Path:
    directory = runtime / "omasheets"
    directory.mkdir(parents=True, mode=0o700, exist_ok=True)
    if stat.S_IMODE(directory.stat().st_mode) & 0o077:
        raise EngineError(f"{directory} must not be readable by other users")
    return directory


def _ensure_native_service(runtime: Path) -> subprocess.Popen | None:
    directory = _service_directory(runtime)
    socket_path = directory / "native.sock"
    if _service_socket_ready(socket_path):
        return None
    lock_path = directory / "grid-service.lock"
    with lock_path.open("a+b") as lock:
        os.chmod(lock_path, 0o600)
        fcntl.flock(lock, fcntl.LOCK_EX)
        if _service_socket_ready(socket_path):
            return None
        executable = service_executable()
        if executable is None:
            raise EngineError("omasheets-service is not installed; run OmaSheets setup from the Omarchy widget")
        process = subprocess.Popen(
            [str(executable), "serve", "--runtime-dir", str(runtime)],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            start_new_session=True,
        )
        for _ in range(100):
            if _service_socket_ready(socket_path):
                if process.poll() is None:
                    return process
                break
            if process.poll() is not None:
                break
            time.sleep(0.05)
    _stop_native_service(runtime, process)
    raise EngineError("the native document service did not become ready")


def _stop_native_service(runtime: Path, process: subprocess.Popen) -> None:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    directory = runtime / "omasheets"
    if not directory.is_dir():
        return
    lock_path = directory / "grid-service.lock"
    with lock_path.open("a+b") as lock:
        os.chmod(lock_path, 0o600)
        fcntl.flock(lock, fcntl.LOCK_EX)
        socket_path = directory / "native.sock"
        if not _service_socket_ready(socket_path):
            socket_path.unlink(missing_ok=True)
            (directory / "native.token").unlink(missing_ok=True)


def _run_host(source: Path | None) -> int:
    runtime = _runtime_base()
    executable = grid_executable()
    if executable is None:
        raise EngineError("omasheets-grid is not installed; run OmaSheets setup from the Omarchy widget")
    environment = os.environ.copy()
    environment.pop("OMASHEETS_DOCUMENT", None)
    if source is not None:
        environment["OMASHEETS_DOCUMENT"] = str(source)
    # Every grid holds a shared lease, including grids reusing a service.
    # The owning supervisor survives its window until all leases are released.
    # An exclusive lease also prevents a new window racing service shutdown.
    lease_path = _service_directory(runtime) / "grid-clients.lock"
    with lease_path.open("a+b") as lease:
        os.chmod(lease_path, 0o600)
        fcntl.flock(lease, fcntl.LOCK_SH)
        owned_service = _ensure_native_service(runtime)
        try:
            grid = subprocess.Popen(
                [str(executable)] + ([str(source)] if source is not None else []),
                env=environment,
                close_fds=True,
            )
            return grid.wait()
        finally:
            fcntl.flock(lease, fcntl.LOCK_UN)
            if owned_service is not None:
                fcntl.flock(lease, fcntl.LOCK_EX)
                _stop_native_service(runtime, owned_service)


def open_grid(path: Path) -> int:
    source = path.expanduser().resolve(strict=True)
    if source.suffix.lower() != ".omasheets":
        raise PolicyError("the native grid opens .omasheets documents only")
    return open_workbook(source)


def open_app() -> int:
    return _open_host(None)


def _open_host(source: Path | None) -> int:
    executable = grid_executable()
    if executable is None:
        raise EngineError("omasheets-grid is not installed; run OmaSheets setup from the Omarchy widget")
    environment = os.environ.copy()
    environment.pop("OMASHEETS_DOCUMENT", None)
    if source is not None:
        environment["OMASHEETS_DOCUMENT"] = str(source)
    environment["OMASHEETS_GRID"] = str(executable)
    service = service_executable()
    if service is None:
        raise EngineError("omasheets-service is not installed; run OmaSheets setup from the Omarchy widget")
    environment["OMASHEETS_NATIVE_SERVICE"] = str(service)
    process = subprocess.Popen(
        [sys.executable, "-m", "omasheets.native_grid", "--host"]
        + ([str(source)] if source is not None else []),
        env=environment,
        close_fds=True,
        start_new_session=True,
    )
    return process.pid


def main(argv: list[str] | None = None) -> int:
    arguments = list(sys.argv[1:] if argv is None else argv)
    if len(arguments) not in {1, 2} or arguments[0] != "--host":
        raise SystemExit("usage: python -m omasheets.native_grid --host [DOCUMENT.omasheets]")
    return _run_host(Path(arguments[1]).resolve(strict=True) if len(arguments) == 2 else None)


if __name__ == "__main__":
    raise SystemExit(main())
