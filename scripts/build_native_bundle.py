#!/usr/bin/env python3
"""Build the native OmaSheets executables and package a verified release bundle."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

from omasheets import __version__  # noqa: E402
from omasheets.installation import source_identity  # noqa: E402
from omasheets.native_bundle import NATIVE_EXECUTABLES, asset_name, normalized_architecture, platform_id  # noqa: E402


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--build-dir", type=Path)
    arguments = parser.parse_args(argv)
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
        subprocess.run([
            "cmake", "-S", str(ROOT / "native/libreofficekit"), "-B", str(build),
            "-DCMAKE_BUILD_TYPE=Release", f"-DCMAKE_INSTALL_PREFIX={stage}",
            f"-DOMASHEETS_SOURCE_SHA256={identity['sha256']}",
            f"-DOMASHEETS_SOURCE_COMMIT={identity['commit']}",
        ], check=True)
        subprocess.run(["cmake", "--build", str(build), "--parallel", "2"], check=True)
        subprocess.run(["cmake", "--install", str(build)], check=True)
        files = {f"bin/{name}": sha256(stage / "bin" / name) for name in NATIVE_EXECUTABLES}
        manifest = {
            "schema": 1,
            "version": __version__,
            "platform": platform_id(),
            "architecture": normalized_architecture(),
            "source": identity,
            "build_contract": "native/libreofficekit/CMakeLists.txt",
            "files": files,
        }
        (stage / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        name = asset_name(__version__)
        archive = arguments.output / name
        with tarfile.open(archive, "w:gz", format=tarfile.PAX_FORMAT) as bundle:
            bundle.add(stage / "manifest.json", arcname="manifest.json")
            for executable in NATIVE_EXECUTABLES:
                bundle.add(stage / "bin" / executable, arcname=f"bin/{executable}")
        digest = sha256(archive)
        (arguments.output / f"{name}.sha256").write_text(f"{digest}  {name}\n")
        print(json.dumps({"archive": str(archive), "sha256": digest, "source": identity}, sort_keys=True))
        return 0
    finally:
        stage_context.cleanup()
        if temporary_build is not None:
            temporary_build.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
