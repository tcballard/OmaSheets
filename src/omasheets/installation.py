"""User-local production installation and conflict-safe removal."""

from __future__ import annotations

import base64
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from . import __version__
from .errors import ConflictError
from .integration import IntegrationPaths, install as install_integration, uninstall as uninstall_integration
from .store import read_json, write_json_atomic

PLUGIN_NAME = "omasheets"
PLUGIN_ENTRY = {
    "name": PLUGIN_NAME,
    "source": {"source": "local", "path": "./.codex/plugins/omasheets"},
    "policy": {"installation": "INSTALLED_BY_DEFAULT", "authentication": "ON_USE"},
    "category": "Productivity",
}
ARCH_PACKAGES = (
    "gcc", "make", "cmake", "pkgconf", "gtk3", "libreoffice-fresh",
    "libreoffice-fresh-sdk", "bubblewrap",
)


@dataclass(frozen=True, slots=True)
class InstallPaths:
    app: Path
    build: Path
    launcher: Path
    codex_plugin: Path
    codex_marketplace: Path
    journal: Path
    integration: IntegrationPaths

    @classmethod
    def discover(cls) -> "InstallPaths":
        home = Path.home()
        data = Path(os.environ.get("XDG_DATA_HOME", home / ".local/share"))
        cache = Path(os.environ.get("XDG_CACHE_HOME", home / ".cache"))
        state = Path(os.environ.get("XDG_STATE_HOME", home / ".local/state"))
        config = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
        return cls(
            app=data / "omasheets/app",
            build=cache / "omasheets/native-build",
            launcher=home / ".local/bin/omasheets",
            codex_plugin=home / ".codex/plugins/omasheets",
            codex_marketplace=home / ".agents/plugins/marketplace.json",
            journal=state / "omasheets/installation.json",
            integration=IntegrationPaths(
                data / "applications/io.github.tcballard.OmaSheets.desktop",
                config / "mimeapps.list",
                state / "omasheets/desktop-integration.json",
            ),
        )


def _sha_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _sha_file(path: Path) -> str:
    return _sha_bytes(path.read_bytes())


def _tree_sha(path: Path) -> str:
    digest = hashlib.sha256()
    for item in sorted(candidate for candidate in path.rglob("*") if candidate.is_file()):
        relative = item.relative_to(path).as_posix().encode()
        digest.update(len(relative).to_bytes(4, "big"))
        digest.update(relative)
        data = item.read_bytes()
        digest.update(len(data).to_bytes(8, "big"))
        digest.update(data)
    return digest.hexdigest()


def source_identity(root: Path) -> dict[str, str]:
    completed = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
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
        ("cmake", shutil.which("cmake")),
        ("C++ compiler", shutil.which("c++")),
        ("make", shutil.which("make")),
        ("pkg-config", shutil.which("pkg-config")),
        ("GTK3", _pkg_config("gtk+-3.0")),
        ("LibreOffice", _first_existing("/usr/bin/soffice", "/usr/bin/libreoffice")),
        ("LibreOfficeKit headers", _first_existing("/usr/include/libreoffice/LibreOfficeKit/LibreOfficeKit.hxx")),
        ("LibreOfficeKitGTK", _first_existing(
            "/usr/lib/libreofficekitgtk.so",
            "/usr/lib/liblibreofficekitgtk.so",
            "/usr/lib/libreoffice/program/liblibreofficekitgtk.so",
        )),
        ("Bubblewrap", _first_existing("/usr/bin/bwrap")),
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


def _pkg_config(package: str) -> str | None:
    executable = shutil.which("pkg-config")
    if not executable:
        return None
    result = subprocess.run([executable, "--modversion", package], text=True, capture_output=True)
    return result.stdout.strip() if result.returncode == 0 else None


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
        f"export PYTHONPATH={shlex.quote(str(module_root))}\n"
        f"export PATH={shlex.quote(str(app / 'bin'))}:\"$PATH\"\n"
        "exec /usr/bin/python -m omasheets.cli \"$@\"\n"
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
    runner: Callable[..., subprocess.CompletedProcess[Any]] = subprocess.run,
) -> dict[str, Any]:
    paths = paths or InstallPaths.discover()
    source_root = source_root.resolve(strict=True)
    if paths.journal.is_file():
        journal = read_json(paths.journal)
        intact = paths.launcher.is_file() and _sha_file(paths.launcher) == journal["launcher_sha256"]
        intact = intact and paths.app.is_dir() and _tree_sha(paths.app) == journal["app_sha256"]
        intact = intact and paths.codex_plugin.is_dir()
        intact = intact and _tree_sha(paths.codex_plugin) == journal["codex_plugin_sha256"]
        intact = intact and paths.codex_marketplace.is_file()
        if intact:
            marketplace = json.loads(paths.codex_marketplace.read_text())
            intact = PLUGIN_ENTRY in marketplace.get("plugins", [])
        intact = intact and paths.integration.desktop.is_file() and paths.integration.journal.is_file()
        if intact:
            return {"installed": True, "changed": False, "source": journal["source"]}
        raise ConflictError("installed OmaSheets files changed; uninstall or resolve them before reinstalling")
    report = dependency_report()
    if check_dependencies and not report["ready"]:
        missing = ", ".join(check["name"] for check in report["checks"] if not check["ok"])
        raise RuntimeError(f"missing dependencies: {missing}\nInstall them explicitly with: {report['install_command']}")
    for target in (paths.app, paths.launcher, paths.codex_plugin):
        if target.exists():
            raise ConflictError(f"refusing to overwrite unowned installation target: {target}")

    identity = source_identity(source_root)
    paths.app.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=".app.", dir=paths.app.parent))
    marketplace_before = paths.codex_marketplace.read_bytes() if paths.codex_marketplace.is_file() else None
    marketplace_after = _marketplace_after(marketplace_before)
    try:
        shutil.copytree(source_root / "src/omasheets", stage / "lib/omasheets")
        build = paths.build
        if build.exists():
            shutil.rmtree(build)
        runner([
            "cmake", "-S", str(source_root / "native/libreofficekit"), "-B", str(build),
            "-DCMAKE_BUILD_TYPE=Release", f"-DCMAKE_INSTALL_PREFIX={stage}",
            f"-DOMASHEETS_SOURCE_SHA256={identity['sha256']}",
            f"-DOMASHEETS_SOURCE_COMMIT={identity['commit']}",
        ], check=True)
        runner(["cmake", "--build", str(build), "--parallel", "2"], check=True)
        runner(["cmake", "--install", str(build)], check=True)
        provenance = {
            "schema": 1, "version": __version__, "source": identity,
            "build_contract": "native/libreofficekit/CMakeLists.txt",
        }
        _write_bytes(stage / "provenance.json", (json.dumps(provenance, indent=2, sort_keys=True) + "\n").encode())
        os.replace(stage, paths.app)
        _write_bytes(paths.launcher, _launcher(paths.app), 0o755)
        paths.codex_plugin.parent.mkdir(parents=True, exist_ok=True)
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
            "codex_plugin_sha256": _tree_sha(paths.codex_plugin),
            "marketplace_before": base64.b64encode(marketplace_before).decode() if marketplace_before is not None else None,
            "marketplace_after_sha256": _sha_bytes(marketplace_after),
        }
        write_json_atomic(paths.journal, journal)
        return {"installed": True, "changed": True, "source": identity, "launcher": str(paths.launcher)}
    except Exception:
        if stage.exists():
            shutil.rmtree(stage)
        for target in (paths.app, paths.codex_plugin):
            if target.exists():
                shutil.rmtree(target)
        if paths.launcher.exists():
            paths.launcher.unlink()
        if marketplace_before is None:
            paths.codex_marketplace.unlink(missing_ok=True)
        else:
            _write_bytes(paths.codex_marketplace, marketplace_before)
        try:
            uninstall_integration(paths.integration)
        except Exception:
            pass
        raise


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
    if not paths.journal.is_file():
        return {"installed": False, "changed": False, "conflicts": []}
    journal = read_json(paths.journal)
    conflicts: list[str] = []
    try:
        uninstall_integration(paths.integration)
    except ConflictError as error:
        conflicts.append(str(error))
    _remove_marketplace_entry(paths.codex_marketplace, journal)
    owned = (
        (paths.launcher, journal["launcher_sha256"], _sha_file),
        (paths.codex_plugin, journal["codex_plugin_sha256"], _tree_sha),
        (paths.app, journal["app_sha256"], _tree_sha),
    )
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
