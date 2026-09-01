#!/usr/bin/env python3
"""Freeze a deterministic, de-duplicated sample of a local corpus as a manifest.

Every regular ``.xlsx`` file below ROOT is hashed. Byte-identical workbooks
collapse to the lexicographically first relative path, the unique digests are
sorted, and the first COUNT become the manifest. SHA-256 order is effectively
uniform over unique workbooks, so the sample is reproducible from the same
extracted archive without a seed and without any local path entering Git.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys

MAX_COUNT = 1_000
MAX_FILES = 100_000
MAX_INPUT_BYTES = 512 * 1024 * 1024
MAX_DEPTH = 64


class SampleError(Exception):
    """A bounded, user-facing failure."""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def walk_workbooks(root: Path) -> list[Path]:
    workbooks: list[Path] = []
    stack = [root]
    while stack:
        current = stack.pop()
        for entry in sorted(current.iterdir(), key=lambda item: item.name):
            if entry.is_symlink():
                continue
            if entry.is_dir():
                if len(entry.relative_to(root).parts) >= MAX_DEPTH:
                    raise SampleError("corpus tree is deeper than the 64-level bound")
                stack.append(entry)
            elif entry.is_file() and entry.suffix == ".xlsx":
                if entry.stat().st_size > MAX_INPUT_BYTES:
                    raise SampleError("corpus input exceeds the 512 MiB limit")
                workbooks.append(entry)
                if len(workbooks) > MAX_FILES:
                    raise SampleError(f"corpus exceeds the {MAX_FILES}-file bound")
    return workbooks


def sample(root: Path, count: int, prefix: str) -> tuple[list[dict], dict]:
    if not root.is_dir():
        raise SampleError("corpus root must be a directory")
    if not 1 <= count <= MAX_COUNT:
        raise SampleError(f"count must be between 1 and {MAX_COUNT}")
    if not prefix or len(prefix) > 32 or not all(
        character.isalnum() or character in "._-" for character in prefix
    ):
        raise SampleError("prefix must be 1-32 characters of [A-Za-z0-9._-]")
    workbooks = walk_workbooks(root)
    if not workbooks:
        raise SampleError("corpus root contains no .xlsx files")
    by_digest: dict[str, str] = {}
    duplicates = 0
    for workbook in workbooks:
        relative = workbook.relative_to(root).as_posix()
        digest = sha256_file(workbook)
        previous = by_digest.get(digest)
        if previous is None or relative < previous:
            if previous is not None:
                duplicates += 1
            by_digest[digest] = relative
        else:
            duplicates += 1
    chosen = sorted(by_digest)[:count]
    width = len(str(MAX_COUNT))
    entries = [
        {"id": f"{prefix}-{index:0{width}d}", "path": by_digest[digest], "sha256": digest}
        for index, digest in enumerate(chosen, start=1)
    ]
    report = {
        "schema": 1,
        "files_seen": len(workbooks),
        "unique_workbooks": len(by_digest),
        "duplicate_files": duplicates,
        "sampled": len(entries),
        "method": "sorted unique SHA-256, first N",
    }
    return entries, report


def write_manifest(entries: list[dict], output: Path) -> None:
    payload = "".join(
        json.dumps(entry, sort_keys=False, separators=(",", ":")) + "\n" for entry in entries
    )
    if len(payload.encode("utf-8")) > 1024 * 1024:
        raise SampleError("manifest exceeds the 1 MiB limit")
    output.parent.mkdir(parents=True, exist_ok=True)
    try:
        with output.open("x", encoding="utf-8") as handle:
            handle.write(payload)
    except FileExistsError as exc:
        raise SampleError(f"refusing to replace existing manifest {output.name}") from exc


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path, help="extracted corpus root (never in Git)")
    parser.add_argument("output", type=Path, help="manifest JSONL to create")
    parser.add_argument("--count", type=int, default=MAX_COUNT)
    parser.add_argument("--prefix", default="wb")
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
    try:
        entries, report = sample(arguments.root, arguments.count, arguments.prefix)
        write_manifest(entries, arguments.output)
    except SampleError as exc:
        print(f"sample_corpus: {exc}", file=sys.stderr)
        return 1
    print(json.dumps(report, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
