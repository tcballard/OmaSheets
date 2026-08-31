"""Isolated LibreOffice Calc execution boundary."""

from __future__ import annotations

import json
import os
import resource
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from .errors import ConflictError, EngineError
from .identity import identify_regular_file
from .paths import AppPaths
from .policy import conversion_destination
from .store import read_json, write_json_atomic


@dataclass(frozen=True, slots=True)
class CalcLimits:
    timeout_seconds: int = 90
    cpu_seconds: int = 60
    address_space_bytes: int = 2 * 1024 * 1024 * 1024
    output_bytes: int = 256 * 1024 * 1024
    open_files: int = 128


@dataclass(frozen=True, slots=True)
class CalcConfig:
    bwrap: Path = Path("/usr/bin/bwrap")
    python: Path = Path("/usr/bin/python")
    soffice: Path = Path("/usr/bin/soffice")
    worker: Path | None = None
    allow_unsafe_development_mode: bool = False


def _copy_no_clobber(source: Path, destination: Path) -> None:
    """Copy a regular artifact without replacing any existing path."""

    destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    descriptor = os.open(destination, flags, 0o600)
    try:
        with source.open("rb") as incoming, os.fdopen(descriptor, "wb", closefd=False) as outgoing:
            shutil.copyfileobj(incoming, outgoing, length=1024 * 1024)
            outgoing.flush()
            os.fsync(outgoing.fileno())
    except Exception:
        destination.unlink(missing_ok=True)
        raise
    finally:
        os.close(descriptor)


def _runtime_path_arguments(paths: tuple[Path, ...]) -> list[str]:
    """Recreate merged-/usr links or bind separate loader directories read-only."""

    arguments: list[str] = []
    for path in paths:
        if path.is_symlink():
            arguments.extend(["--symlink", os.readlink(path), str(path)])
        elif path.exists():
            arguments.extend(["--ro-bind", str(path), str(path)])
    return arguments


class CalcEngine:
    """Run one Calc job per process inside a networkless Bubblewrap sandbox."""

    def __init__(
        self,
        paths: AppPaths,
        config: CalcConfig | None = None,
        limits: CalcLimits | None = None,
        runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
    ):
        self.paths = paths
        self.config = config or CalcConfig()
        self.limits = limits or CalcLimits()
        self.runner = runner
        self.paths.ensure()

    @property
    def worker_path(self) -> Path:
        return self.config.worker or Path(__file__).with_name("calc_worker.py")

    def _sandbox_command(self, job: Path) -> list[str]:
        if not self.config.bwrap.is_file() and not self.config.allow_unsafe_development_mode:
            raise EngineError("Bubblewrap is required for Calc jobs")
        if not self.config.python.is_file():
            raise EngineError("system Python with LibreOffice UNO support is unavailable")
        if not self.config.soffice.is_file():
            raise EngineError("LibreOffice Calc is unavailable")
        if not self.worker_path.is_file():
            raise EngineError("Calc worker is unavailable")

        if self.config.allow_unsafe_development_mode and not self.config.bwrap.is_file():
            return [
                str(self.config.python),
                str(self.worker_path),
                str(job / "request.json"),
                str(job / "result.json"),
            ]

        command = [
            str(self.config.bwrap),
            "--die-with-parent",
            "--new-session",
            "--unshare-all",
            "--clearenv",
            "--ro-bind", "/usr", "/usr",
            "--dev", "/dev",
            "--proc", "/proc",
            "--tmpfs", "/tmp",
            "--dir", "/run",
        ]
        command.extend(_runtime_path_arguments(tuple(Path(item) for item in ("/bin", "/sbin", "/lib", "/lib64"))))
        if Path("/etc/fonts").exists():
            command.extend(["--ro-bind", "/etc/fonts", "/etc/fonts"])
        if Path("/etc/machine-id").exists():
            command.extend(["--ro-bind", "/etc/machine-id", "/etc/machine-id"])
        command.extend([
            "--bind", str(job), "/job",
            "--ro-bind", str(self.worker_path), "/omasheets-worker.py",
            "--chdir", "/job",
            "--setenv", "HOME", "/job/home",
            "--setenv", "XDG_RUNTIME_DIR", "/job/runtime",
            "--setenv", "PATH", "/usr/bin",
            "--setenv", "LANG", "C.UTF-8",
            "--setenv", "LC_ALL", "C.UTF-8",
            "--setenv", "PYTHONNOUSERSITE", "1",
            "--setenv", "PYTHONPATH", "/usr/lib/libreoffice/program",
            "--setenv", "SAL_USE_VCLPLUGIN", "gen",
            str(self.config.python),
            "/omasheets-worker.py",
            "/job/request.json",
            "/job/result.json",
        ])
        return command

    def _resource_limits(self) -> None:
        resource.setrlimit(resource.RLIMIT_CPU, (self.limits.cpu_seconds, self.limits.cpu_seconds))
        resource.setrlimit(
            resource.RLIMIT_AS,
            (self.limits.address_space_bytes, self.limits.address_space_bytes),
        )
        resource.setrlimit(resource.RLIMIT_FSIZE, (self.limits.output_bytes, self.limits.output_bytes))
        resource.setrlimit(resource.RLIMIT_NOFILE, (self.limits.open_files, self.limits.open_files))
        os.umask(0o077)

    def _execute(
        self,
        action: str,
        source: Path,
        arguments: dict[str, Any],
        artifacts: dict[str, Path] | None = None,
    ) -> dict[str, Any]:
        before = identify_regular_file(source)
        job_root = self.paths.cache / "jobs"
        job_root.mkdir(mode=0o700, parents=True, exist_ok=True)
        job = Path(tempfile.mkdtemp(prefix="calc-", dir=job_root))
        job.chmod(0o700)
        try:
            for name in ("input", "out", "home", "runtime", "profile"):
                (job / name).mkdir(mode=0o700)
            job_source = job / "input" / f"workbook{source.suffix.lower()}"
            _copy_no_clobber(source, job_source)
            after = identify_regular_file(source)
            if after != before:
                raise ConflictError("workbook changed while preparing the Calc job")
            request = {
                "action": action,
                "source": f"input/{job_source.name}",
                "arguments": arguments,
                "limits": {
                    "max_cells": 250_000,
                    "max_formulas": 20_000,
                    "max_sheets": 256,
                    "max_results": 200,
                },
                "soffice": str(self.config.soffice),
            }
            write_json_atomic(job / "request.json", request)
            command = self._sandbox_command(job)
            try:
                completed = self.runner(
                    command,
                    check=False,
                    capture_output=True,
                    text=True,
                    timeout=self.limits.timeout_seconds,
                    cwd=job,
                    env={},
                    preexec_fn=self._resource_limits,
                )
            except subprocess.TimeoutExpired as exc:
                raise EngineError("Calc job exceeded its time limit") from exc
            if completed.returncode != 0:
                raise EngineError("isolated Calc job failed")
            result_path = job / "result.json"
            if not result_path.is_file() or result_path.stat().st_size > 4 * 1024 * 1024:
                raise EngineError("Calc worker returned no bounded result")
            result = read_json(result_path)
            if result.get("ok") is not True:
                raise EngineError(str(result.get("error", "Calc worker failed"))[:512])

            for artifact_name, destination in (artifacts or {}).items():
                relative = result.get("artifacts", {}).get(artifact_name)
                if not isinstance(relative, str):
                    raise EngineError(f"Calc worker omitted {artifact_name}")
                candidate = (job / relative).resolve()
                output_root = (job / "out").resolve()
                if output_root not in candidate.parents or not candidate.is_file():
                    raise EngineError("Calc worker returned an invalid artifact path")
                if candidate.stat().st_size > self.limits.output_bytes:
                    raise EngineError("Calc artifact exceeded its size limit")
                _copy_no_clobber(candidate, destination)
            payload = result.get("result")
            if not isinstance(payload, dict):
                raise EngineError("Calc worker returned an invalid result")
            return payload
        finally:
            shutil.rmtree(job, ignore_errors=True)

    def describe(self, source: Path, *, include_formulas: bool) -> dict[str, Any]:
        return self._execute("describe", source, {"include_formulas": include_formulas})

    def read_range(self, source: Path, **arguments: Any) -> dict[str, Any]:
        return self._execute("read_range", source, arguments)

    def search(self, source: Path, **arguments: Any) -> dict[str, Any]:
        return self._execute("search", source, arguments)

    def trace(self, source: Path, **arguments: Any) -> dict[str, Any]:
        return self._execute("trace", source, arguments)

    def render(self, source: Path, *, output: Path) -> dict[str, Any]:
        return self._execute("render", source, {}, {"preview": output})

    def stage(
        self,
        source: Path,
        operations: list[dict[str, Any]],
        *,
        output: Path,
        preview: Path,
    ) -> dict[str, Any]:
        return self._execute(
            "stage",
            source,
            {"operations": operations},
            {"workbook": output, "preview": preview},
        )

    def convert_legacy(self, source: Path, *, destination: Path | None = None, preview: Path) -> dict[str, Any]:
        expected = conversion_destination(source)
        chosen = destination or expected
        if chosen != expected:
            raise ConflictError("legacy conversion destination must be the adjacent .xlsx path")
        return self._execute(
            "convert_xls",
            source,
            {},
            # Keep the public adjacent workbook as the final copy. If the
            # process is interrupted during artifact publication, at worst a
            # private preview remains; no half-completed public conversion is
            # presented as finished.
            {"preview": preview, "workbook": chosen},
        )
