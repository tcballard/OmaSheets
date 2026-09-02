"""Verify minisign-format Ed25519 release signatures with the standard library.

The release archive, its detached checksum and its self-reported provenance
all travel through the same GitHub release channel, so a compromised release
credential could replace them together. The signature verified here is rooted
in a public key committed to the plugin checkout that Omarchy validated, and
the private half never enters release automation. The implementation follows
RFC 8032 (Ed25519) and the minisign file formats, with no third-party
dependency, so a user-local bootstrap can verify a bundle before any byte of it
is executed.
"""

from __future__ import annotations

import base64
import hashlib
import os
import re
from dataclasses import dataclass
from pathlib import Path

KEY_ALGORITHM = b"Ed"
SIGNATURE_PREHASHED = b"ED"
SIGNATURE_LEGACY = b"Ed"
MAX_PUBLIC_KEY_BYTES = 4 * 1024
MAX_SIGNATURE_BYTES = 8 * 1024
MAX_TRUSTED_COMMENT_BYTES = 1024
_TRUSTED_COMMENT_PREFIX = "trusted comment: "
_UNTRUSTED_COMMENT_PREFIX = "untrusted comment: "
_PRINTABLE = re.compile(r"^[\x20-\x7e]*$")


class SignatureError(RuntimeError):
    """A signature, key or trusted comment failed verification."""


# --- Ed25519 (RFC 8032, section 5.1) -------------------------------------

_P = 2**255 - 19
_L = 2**252 + 27742317777372353535851937790883648493
_D = (-121665 * pow(121666, _P - 2, _P)) % _P
_SQRT_M1 = pow(2, (_P - 1) // 4, _P)
_IDENTITY = (0, 1, 1, 0)


def _recover_x(y: int, sign: int) -> int | None:
    if y >= _P:
        return None
    x2 = (y * y - 1) * pow(_D * y * y + 1, _P - 2, _P) % _P
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (_P + 3) // 8, _P)
    if (x * x - x2) % _P != 0:
        x = x * _SQRT_M1 % _P
    if (x * x - x2) % _P != 0:
        return None
    if (x & 1) != sign:
        x = _P - x
    return x


def _add(first: tuple[int, int, int, int], second: tuple[int, int, int, int]) -> tuple[int, int, int, int]:
    x1, y1, z1, t1 = first
    x2, y2, z2, t2 = second
    a = (y1 - x1) * (y2 - x2) % _P
    b = (y1 + x1) * (y2 + x2) % _P
    c = 2 * t1 * t2 * _D % _P
    d = 2 * z1 * z2 % _P
    e, f, g, h = b - a, d - c, d + c, b + a
    return (e * f % _P, g * h % _P, f * g % _P, e * h % _P)


def _double(point: tuple[int, int, int, int]) -> tuple[int, int, int, int]:
    x, y, z, _ = point
    a = x * x % _P
    b = y * y % _P
    c = 2 * z * z % _P
    h = a + b
    e = h - (x + y) * (x + y)
    g = a - b
    f = c + g
    return (e * f % _P, g * h % _P, f * g % _P, e * h % _P)


def _multiply(scalar: int, point: tuple[int, int, int, int]) -> tuple[int, int, int, int]:
    result = _IDENTITY
    addend = point
    while scalar > 0:
        if scalar & 1:
            result = _add(result, addend)
        addend = _double(addend)
        scalar >>= 1
    return result


def _compress(point: tuple[int, int, int, int]) -> bytes:
    x, y, z, _ = point
    inverse = pow(z, _P - 2, _P)
    x, y = x * inverse % _P, y * inverse % _P
    return (y | ((x & 1) << 255)).to_bytes(32, "little")


def _decompress(encoded: bytes) -> tuple[int, int, int, int] | None:
    if len(encoded) != 32:
        return None
    value = int.from_bytes(encoded, "little")
    sign = value >> 255
    y = value & ((1 << 255) - 1)
    x = _recover_x(y, sign)
    if x is None:
        return None
    return (x, y, 1, x * y % _P)


_BASE_Y = 4 * pow(5, _P - 2, _P) % _P
_BASE = (_recover_x(_BASE_Y, 0), _BASE_Y, 1, _recover_x(_BASE_Y, 0) * _BASE_Y % _P)


def _clamp(scalar_bytes: bytes) -> int:
    scalar = int.from_bytes(scalar_bytes, "little")
    scalar &= (1 << 254) - 8
    scalar |= 1 << 254
    return scalar


def _hash_scalar(*parts: bytes) -> int:
    return int.from_bytes(hashlib.sha512(b"".join(parts)).digest(), "little") % _L


def ed25519_public_key(seed: bytes) -> bytes:
    if len(seed) != 32:
        raise ValueError("Ed25519 seeds are 32 bytes")
    return _compress(_multiply(_clamp(hashlib.sha512(seed).digest()[:32]), _BASE))


def ed25519_sign(seed: bytes, message: bytes) -> bytes:
    """Deterministic RFC 8032 signing; used by tests and offline tooling only."""
    if len(seed) != 32:
        raise ValueError("Ed25519 seeds are 32 bytes")
    digest = hashlib.sha512(seed).digest()
    scalar = _clamp(digest[:32])
    public = _compress(_multiply(scalar, _BASE))
    nonce = _hash_scalar(digest[32:], message)
    commitment = _compress(_multiply(nonce, _BASE))
    challenge = _hash_scalar(commitment, public, message)
    return commitment + ((nonce + challenge * scalar) % _L).to_bytes(32, "little")


def ed25519_verify(public_key: bytes, message: bytes, signature: bytes) -> bool:
    if len(public_key) != 32 or len(signature) != 64:
        return False
    commitment = _decompress(signature[:32])
    key_point = _decompress(public_key)
    if commitment is None or key_point is None:
        return False
    scalar = int.from_bytes(signature[32:], "little")
    if scalar >= _L:
        return False
    challenge = _hash_scalar(signature[:32], public_key, message)
    left = _multiply(scalar, _BASE)
    right = _add(commitment, _multiply(challenge, key_point))
    return _compress(left) == _compress(right)


# --- minisign formats ------------------------------------------------------


@dataclass(frozen=True, slots=True)
class PublicKey:
    key_id: bytes
    key: bytes
    comment: str

    @property
    def key_id_hex(self) -> str:
        return self.key_id.hex().upper()


@dataclass(frozen=True, slots=True)
class Signature:
    algorithm: bytes
    key_id: bytes
    signature: bytes
    trusted_comment: str
    global_signature: bytes


def _decode(line: str, expected_length: int, what: str) -> bytes:
    try:
        raw = base64.b64decode(line.strip().encode("ascii"), validate=True)
    except (ValueError, UnicodeEncodeError) as error:
        raise SignatureError(f"{what} is not valid base64") from error
    if len(raw) != expected_length:
        raise SignatureError(f"{what} has an unexpected length")
    return raw


def _lines(text: str, limit: int, what: str) -> list[str]:
    if len(text.encode("utf-8", "surrogateescape")) > limit:
        raise SignatureError(f"{what} exceeds the size limit")
    lines = [line.rstrip("\r") for line in text.split("\n")]
    while lines and not lines[-1]:
        lines.pop()
    return lines


def parse_public_key(text: str) -> PublicKey:
    lines = _lines(text, MAX_PUBLIC_KEY_BYTES, "public key")
    comment = ""
    if lines and lines[0].startswith(_UNTRUSTED_COMMENT_PREFIX):
        comment = lines.pop(0)[len(_UNTRUSTED_COMMENT_PREFIX):]
    if len(lines) != 1:
        raise SignatureError("public key must hold one untrusted comment and one key line")
    raw = _decode(lines[0], 42, "public key")
    if raw[:2] != KEY_ALGORITHM:
        raise SignatureError("public key algorithm is not Ed25519")
    if _decompress(raw[10:]) is None:
        raise SignatureError("public key is not a valid Ed25519 point")
    return PublicKey(key_id=raw[2:10], key=raw[10:], comment=comment)


def load_public_key(path: Path) -> PublicKey:
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError as error:
        raise SignatureError(f"release signing key is missing: {path}") from error
    except (OSError, UnicodeDecodeError) as error:
        raise SignatureError(f"release signing key cannot be read: {path}") from error
    return parse_public_key(text)


def parse_signature(text: str) -> Signature:
    lines = _lines(text, MAX_SIGNATURE_BYTES, "signature")
    if len(lines) != 4 or not lines[0].startswith(_UNTRUSTED_COMMENT_PREFIX):
        raise SignatureError("signature must hold exactly the four minisign lines")
    raw = _decode(lines[1], 74, "signature")
    algorithm = raw[:2]
    if algorithm not in (SIGNATURE_PREHASHED, SIGNATURE_LEGACY):
        raise SignatureError("signature algorithm is not Ed25519")
    if not lines[2].startswith(_TRUSTED_COMMENT_PREFIX):
        raise SignatureError("signature trusted comment line is malformed")
    trusted_comment = lines[2][len(_TRUSTED_COMMENT_PREFIX):]
    if len(trusted_comment.encode()) > MAX_TRUSTED_COMMENT_BYTES or not _PRINTABLE.fullmatch(
        trusted_comment.replace("\t", " ")
    ):
        raise SignatureError("signature trusted comment is not bounded printable text")
    global_signature = _decode(lines[3], 64, "global signature")
    return Signature(
        algorithm=algorithm,
        key_id=raw[2:10],
        signature=raw[10:],
        trusted_comment=trusted_comment,
        global_signature=global_signature,
    )


def _file_digest(path: Path) -> bytes:
    digest = hashlib.blake2b(digest_size=64)
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.digest()


def _trusted_comment_names(comment: str) -> set[str]:
    return {
        field[len("file:"):]
        for field in comment.split("\t")
        if field.startswith("file:")
    }


def verify_file(
    path: Path,
    signature_text: str,
    public_key: PublicKey,
    *,
    expected_name: str | None = None,
) -> Signature:
    """Verify ``path`` against a minisign signature and return the parsed signature.

    Both the file signature and the global signature over the trusted comment
    must verify under ``public_key``. When ``expected_name`` is given, the
    trusted comment must bind the signature to that file name, so a valid
    signature for another asset cannot be reused.
    """
    signature = parse_signature(signature_text)
    if signature.key_id != public_key.key_id:
        raise SignatureError(
            f"signature key {signature.key_id.hex().upper()} does not match the pinned release key "
            f"{public_key.key_id_hex}"
        )
    if signature.algorithm == SIGNATURE_PREHASHED:
        message = _file_digest(path)
    else:
        if path.stat().st_size > 64 * 1024 * 1024:
            raise SignatureError("legacy signatures are accepted only for files up to 64 MiB")
        message = path.read_bytes()
    if not ed25519_verify(public_key.key, message, signature.signature):
        raise SignatureError(f"release signature does not verify for {path.name}")
    if not ed25519_verify(
        public_key.key, signature.signature + signature.trusted_comment.encode(), signature.global_signature
    ):
        raise SignatureError("release signature trusted comment does not verify")
    if expected_name is not None and expected_name not in _trusted_comment_names(signature.trusted_comment):
        raise SignatureError(f"release signature is not bound to {expected_name}")
    return signature


def sign_file(
    path: Path,
    seed: bytes,
    key_id: bytes,
    *,
    trusted_comment: str | None = None,
    untrusted_comment: str = "signature from omasheets tests",
) -> str:
    """Produce a minisign-compatible prehashed signature; tests and offline tooling only."""
    if len(key_id) != 8:
        raise ValueError("minisign key ids are 8 bytes")
    comment = trusted_comment if trusted_comment is not None else f"timestamp:0\tfile:{path.name}\thashed"
    if not _PRINTABLE.fullmatch(comment.replace("\t", " ")):
        raise ValueError("trusted comments must be printable")
    signature = ed25519_sign(seed, _file_digest(path))
    global_signature = ed25519_sign(seed, signature + comment.encode())
    return "\n".join([
        f"{_UNTRUSTED_COMMENT_PREFIX}{untrusted_comment}",
        base64.b64encode(SIGNATURE_PREHASHED + key_id + signature).decode(),
        f"{_TRUSTED_COMMENT_PREFIX}{comment}",
        base64.b64encode(global_signature).decode(),
    ]) + "\n"


def format_public_key(seed: bytes, key_id: bytes, *, comment: str = "minisign public key") -> str:
    """Encode the public half of ``seed`` as a minisign public key file."""
    if len(key_id) != 8:
        raise ValueError("minisign key ids are 8 bytes")
    return f"{_UNTRUSTED_COMMENT_PREFIX}{comment}\n" + base64.b64encode(
        KEY_ALGORITHM + key_id + ed25519_public_key(seed)
    ).decode() + "\n"


def generate_key_id() -> bytes:
    return os.urandom(8)
