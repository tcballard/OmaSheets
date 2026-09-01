#!/usr/bin/env python3
"""Fetch one corpus archive by digest and extract only its workbooks.

The source register is a small JSON document kept beside the frozen manifest
(see ``corpus/README.md``). It pins the archive URL and SHA-256, so a clean
machine either reproduces the same bytes or stops. Zip archives are read with
the standard library; ``.7z`` archives need the ``7z`` command. No workbook
bytes, cell contents or local paths are ever written back into the repository.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess
import sys
import urllib.request
import zipfile

MAX_ARCHIVE_BYTES = 4 * 1024 * 1024 * 1024
MAX_MEMBERS = 50_000
MAX_MEMBER_BYTES = 512 * 1024 * 1024
MAX_TOTAL_BYTES = 16 * 1024 * 1024 * 1024
MAX_REGISTER_BYTES = 64 * 1024
REQUIRED_FIELDS = (
    "schema",
    "name",
    "url",
    "archive_sha256",
    "license",
    "retrieved",
    "sampling",
)
ALLOWED_SCHEMES = ("https", "file")
ARCHIVE_FORMATS = ("zip", "7z")
SEVEN_ZIP_COMMANDS = ("7z", "7za", "7zr")


class FetchError(Exception):
    """A bounded, user-facing failure."""


def load_register(path: Path) -> dict:
    if not path.is_file() or path.stat().st_size > MAX_REGISTER_BYTES:
        raise FetchError("source register must be a regular file no larger than 64 KiB")
    try:
        register = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise FetchError(f"source register is not valid JSON: {exc}") from exc
    if not isinstance(register, dict):
        raise FetchError("source register must be a JSON object")
    missing = [field for field in REQUIRED_FIELDS if field not in register]
    if missing:
        raise FetchError(f"source register is missing {', '.join(missing)}")
    if register["schema"] != 1:
        raise FetchError("unsupported source register schema")
    name = register["name"]
    if (
        not isinstance(name, str)
        or not name
        or len(name) > 64
        or not all(character.isalnum() or character in "._-" for character in name)
    ):
        raise FetchError("source name must be 1-64 characters of [A-Za-z0-9._-]")
    digest = register["archive_sha256"]
    if (
        not isinstance(digest, str)
        or len(digest) != 64
        or any(character not in "0123456789abcdef" for character in digest)
    ):
        raise FetchError("archive_sha256 must be 64 lowercase hex characters")
    url = register["url"]
    if not isinstance(url, str) or url.split(":", 1)[0] not in ALLOWED_SCHEMES:
        raise FetchError("source url must use https or file")
    archive_format = register.get("archive_format", "zip")
    if archive_format not in ARCHIVE_FORMATS:
        raise FetchError("archive_format must be zip or 7z")
    register["archive_format"] = archive_format
    upstream_md5 = register.get("upstream_md5")
    if upstream_md5 is not None and (
        not isinstance(upstream_md5, str)
        or len(upstream_md5) != 32
        or any(character not in "0123456789abcdef" for character in upstream_md5)
    ):
        raise FetchError("upstream_md5 must be 32 lowercase hex characters when present")
    return register


def sha256_file(path: Path) -> str:
    return _digest(path, hashlib.sha256())


def md5_file(path: Path) -> str:
    """Only for cross-checking a digest the upstream host publishes."""
    return _digest(path, hashlib.md5())  # noqa: S324 - provenance cross-check only


def _digest(path: Path, digest) -> str:
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download(url: str, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        handle = destination.open("xb")
    except FileExistsError as exc:
        raise FetchError(f"refusing to replace existing archive {destination.name}") from exc
    written = 0
    try:
        with handle, urllib.request.urlopen(url) as response:  # noqa: S310 - scheme checked
            for chunk in iter(lambda: response.read(1024 * 1024), b""):
                written += len(chunk)
                if written > MAX_ARCHIVE_BYTES:
                    raise FetchError("archive exceeds the 4 GiB limit")
                handle.write(chunk)
    except Exception:
        destination.unlink(missing_ok=True)
        raise


def safe_member_path(name: str) -> PurePosixPath | None:
    path = PurePosixPath(name.replace("\\", "/"))
    if path.is_absolute() or any(part in ("", ".", "..") for part in path.parts):
        return None
    if path.suffix.lower() != ".xlsx":
        return None
    return path.with_suffix(".xlsx")


def extract_workbooks(archive: Path, destination: Path, archive_format: str = "zip") -> dict:
    if destination.exists():
        raise FetchError(f"refusing to extract into existing directory {destination.name}")
    if archive_format == "7z":
        return extract_7z_workbooks(archive, destination)
    extracted = 0
    skipped = 0
    total = 0
    with zipfile.ZipFile(archive) as bundle:
        members = bundle.infolist()
        if len(members) > MAX_MEMBERS:
            raise FetchError(f"archive lists more than {MAX_MEMBERS} members")
        destination.mkdir(parents=True)
        for member in members:
            target = safe_member_path(member.filename)
            is_symlink = stat.S_ISLNK(member.external_attr >> 16)
            if member.is_dir() or target is None or is_symlink:
                skipped += 1
                continue
            if member.file_size > MAX_MEMBER_BYTES:
                raise FetchError("archive member exceeds the 512 MiB limit")
            total += member.file_size
            if total > MAX_TOTAL_BYTES:
                raise FetchError("archive workbooks exceed the 16 GiB limit")
            output = destination / Path(*target.parts)
            output.parent.mkdir(parents=True, exist_ok=True)
            with bundle.open(member) as source, output.open("xb") as sink:
                copied = 0
                for chunk in iter(lambda: source.read(1024 * 1024), b""):
                    copied += len(chunk)
                    if copied > MAX_MEMBER_BYTES:
                        raise FetchError("archive member exceeds the 512 MiB limit")
                    sink.write(chunk)
            output.chmod(0o600)
            extracted += 1
    return {"workbooks_extracted": extracted, "members_skipped": skipped}


def seven_zip_command() -> str:
    for candidate in SEVEN_ZIP_COMMANDS:
        found = shutil.which(candidate)
        if found:
            return found
    raise FetchError("extracting a .7z archive needs the 7z, 7za or 7zr command")


def extract_7z_workbooks(archive: Path, destination: Path) -> dict:
    """Unpack through 7-Zip into a staging directory, then keep only safe workbooks.

    7-Zip decides the on-disk layout, so the staging tree is walked afterwards
    and only regular, non-symlink ``.xlsx`` files below it are moved into the
    destination with the extension normalised to lowercase.
    """
    command = seven_zip_command()
    staging = destination.with_name(destination.name + ".staging")
    if staging.exists():
        raise FetchError(f"refusing to reuse existing staging directory {staging.name}")
    staging.mkdir(parents=True)
    try:
        completed = subprocess.run(  # noqa: S603 - fixed argv, no shell
            [command, "x", "-y", "-bd", "-bso0", "-bsp0", f"-o{staging}", "--", str(archive)],
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        if completed.returncode != 0:
            detail = completed.stderr.strip().splitlines()[-1:] or ["no detail"]
            raise FetchError(f"7-Zip extraction failed: {detail[0][:200]}")
        extracted = 0
        skipped = 0
        total = 0
        destination.mkdir(parents=True)
        for path in sorted(staging.rglob("*")):
            if path.is_symlink() or not path.is_file():
                if path.is_symlink():
                    skipped += 1
                continue
            relative = path.relative_to(staging)
            if len(relative.parts) > 64 or relative.suffix.lower() != ".xlsx":
                skipped += 1
                continue
            if extracted >= MAX_MEMBERS:
                raise FetchError(f"archive holds more than {MAX_MEMBERS} workbooks")
            size = path.stat().st_size
            if size > MAX_MEMBER_BYTES:
                raise FetchError("archive member exceeds the 512 MiB limit")
            total += size
            if total > MAX_TOTAL_BYTES:
                raise FetchError("archive workbooks exceed the 16 GiB limit")
            output = destination / relative.with_suffix(".xlsx")
            output.parent.mkdir(parents=True, exist_ok=True)
            if output.exists():
                raise FetchError(f"archive names {relative.as_posix()} twice after normalisation")
            shutil.move(str(path), str(output))
            output.chmod(0o600)
            extracted += 1
    finally:
        shutil.rmtree(staging, ignore_errors=True)
    return {"workbooks_extracted": extracted, "members_skipped": skipped}


def fetch(register_path: Path, destination: Path) -> dict:
    register = load_register(register_path)
    archive = destination / "archives" / f"{register['name']}.{register['archive_format']}"
    download(register["url"], archive)
    observed = sha256_file(archive)
    if observed != register["archive_sha256"]:
        archive.unlink()
        raise FetchError(
            "archive digest drift: expected "
            f"{register['archive_sha256']} but downloaded {observed}"
        )
    upstream_md5 = register.get("upstream_md5")
    if upstream_md5 is not None and md5_file(archive) != upstream_md5:
        archive.unlink()
        raise FetchError("archive does not match the upstream-published MD5")
    result = extract_workbooks(archive, destination / register["name"], register["archive_format"])
    return {
        "schema": 1,
        "name": register["name"],
        "archive_format": register["archive_format"],
        "archive_sha256": observed,
        "archive_bytes": archive.stat().st_size,
        **result,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("register", type=Path, help="source register JSON")
    parser.add_argument("destination", type=Path, help="local corpus root (never in Git)")
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
    try:
        report = fetch(arguments.register, arguments.destination)
    except FetchError as exc:
        print(f"fetch_corpus: {exc}", file=sys.stderr)
        return 1
    print(json.dumps(report, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
