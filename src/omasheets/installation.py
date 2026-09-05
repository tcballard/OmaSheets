"""User-local production installation and conflict-safe removal."""

from __future__ import annotations

import base64
import fcntl
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from . import __version__
from .errors import ConflictError
from .integration import IntegrationPaths, install as install_integration, uninstall as uninstall_integration
from .native_bundle import download_native_bundle, install_native_bundle
from .store import read_json, write_json_atomic
from .transactions import exclusive_lock
from .user_service import UserServicePaths, uninstall as uninstall_user_service

PLUGIN_NAME = "omasheets"
PLUGIN_ENTRY = {
    "name": PLUGIN_NAME,
    "source": {"source": "local", "path": "./.codex/plugins/omasheets"},
    "policy": {"installation": "INSTALLED_BY_DEFAULT", "authentication": "ON_USE"},
    "category": "Productivity",
}
ARCH_PACKAGES = (
    "gtk3", "libreoffice-fresh", "bubblewrap",
    "qt6-base", "qt6-declarative", "qt6-wayland",
)


@dataclass(frozen=True, slots=True)
class InstallPaths:
    app: Path
    build: Path
    launcher: Path
    service_launcher: Path
    codex_plugin: Path
    codex_marketplace: Path
    journal: Path
    integration: IntegrationPaths
    user_service: UserServicePaths

    @classmethod
    def discover(cls) -> "InstallPaths":
        home = Path.home()
        data = Path(os.environ.get("XDG_DATA_HOME", home / ".local/share"))
        cache = Path(os.environ.get("XDG_CACHE_HOME", home / ".cache"))
        state = Path(os.environ.get("XDG_STATE_HOME", home / ".local/state"))
        config = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
        return cls(
            app=data / "omasheets/app",
            build=cache / "omasheets/native-bundle",
            launcher=home / ".local/bin/omasheets",
            service_launcher=home / ".local/bin/omasheets-service",
            codex_plugin=home / ".codex/plugins/omasheets",
            codex_marketplace=home / ".agents/plugins/marketplace.json",
            journal=state / "omasheets/installation.json",
            integration=IntegrationPaths(
                data / "applications/io.github.tcballard.OmaSheets.desktop",
                config / "mimeapps.list",
                state / "omasheets/desktop-integration.json",
                data / "mime/packages/io.github.tcballard.OmaSheets.xml",
            ),
            user_service=UserServicePaths(
                data / "omasheets/app/bin/omasheets-service",
                config / "systemd/user/omasheets-native.service",
                state / "omasheets/user-service.json",
            ),
        )


def _sha_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _sha_file(path: Path) -> str:
    return _sha_bytes(path.read_bytes())


def _tree_sha(path: Path) -> str:
    digest = hashlib.sha256()
    for item in sorted(
        candidate for candidate in path.rglob("*")
        if candidate.is_file()
        and "__pycache__" not in candidate.parts
        and candidate.suffix not in {".pyc", ".pyo"}
    ):
        relative = item.relative_to(path).as_posix().encode()
        digest.update(len(relative).to_bytes(4, "big"))
        digest.update(relative)
        data = item.read_bytes()
        digest.update(len(data).to_bytes(8, "big"))
        digest.update(data)
    return digest.hexdigest()


def source_identity(root: Path) -> dict[str, str]:
    completed = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--cached"],
        capture_output=True, check=True,
    )
    files = [Path(raw.decode()) for raw in completed.stdout.split(b"\0") if raw]
    digest = hashlib.sha256()
    for relative in sorted(files):
        source = root / relative
        if not source.is_file():
            continue
        data = source.read_bytes()
        encoded = relative.as_posix().encode()
        digest.update(len(encoded).to_bytes(4, "big"))
        digest.update(encoded)
        digest.update(len(data).to_bytes(8, "big"))
        digest.update(data)
    commit = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "HEAD"], text=True,
        capture_output=True, check=True,
    ).stdout.strip()
    return {"commit": commit, "sha256": digest.hexdigest()}


def dependency_report() -> dict[str, Any]:
    checks = [
        ("GTK3", _first_existing("/usr/lib/libgtk-3.so", "/usr/lib/libgtk-3.so.0")),
        ("LibreOffice", _first_existing("/usr/bin/soffice", "/usr/bin/libreoffice")),
        ("LibreOfficeKitGTK", _first_existing(
            "/usr/lib/libreofficekitgtk.so",
            "/usr/lib/liblibreofficekitgtk.so",
            "/usr/lib/libreoffice/program/liblibreofficekitgtk.so",
        )),
        ("Bubblewrap", _first_existing("/usr/bin/bwrap")),
        ("Qt Quick", _first_existing("/usr/lib/libQt6Quick.so", "/usr/lib/libQt6Quick.so.6")),
        ("Qt Wayland", _first_existing(
            "/usr/lib/qt6/plugins/platforms/libqwayland.so",
            "/usr/lib/qt6/plugins/platforms/libqwayland-egl.so",
            "/usr/lib/qt6/plugins/platforms/libqwayland-generic.so",
        )),
        ("system Python", _first_existing("/usr/bin/python")),
        ("Python UNO", _python_uno()),
    ]
    return {
        "ready": all(detail is not None for _, detail in checks),
        "checks": [{"name": name, "ok": detail is not None, "detail": detail or "missing"} for name, detail in checks],
        "install_command": "omarchy pkg add " + " ".join(ARCH_PACKAGES),
    }


def _first_existing(*values: str) -> str | None:
    return next((value for value in values if Path(value).is_file()), None)


def _python_uno() -> str | None:
    python = _first_existing("/usr/bin/python")
    if not python:
        return None
    result = subprocess.run([python, "-c", "import uno"], capture_output=True)
    return "importable" if result.returncode == 0 else None


def _launcher(app: Path) -> bytes:
    module_root = app / "lib"
    return (
        "#!/bin/bash\nset -euo pipefail\n"
        "export PYTHONDONTWRITEBYTECODE=1\n"
        f"export PYTHONPATH={shlex.quote(str(module_root))}\n"
        f"export PATH={shlex.quote(str(app / 'bin'))}:\"$PATH\"\n"
        "export OMASHEETS_PYTHON=/usr/bin/python\n"
        "exec /usr/bin/python -m omasheets.cli \"$@\"\n"
    ).encode()


def _service_launcher(app: Path) -> bytes:
    executable = app / "bin/omasheets-service"
    return (
        "#!/bin/bash\nset -euo pipefail\n"
        f"exec {shlex.quote(str(executable))} \"$@\"\n"
    ).encode()


def _marketplace_after(before: bytes | None) -> bytes:
    if before:
        payload = json.loads(before)
        plugins = payload.get("plugins")
        if not isinstance(plugins, list):
            raise ConflictError("personal Codex marketplace has no plugins array")
        matches = [item for item in plugins if isinstance(item, dict) and item.get("name") == PLUGIN_NAME]
        if matches and matches != [PLUGIN_ENTRY]:
            raise ConflictError("personal Codex marketplace already contains a different OmaSheets entry")
        if not matches:
            plugins.append(PLUGIN_ENTRY)
    else:
        payload = {
            "name": "personal-plugins",
            "interface": {"displayName": "Personal plugins"},
            "plugins": [PLUGIN_ENTRY],
        }
    return (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode()


def _write_bytes(path: Path, data: bytes, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.fchmod(descriptor, mode)
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def install(
    source_root: Path, paths: InstallPaths | None = None, *,
    check_dependencies: bool = True,
    bundle_path: Path | None = None,
) -> dict[str, Any]:
    paths = paths or InstallPaths.discover()
    with exclusive_lock(paths.journal.parent / ".installation.lock"):
        return _install_locked(source_root, paths, check_dependencies=check_dependencies, bundle_path=bundle_path)


def _install_locked(source_root: Path, paths: InstallPaths, *, check_dependencies: bool, bundle_path: Path | None) -> dict[str, Any]:
    source_root = source_root.resolve(strict=True)
    identity = source_identity(source_root)
    previous_journal = None
    if paths.journal.is_file():
        journal = read_json(paths.journal)
        intact = paths.launcher.is_file() and _sha_file(paths.launcher) == journal["launcher_sha256"]
        service_launcher_sha256 = journal.get("service_launcher_sha256")
        intact = intact and service_launcher_sha256 is not None
        intact = intact and paths.service_launcher.is_file()
        if intact:
            intact = _sha_file(paths.service_launcher) == service_launcher_sha256
        intact = intact and paths.app.is_dir() and _tree_sha(paths.app) == journal["app_sha256"]
        intact = intact and paths.codex_plugin.is_dir()
        intact = intact and _tree_sha(paths.codex_plugin) == journal["codex_plugin_sha256"]
        intact = intact and paths.codex_marketplace.is_file()
        if intact:
            marketplace = json.loads(paths.codex_marketplace.read_text())
            intact = PLUGIN_ENTRY in marketplace.get("plugins", [])
        intact = intact and paths.integration.desktop.is_file() and paths.integration.journal.is_file()
        if not intact:
            raise ConflictError("installed OmaSheets files changed; resolve them before updating")
        if journal["source"] == identity:
            return {"installed": True, "changed": False, "source": journal["source"]}
        previous_journal = journal
    report = dependency_report()
    if check_dependencies and not report["ready"]:
        missing = ", ".join(check["name"] for check in report["checks"] if not check["ok"])
        raise RuntimeError(f"missing dependencies: {missing}\nInstall them explicitly with: {report['install_command']}")
    for target in (paths.app, paths.launcher, paths.service_launcher, paths.codex_plugin):
        if target.exists() and previous_journal is None:
            raise ConflictError(f"refusing to overwrite unowned installation target: {target}")
    paths.app.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=".app.", dir=paths.app.parent))
    marketplace_before = paths.codex_marketplace.read_bytes() if paths.codex_marketplace.is_file() else None
    marketplace_after = _marketplace_after(marketplace_before)
    backups: dict[Path, Path] = {}
    published: set[Path] = set()
    launcher_before = {
        path: path.read_bytes() if path.is_file() else None
        for path in (paths.launcher, paths.service_launcher)
    }
    lease = None
    try:
        shutil.copytree(
            source_root / "src/omasheets", stage / "lib/omasheets",
            ignore=shutil.ignore_patterns("__pycache__", "*.pyc", "*.pyo"),
        )
        configured_bundle = bundle_path or (
            Path(value) if (value := os.environ.get("OMASHEETS_NATIVE_BUNDLE_PATH")) else None
        )
        if configured_bundle is None:
            configured_bundle = download_native_bundle(
                __version__, paths.build, source_root=source_root,
            )
        configured_bundle = configured_bundle.expanduser().resolve(strict=True)
        native_manifest = install_native_bundle(
            configured_bundle, stage, version=__version__, source=identity,
        )
        provenance = {
            "schema": 1, "version": __version__, "source": identity,
            "native_bundle": native_manifest,
        }
        _write_bytes(stage / "provenance.json", (json.dumps(provenance, indent=2, sort_keys=True) + "\n").encode())
        if previous_journal is not None:
            runtime = os.environ.get("XDG_RUNTIME_DIR")
            if runtime and (Path(runtime) / "omasheets").is_dir():
                lease = (Path(runtime) / "omasheets/grid-clients.lock").open("a+b")
                os.chmod(lease.name, 0o600)
                try:
                    fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    raise ConflictError("Close OmaSheets windows before updating; your workbooks are preserved") from None
                from .native_grid import _service_socket_ready
                if _service_socket_ready(Path(runtime) / "omasheets/native.sock"):
                    raise ConflictError("The native service is still running. Close OmaSheets, stop any optional user service, then retry the update")
            for target in (paths.app, paths.codex_plugin):
                backup = Path(tempfile.mkdtemp(prefix=f".{target.name}.previous.", dir=target.parent))
                backup.rmdir()
                os.replace(target, backup)
                backups[target] = backup
        os.replace(stage, paths.app)
        published.add(paths.app)
        _write_bytes(paths.launcher, _launcher(paths.app), 0o755)
        _write_bytes(paths.service_launcher, _service_launcher(paths.app), 0o755)
        paths.codex_plugin.parent.mkdir(parents=True, exist_ok=True)
        published.add(paths.codex_plugin)
        shutil.copytree(source_root / "plugins/omasheets", paths.codex_plugin)
        mcp_path = paths.codex_plugin / ".mcp.json"
        mcp = json.loads(mcp_path.read_text())
        mcp["mcpServers"]["omasheets"]["command"] = str(paths.launcher)
        _write_bytes(mcp_path, (json.dumps(mcp, indent=2, sort_keys=True) + "\n").encode())
        _write_bytes(paths.codex_marketplace, marketplace_after)
        install_integration(paths.integration, executable=paths.launcher)
        journal = {
            "schema": 1, "version": __version__, "source": identity,
            "app_sha256": _tree_sha(paths.app),
            "launcher_sha256": _sha_file(paths.launcher),
            "service_launcher_sha256": _sha_file(paths.service_launcher),
            "codex_plugin_sha256": _tree_sha(paths.codex_plugin),
            "marketplace_before": previous_journal["marketplace_before"] if previous_journal is not None else (
                base64.b64encode(marketplace_before).decode() if marketplace_before is not None else None
            ),
            "marketplace_after_sha256": _sha_bytes(marketplace_after),
        }
        write_json_atomic(paths.journal, journal)
        for backup in backups.values():
            shutil.rmtree(backup, ignore_errors=True)
        return {
            "installed": True,
            "changed": True,
            "updated": previous_journal is not None,
            "source": identity,
            "launcher": str(paths.launcher),
            "service_launcher": str(paths.service_launcher),
        }
    except Exception:
        for target in published:
            if target.exists():
                shutil.rmtree(target)
        for target, backup in backups.items():
            os.replace(backup, target)
        for launcher, before in launcher_before.items():
            if before is None:
                launcher.unlink(missing_ok=True)
            else:
                _write_bytes(launcher, before, 0o755)
        if marketplace_before is None:
            paths.codex_marketplace.unlink(missing_ok=True)
        else:
            _write_bytes(paths.codex_marketplace, marketplace_before)
        if previous_journal is None:
            try:
                uninstall_integration(paths.integration)
            except Exception:
                pass
        raise
    finally:
        if stage.exists():
            shutil.rmtree(stage)
        if lease is not None:
            lease.close()


def _remove_marketplace_entry(path: Path, journal: dict[str, Any]) -> None:
    if not path.exists():
        return
    current = path.read_bytes()
    previous = journal.get("marketplace_before")
    if _sha_bytes(current) == journal["marketplace_after_sha256"]:
        if previous is None:
            path.unlink()
        else:
            _write_bytes(path, base64.b64decode(previous))
        return
    payload = json.loads(current)
    plugins = payload.get("plugins", [])
    payload["plugins"] = [item for item in plugins if not (isinstance(item, dict) and item == PLUGIN_ENTRY)]
    _write_bytes(path, (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode())


def uninstall(paths: InstallPaths | None = None) -> dict[str, Any]:
    paths = paths or InstallPaths.discover()
    try:
        service_result = uninstall_user_service(paths.user_service)
    except (ConflictError, RuntimeError) as error:
        return {"installed": True, "changed": False, "conflicts": [str(error)]}
    if not paths.journal.is_file():
        return {
            "installed": False,
            "changed": service_result["changed"],
            "conflicts": [],
        }
    journal = read_json(paths.journal)
    conflicts: list[str] = []
    try:
        uninstall_integration(paths.integration)
    except ConflictError as error:
        conflicts.append(str(error))
    _remove_marketplace_entry(paths.codex_marketplace, journal)
    owned = [
        (paths.launcher, journal["launcher_sha256"], _sha_file),
        (paths.codex_plugin, journal["codex_plugin_sha256"], _tree_sha),
        (paths.app, journal["app_sha256"], _tree_sha),
    ]
    if service_launcher_sha256 := journal.get("service_launcher_sha256"):
        owned.insert(1, (paths.service_launcher, service_launcher_sha256, _sha_file))
    for target, expected, hasher in owned:
        if not target.exists():
            continue
        if hasher(target) != expected:
            conflicts.append(f"modified installation target was preserved: {target}")
            continue
        if target.is_dir():
            shutil.rmtree(target)
        else:
            target.unlink()
    if not conflicts:
        paths.journal.unlink()
    return {"installed": bool(conflicts), "changed": True, "conflicts": conflicts}
