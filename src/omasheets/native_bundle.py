"""Download and verify release-built OmaSheets native executables."""

from __future__ import annotations

import hashlib
import json
import os
import platform
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
from typing import Any
from urllib.request import Request, urlopen

from .release_signing import PublicKey, SignatureError, load_public_key, verify_file

REPOSITORY = "tcballard/OmaSheets"
MAX_BUNDLE_BYTES = 64 * 1024 * 1024
MAX_SIDECAR_BYTES = 8 * 1024
NATIVE_EXECUTABLES = ("omasheets-window", "omasheets-lok-render", "omasheets-service")
RELEASE_SIGNING_KEY = Path("release/signing-key.pub")
_SHA256 = re.compile(r"^[0-9a-f]{64}$")


def require_exact_version_tag(source_root: Path, version: str) -> str:
    """Require checkout HEAD to be the commit named by ``v<version>``.

    Automatic downloads are safe only at a published release boundary. An
    explicit bundle remains available to CI and developers because its source
    identity is verified independently by :func:`install_native_bundle`.
    """

    expected_tag = f"v{version}"

    def resolve(revision: str) -> str | None:
        try:
            completed = subprocess.run(
                [
                    "git", "-C", str(source_root), "rev-parse", "--verify", "--quiet",
                    f"{revision}^{{commit}}",
                ],
                text=True,
                capture_output=True,
                check=False,
                timeout=5,
            )
        except (OSError, subprocess.TimeoutExpired):
            return None
        value = completed.stdout.strip()
        return value if completed.returncode == 0 and value else None

    head = resolve("HEAD")
    tagged = resolve(f"refs/tags/{expected_tag}")
    if head is None or tagged != head:
        raise RuntimeError(
            "automatic native bundle download requires checkout HEAD to be exactly "
            f"tagged {expected_tag}; check out that published release tag or provide "
            "a matching explicit bundle with OMASHEETS_NATIVE_BUNDLE_PATH"
        )
    return head


def normalized_architecture(machine: str | None = None) -> str:
    value = (machine or platform.machine()).lower()
    aliases = {"amd64": "x86_64", "x64": "x86_64", "arm64": "aarch64"}
    return aliases.get(value, value)


def platform_id() -> str:
    return "linux" if sys.platform.startswith("linux") else sys.platform


def asset_name(version: str, *, system: str | None = None, architecture: str | None = None) -> str:
    return f"omasheets-native-{version}-{system or platform_id()}-{architecture or normalized_architecture()}.tar.gz"


def _sha_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_release_public_key(source_root: Path) -> PublicKey:
    """Load the release signing key pinned in the validated plugin checkout.

    The key is part of the tracked source identity, so it is as trusted as the
    bootstrap code itself and independent of the release channel. A checkout
    without a pinned key cannot download anything: the bootstrap fails closed.
    """
    path = source_root / RELEASE_SIGNING_KEY
    try:
        return load_public_key(path)
    except SignatureError as error:
        raise RuntimeError(
            f"automatic native bundle download requires the pinned release signing key: {error}"
        ) from error


def _download(url: str, destination: Path, *, limit: int = MAX_BUNDLE_BYTES) -> None:
    request = Request(url, headers={"User-Agent": "OmaSheets installer"})
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{destination.name}.", dir=destination.parent)
    total = 0
    try:
        with os.fdopen(descriptor, "wb") as output, urlopen(request, timeout=60) as response:
            while chunk := response.read(1024 * 1024):
                total += len(chunk)
                if total > limit:
                    raise RuntimeError(f"{destination.name} exceeds the {limit} byte download limit")
                output.write(chunk)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary_name, destination)
    finally:
        Path(temporary_name).unlink(missing_ok=True)


def download_native_bundle(
    version: str,
    destination: Path,
    *,
    source_root: Path,
    public_key: PublicKey | None = None,
) -> Path:
    """Download the release bundle and verify it before returning its path.

    Verification order is deliberate: the release boundary and the pinned
    signing key are checked before any network access; the detached minisign
    signature is verified against the pinned key before the release checksum
    is consulted; the archive is deleted on any failure. Nothing inside the
    bundle is opened, let alone executed, until this function returns.
    """
    require_exact_version_tag(source_root, version)
    public_key = public_key or load_release_public_key(source_root)
    system = platform_id()
    architecture = normalized_architecture()
    if (system, architecture) != ("linux", "x86_64"):
        raise RuntimeError(f"OmaSheets v{version} has no native bundle for {system}/{architecture}")
    destination.mkdir(parents=True, exist_ok=True)
    name = asset_name(version, system=system, architecture=architecture)
    base = f"https://github.com/{REPOSITORY}/releases/download/v{version}"
    archive = destination / name
    signature = destination / f"{name}.minisig"
    checksum = destination / f"{name}.sha256"
    _download(f"{base}/{name}", archive)
    try:
        _download(f"{base}/{name}.minisig", signature, limit=MAX_SIDECAR_BYTES)
        try:
            verify_file(
                archive, signature.read_text(encoding="utf-8"), public_key, expected_name=name,
            )
        except (SignatureError, UnicodeDecodeError) as error:
            raise RuntimeError(f"native bundle signature verification failed: {error}") from error
        _download(f"{base}/{name}.sha256", checksum, limit=MAX_SIDECAR_BYTES)
        fields = checksum.read_text().strip().split()
        if len(fields) != 2 or fields[1].lstrip("*") != name or not _SHA256.fullmatch(fields[0]):
            raise RuntimeError("native bundle checksum file is malformed")
        if _sha_file(archive) != fields[0]:
            raise RuntimeError("native bundle checksum does not match the release")
        return archive
    except Exception:
        archive.unlink(missing_ok=True)
        raise
    finally:
        signature.unlink(missing_ok=True)
        checksum.unlink(missing_ok=True)


def install_native_bundle(
    archive: Path,
    destination: Path,
    *,
    version: str,
    source: dict[str, str],
) -> dict[str, Any]:
    """Validate a bundle completely before copying its allow-listed files."""
    if archive.stat().st_size > MAX_BUNDLE_BYTES:
        raise RuntimeError("native bundle exceeds the 64 MiB archive limit")
    allowed = {"manifest.json", *(f"bin/{name}" for name in NATIVE_EXECUTABLES)}
    with tarfile.open(archive, "r:gz") as bundle:
        members = bundle.getmembers()
        names = [member.name for member in members]
        if len(names) != len(set(names)) or set(names) != allowed:
            raise RuntimeError("native bundle contains an unexpected file set")
        if any(not member.isfile() for member in members):
            raise RuntimeError("native bundle may contain regular files only")
        if bundle.getmember("manifest.json").size > 64 * 1024:
            raise RuntimeError("native bundle manifest exceeds the 64 KiB limit")
        if any(member.size > 32 * 1024 * 1024 for member in members if member.name.startswith("bin/")):
            raise RuntimeError("native bundle executable exceeds the 32 MiB limit")
        if sum(member.size for member in members) > MAX_BUNDLE_BYTES:
            raise RuntimeError("native bundle expands beyond the 64 MiB limit")
        manifest_member = bundle.getmember("manifest.json")
        manifest_file = bundle.extractfile(manifest_member)
        if manifest_file is None:
            raise RuntimeError("native bundle manifest cannot be read")
        manifest = json.load(manifest_file)
        expected_header = {
            "schema": 1,
            "version": version,
            "platform": platform_id(),
            "architecture": normalized_architecture(),
            "source": source,
        }
        for key, expected in expected_header.items():
            if manifest.get(key) != expected:
                raise RuntimeError(f"native bundle {key} does not match this plugin checkout")
        files = manifest.get("files")
        expected_paths = {f"bin/{name}" for name in NATIVE_EXECUTABLES}
        if not isinstance(files, dict) or set(files) != expected_paths:
            raise RuntimeError("native bundle manifest has an unexpected executable set")
        payloads: dict[str, bytes] = {}
        for relative in sorted(expected_paths):
            member_file = bundle.extractfile(bundle.getmember(relative))
            if member_file is None:
                raise RuntimeError(f"native bundle file cannot be read: {relative}")
            data = member_file.read()
            if hashlib.sha256(data).hexdigest() != files[relative]:
                raise RuntimeError(f"native bundle file checksum failed: {relative}")
            payloads[relative] = data

    for relative, data in payloads.items():
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        target.chmod(0o755)
        completed = subprocess.run(
            [target, "--provenance"], text=True, capture_output=True, check=True, timeout=5,
        )
        provenance = json.loads(completed.stdout)
        if provenance.get("source_commit") != source["commit"]:
            raise RuntimeError(f"native executable commit provenance failed: {relative}")
        if provenance.get("source_sha256") != source["sha256"]:
            raise RuntimeError(f"native executable source provenance failed: {relative}")
    (destination / "native-bundle.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    return manifest
