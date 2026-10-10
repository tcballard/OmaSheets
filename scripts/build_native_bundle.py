#!/usr/bin/env python3
"""Build the native OmaSheets executables and package a verified release bundle."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

from omasheets import __version__  # noqa: E402
from omasheets.installation import source_identity  # noqa: E402
from omasheets.native_bundle import NATIVE_EXECUTABLES, asset_name, normalized_architecture, platform_id  # noqa: E402

_OFFICE_LIBRARY = re.compile(r"libreoffice|lib(?:mergedlo|soffice|uno[_-])", re.IGNORECASE)


def verify_owned_runtime(directory: Path, executables: tuple[str, ...] = NATIVE_EXECUTABLES) -> None:
    """Reject foreign office libraries or unresolved dependencies in built ELF files."""
    for name in executables:
        path = directory / name
        with path.open("rb") as handle:
            if handle.read(4) != b"\x7fELF":
                raise RuntimeError(f"native release executable is not ELF: {name}")
        completed = subprocess.run(
            ["ldd", str(path)], text=True, capture_output=True, check=False, timeout=15,
        )
        detail = completed.stdout + completed.stderr
        if _OFFICE_LIBRARY.search(detail):
            raise RuntimeError(f"native executable depends on LibreOffice/UNO: {name}")
        static = "statically linked" in detail or "not a dynamic executable" in detail
        if "not found" in detail or (completed.returncode != 0 and not static):
            raise RuntimeError(f"native executable has unresolved runtime libraries: {name}: {detail.strip()}")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_date_epoch() -> int:
    """Return the reproducible timestamp: SOURCE_DATE_EPOCH or the HEAD commit time."""
    value = os.environ.get("SOURCE_DATE_EPOCH")
    if value:
        return int(value)
    completed = subprocess.run(
        ["git", "-C", str(ROOT), "log", "-1", "--format=%ct"], text=True, capture_output=True, check=True,
    )
    return int(completed.stdout.strip())


def write_reproducible_archive(archive: Path, members: list[tuple[str, Path, int]], epoch: int) -> None:
    """Write ``members`` as a gzip tar whose bytes depend only on their contents.

    Member order, ownership, permissions and timestamps are fixed, and the gzip
    header carries no name and a fixed mtime, so two builds of identical files
    produce identical archives and one SHA-256 can be compared against a
    rebuild before the bundle is signed.
    """
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.PAX_FORMAT) as bundle:
        for arcname, source, mode in sorted(members):
            data = source.read_bytes()
            info = tarfile.TarInfo(arcname)
            info.size = len(data)
            info.mode = mode
            info.mtime = epoch
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            bundle.addfile(info, io.BytesIO(data))
    with archive.open("wb") as output, gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as compressed:
        compressed.write(buffer.getvalue())


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--build-dir", type=Path)
    parser.add_argument(
        "--build-inputs", type=Path,
        help="JSON record from scripts/build_inputs.py to embed in the bundle manifest",
    )
    arguments = parser.parse_args(argv)
    build_inputs = None
    if arguments.build_inputs is not None:
        build_inputs = json.loads(arguments.build_inputs.read_text())
        if not isinstance(build_inputs, dict) or build_inputs.get("schema") != 1:
            parser.error("--build-inputs must be a schema 1 build record")
    if (platform_id(), normalized_architecture()) != ("linux", "x86_64"):
        parser.error(f"v{__version__} release bundles are supported only on linux/x86_64")

    identity = source_identity(ROOT)
    arguments.output.mkdir(parents=True, exist_ok=True)
    temporary_build = None
    build = arguments.build_dir
    if build is None:
        temporary_build = tempfile.TemporaryDirectory(prefix="omasheets-native-build-")
        build = Path(temporary_build.name)
    stage_context = tempfile.TemporaryDirectory(prefix="omasheets-native-stage-")
    stage = Path(stage_context.name)
    try:
        rust_target = build / "rust-target"
        rust_environment = os.environ.copy()
        rust_environment["CARGO_TARGET_DIR"] = str(rust_target)
        rust_environment["CARGO_PROFILE_RELEASE_STRIP"] = "symbols"
        rust_environment["OMASHEETS_SOURCE_SHA256"] = identity["sha256"]
        rust_environment["OMASHEETS_SOURCE_COMMIT"] = identity["commit"]
        remaps = (
            f"--remap-path-prefix={ROOT}=/omasheets "
            f"--remap-path-prefix={build}=/omasheets-build"
        )
        rust_environment["RUSTFLAGS"] = " ".join(
            part for part in (rust_environment.get("RUSTFLAGS", ""), remaps) if part
        )
        subprocess.run(
            [
                "cargo", "build", "--locked", "--release",
                "-p", "omasheets-service", "-p", "omasheets-kit",
            ],
            cwd=ROOT,
            env=rust_environment,
            check=True,
        )
        grid_target = build / "qt-grid-target"
        subprocess.run(
            ["cargo", "build", "--locked", "--release", "--manifest-path", str(ROOT / "native/setup/Cargo.toml")],
            cwd=ROOT, env=rust_environment, check=True,
        )
        grid_environment = rust_environment.copy()
        grid_environment["CARGO_TARGET_DIR"] = str(grid_target)
        subprocess.run(
            [
                "cargo", "build", "--locked", "--release", "--manifest-path",
                str(ROOT / "spikes/qt-grid/Cargo.toml"),
            ],
            cwd=ROOT,
            env=grid_environment,
            check=True,
        )
        (stage / "bin").mkdir()
        shutil.copy2(rust_target / "release/omasheets-service", stage / "bin/omasheets-service")
        shutil.copy2(rust_target / "release/omasheets-kit", stage / "bin/omasheets-kit")
        shutil.copy2(grid_target / "release/omasheets-grid", stage / "bin/omasheets-grid")
        shutil.copy2(rust_target / "release/omasheets-setup", stage / "bin/omasheets-setup")
        verify_owned_runtime(stage / "bin")
        files = {f"bin/{name}": sha256(stage / "bin" / name) for name in NATIVE_EXECUTABLES}
        manifest = {
            "schema": 1,
            "version": __version__,
            "platform": platform_id(),
            "architecture": normalized_architecture(),
            "source": identity,
            "build_contract": "Cargo.toml",
            "rust_build_contract": [
                "Cargo.lock", "crates/omasheets-service/Cargo.toml",
                "crates/omasheets-kit/Cargo.toml",
                "native/setup/Cargo.toml", "native/setup/Cargo.lock",
            ],
            "qt_build_contract": [
                "spikes/qt-grid/Cargo.lock",
                "spikes/qt-grid/Cargo.toml",
                "spikes/qt-grid/build.rs",
            ],
            "files": files,
        }
        if build_inputs is not None:
            manifest["build"] = build_inputs
        (stage / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        name = asset_name(__version__)
        archive = arguments.output / name
        write_reproducible_archive(
            archive,
            [("manifest.json", stage / "manifest.json", 0o644)]
            + [(f"bin/{executable}", stage / "bin" / executable, 0o755) for executable in NATIVE_EXECUTABLES],
            source_date_epoch(),
        )
        digest = sha256(archive)
        (arguments.output / f"{name}.sha256").write_text(f"{digest}  {name}\n")
        setup_name = "OmaSheets-Setup-linux-x86_64.tar.gz"
        setup_archive = arguments.output / setup_name
        write_reproducible_archive(setup_archive, [
            ("OmaSheets-Setup/omasheets-setup", stage / "bin/omasheets-setup", 0o755),
            ("OmaSheets-Setup/START-HERE.txt", ROOT / "native/setup/START-HERE.txt", 0o644),
        ], source_date_epoch())
        (arguments.output / f"{setup_name}.sha256").write_text(f"{sha256(setup_archive)}  {setup_name}\n")
        print(json.dumps({"archive": str(archive), "sha256": digest, "source": identity}, sort_keys=True))
        return 0
    finally:
        stage_context.cleanup()
        if temporary_build is not None:
            temporary_build.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
