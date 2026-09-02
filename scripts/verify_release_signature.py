#!/usr/bin/env python3
"""Verify a release bundle against its minisign signature and the pinned key.

This is the same check the user-local bootstrap performs before it opens a
downloaded bundle, exposed so maintainers can confirm a signature before
uploading it and users can audit a release by hand:

    python scripts/verify_release_signature.py dist/omasheets-native-X-linux-x86_64.tar.gz
"""

from __future__ import annotations

import argparse
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

from omasheets.native_bundle import RELEASE_SIGNING_KEY  # noqa: E402
from omasheets.release_signing import SignatureError, load_public_key, verify_file  # noqa: E402


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--signature", type=Path, help="defaults to ARCHIVE.minisig")
    parser.add_argument("--key", type=Path, default=ROOT / RELEASE_SIGNING_KEY)
    arguments = parser.parse_args(argv)
    signature = arguments.signature or arguments.archive.with_name(arguments.archive.name + ".minisig")
    try:
        key = load_public_key(arguments.key)
        verified = verify_file(
            arguments.archive, signature.read_text(encoding="utf-8"), key,
            expected_name=arguments.archive.name,
        )
    except (SignatureError, OSError, UnicodeDecodeError) as error:
        print(f"signature verification failed: {error}", file=sys.stderr)
        return 1
    print(f"signature ok: {arguments.archive.name} signed by key {key.key_id_hex}")
    print(f"trusted comment: {verified.trusted_comment}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
