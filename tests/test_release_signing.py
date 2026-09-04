import base64
import os
from pathlib import Path
import tempfile
import unittest

from omasheets import release_signing as signing
from omasheets.release_signing import (
    PublicKey,
    SignatureError,
    ed25519_public_key,
    ed25519_sign,
    ed25519_verify,
    format_public_key,
    parse_public_key,
    parse_signature,
    sign_file,
    verify_file,
)


# RFC 8032, section 7.1, test vectors 1 to 3.
RFC_8032_VECTORS = (
    (
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "",
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    ),
    (
        "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        "72",
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    ),
    (
        "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
        "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        "af82",
        "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
    ),
)


class Ed25519Tests(unittest.TestCase):
    def test_rfc_8032_vectors_sign_and_verify(self):
        for seed, public, message, signature in RFC_8032_VECTORS:
            with self.subTest(public=public):
                seed_bytes = bytes.fromhex(seed)
                message_bytes = bytes.fromhex(message)
                self.assertEqual(ed25519_public_key(seed_bytes).hex(), public)
                self.assertEqual(ed25519_sign(seed_bytes, message_bytes).hex(), signature)
                self.assertTrue(ed25519_verify(bytes.fromhex(public), message_bytes, bytes.fromhex(signature)))

    def test_tampered_inputs_do_not_verify(self):
        seed, public, message, signature = RFC_8032_VECTORS[2]
        public_bytes, message_bytes, signature_bytes = (
            bytes.fromhex(public), bytes.fromhex(message), bytes.fromhex(signature),
        )
        self.assertFalse(ed25519_verify(public_bytes, message_bytes + b"\0", signature_bytes))
        flipped = signature_bytes[:-1] + bytes([signature_bytes[-1] ^ 0x01])
        self.assertFalse(ed25519_verify(public_bytes, message_bytes, flipped))
        other = ed25519_public_key(bytes.fromhex(RFC_8032_VECTORS[0][0]))
        self.assertFalse(ed25519_verify(other, message_bytes, signature_bytes))
        self.assertFalse(ed25519_verify(public_bytes, message_bytes, signature_bytes[:63]))
        self.assertFalse(ed25519_verify(public_bytes[:31], message_bytes, signature_bytes))

    def test_non_canonical_scalar_and_invalid_points_are_rejected(self):
        seed, public, message, signature = RFC_8032_VECTORS[1]
        public_bytes, message_bytes, signature_bytes = (
            bytes.fromhex(public), bytes.fromhex(message), bytes.fromhex(signature),
        )
        scalar = int.from_bytes(signature_bytes[32:], "little") + signing._L
        forged = signature_bytes[:32] + scalar.to_bytes(32, "little")
        self.assertFalse(ed25519_verify(public_bytes, message_bytes, forged))
        self.assertFalse(ed25519_verify(b"\xff" * 32, message_bytes, signature_bytes))
        self.assertFalse(ed25519_verify(public_bytes, message_bytes, b"\xff" * 32 + signature_bytes[32:]))


class MinisignFormatTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.seed = os.urandom(32)
        self.key_id = os.urandom(8)
        self.public_text = format_public_key(self.seed, self.key_id, comment="omasheets test key")
        self.public = parse_public_key(self.public_text)
        self.archive = self.root / "omasheets-native-9.9.9-linux-x86_64.tar.gz"
        self.archive.write_bytes(os.urandom(70_000))
        self.signature = sign_file(self.archive, self.seed, self.key_id)

    def tearDown(self):
        self.temporary.cleanup()

    def test_public_key_round_trips_the_minisign_layout(self):
        raw = base64.b64decode(self.public_text.splitlines()[1])
        self.assertEqual(raw[:2], b"Ed")
        self.assertEqual(raw[2:10], self.key_id)
        self.assertEqual(raw[10:], ed25519_public_key(self.seed))
        self.assertEqual(self.public, PublicKey(self.key_id, ed25519_public_key(self.seed), "omasheets test key"))
        self.assertEqual(self.public.key_id_hex, self.key_id.hex().upper())

    def test_signature_verifies_and_binds_the_file_name(self):
        verified = verify_file(self.archive, self.signature, self.public, expected_name=self.archive.name)
        self.assertEqual(verified.algorithm, b"ED")
        self.assertEqual(verified.key_id, self.key_id)
        self.assertIn(f"file:{self.archive.name}", verified.trusted_comment)
        with self.assertRaisesRegex(SignatureError, "not bound to other.tar.gz"):
            verify_file(self.archive, self.signature, self.public, expected_name="other.tar.gz")

    def test_modified_archive_wrong_key_and_forged_comment_are_rejected(self):
        original = self.archive.read_bytes()
        self.archive.write_bytes(original[:-1] + bytes([original[-1] ^ 1]))
        with self.assertRaisesRegex(SignatureError, "does not verify"):
            verify_file(self.archive, self.signature, self.public)
        self.archive.write_bytes(os.urandom(70_000))
        fresh = sign_file(self.archive, self.seed, self.key_id)
        other_key = parse_public_key(format_public_key(os.urandom(32), self.key_id))
        with self.assertRaisesRegex(SignatureError, "does not verify"):
            verify_file(self.archive, fresh, other_key)
        other_id = parse_public_key(format_public_key(self.seed, os.urandom(8)))
        with self.assertRaisesRegex(SignatureError, "does not match the pinned release key"):
            verify_file(self.archive, fresh, other_id)
        lines = fresh.splitlines()
        lines[2] = f"trusted comment: timestamp:0\tfile:{self.archive.name}\tforged"
        with self.assertRaisesRegex(SignatureError, "trusted comment does not verify"):
            verify_file(self.archive, "\n".join(lines) + "\n", self.public)

    def test_legacy_whole_file_signatures_are_accepted(self):
        message = self.archive.read_bytes()
        signature = ed25519_sign(self.seed, message)
        comment = f"timestamp:0\tfile:{self.archive.name}"
        legacy = "\n".join([
            "untrusted comment: legacy",
            base64.b64encode(b"Ed" + self.key_id + signature).decode(),
            f"trusted comment: {comment}",
            base64.b64encode(ed25519_sign(self.seed, signature + comment.encode())).decode(),
        ]) + "\n"
        self.assertEqual(verify_file(self.archive, legacy, self.public, expected_name=self.archive.name).algorithm, b"Ed")

    def test_malformed_keys_and_signatures_are_rejected_before_any_crypto(self):
        for text, message in (
            ("", "one untrusted comment and one key line"),
            ("untrusted comment: x\nnot base64!\n", "not valid base64"),
            ("untrusted comment: x\n" + base64.b64encode(b"XX" + b"\0" * 40).decode() + "\n", "not Ed25519"),
            ("untrusted comment: x\n" + base64.b64encode(b"Ed" + b"\0" * 8 + b"\xff" * 32).decode() + "\n", "valid Ed25519 point"),
            ("untrusted comment: x\n" + "A" * 5000 + "\n", "size limit"),
        ):
            with self.subTest(message=message), self.assertRaisesRegex(SignatureError, message):
                parse_public_key(text)
        good = self.signature.splitlines()
        for lines, message in (
            (good[:3], "four minisign lines"),
            (["comment"] + good[1:], "four minisign lines"),
            ([good[0], base64.b64encode(b"XX" + b"\0" * 72).decode()] + good[2:], "not Ed25519"),
            (good[:2] + ["untrusted comment: nope", good[3]], "trusted comment line is malformed"),
            (good[:2] + ["trusted comment: \x01"] + good[3:], "bounded printable"),
            (good[:3] + ["A" * 4], "unexpected length"),
        ):
            with self.subTest(message=message), self.assertRaisesRegex(SignatureError, message):
                parse_signature("\n".join(lines) + "\n")

    def test_signing_helpers_reject_bad_key_ids_and_comments(self):
        with self.assertRaises(ValueError):
            sign_file(self.archive, self.seed, b"short")
        with self.assertRaises(ValueError):
            sign_file(self.archive, self.seed, self.key_id, trusted_comment="bad\ncomment")
        with self.assertRaises(ValueError):
            format_public_key(self.seed, b"short")
        with self.assertRaises(ValueError):
            ed25519_public_key(b"short")


if __name__ == "__main__":
    unittest.main()
