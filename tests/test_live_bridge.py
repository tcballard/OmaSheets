import hashlib
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from omasheets import live_bridge
from omasheets.errors import EngineError
from omasheets.live_bridge import bridge_path, request_live_snapshot, semantic_snapshot_hash
from omasheets.paths import AppPaths


class LiveBridgeTests(unittest.TestCase):
    def test_semantic_hash_ignores_view_state_but_detects_cell_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            hashes = []
            for index, value in enumerate(("42", "42", "43")):
                workbook = root / f"book-{index}.xlsx"
                with zipfile.ZipFile(workbook, "w") as archive:
                    archive.writestr(
                        "xl/workbook.xml",
                        f"<workbook><bookViews><view activeTab='{index}'/></bookViews><sheets/></workbook>",
                    )
                    archive.writestr(
                        "xl/worksheets/sheet1.xml",
                        f"<worksheet><sheetViews><selection activeCell='A{index + 1}'/></sheetViews>"
                        f"<sheetData><v>{value}</v></sheetData></worksheet>",
                    )
                hashes.append(semantic_snapshot_hash(workbook, "xlsx"))
            self.assertEqual(hashes[0], hashes[1])
            self.assertNotEqual(hashes[1], hashes[2])

    def test_semantic_hash_streams_members_instead_of_reading_them_whole(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "book.xlsx"
            with zipfile.ZipFile(workbook, "w") as archive:
                archive.writestr("xl/workbook.xml", "<workbook><sheets/></workbook>")
                archive.writestr(
                    "xl/worksheets/sheet1.xml",
                    "<worksheet><sheetData>" + "".join(
                        f"<row r='{row}'><c><v>{row}</v></c></row>" for row in range(5000)
                    ) + "</sheetData></worksheet>",
                )
            with patch.object(zipfile.ZipFile, "read", side_effect=AssertionError("whole-member read")):
                result = semantic_snapshot_hash(workbook, "xlsx")
            self.assertEqual(len(result), 64)

    def test_malformed_xml_falls_back_to_stable_change_sensitive_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            hashes = []
            for index, value in enumerate(("42", "42", "43")):
                workbook = root / f"malformed-{index}.xlsx"
                with zipfile.ZipFile(workbook, "w") as archive:
                    archive.writestr("xl/workbook.xml", "<workbook><sheets/></workbook>")
                    archive.writestr(
                        "xl/worksheets/sheet1.xml",
                        f"<worksheet><sheetData><v>{value}</sheetData></worksheet>",
                    )
                hashes.append(semantic_snapshot_hash(workbook, "xlsx"))
            self.assertEqual(hashes[0], hashes[1])
            self.assertNotEqual(hashes[1], hashes[2])

    def test_binary_xls_semantic_hash_is_streamed_and_change_sensitive(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "book.xls"
            workbook.write_bytes(b"binary-workbook-one")
            first = semantic_snapshot_hash(workbook, "xls")
            self.assertEqual(first, hashlib.sha256(b"binary-workbook-one").hexdigest())
            workbook.write_bytes(b"binary-workbook-two")
            second = semantic_snapshot_hash(workbook, "xls")
            self.assertNotEqual(first, second)

    def test_semantic_hash_rejects_in_place_change_during_single_open_pass(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "book.xls"
            workbook.write_bytes(b"binary-workbook")
            original_stream_digest = live_bridge._stream_digest

            def mutate_after_read(handle):
                result = original_stream_digest(handle)
                with workbook.open("ab") as writer:
                    writer.write(b"-changed")
                return result

            with patch("omasheets.live_bridge._stream_digest", side_effect=mutate_after_read):
                with self.assertRaisesRegex(EngineError, "changed while it was hashed"):
                    semantic_snapshot_hash(workbook, "xls")

    def test_semantic_hash_rejects_fifo_swap_without_blocking(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "book.xls"
            workbook.write_bytes(b"binary-workbook")
            real_open = os.open
            swapped = False

            def swap_before_open(path, flags, mode=0o777, *, dir_fd=None):
                nonlocal swapped
                if Path(path) == workbook and not swapped:
                    workbook.unlink()
                    os.mkfifo(workbook)
                    swapped = True
                if dir_fd is None:
                    return real_open(path, flags, mode)
                return real_open(path, flags, mode, dir_fd=dir_fd)

            with patch("omasheets.live_bridge.os.open", side_effect=swap_before_open):
                with self.assertRaisesRegex(EngineError, "changed while it was opened"):
                    semantic_snapshot_hash(workbook, "xls")

            self.assertTrue(swapped)

    def test_snapshot_swap_during_hash_is_rejected_and_cleaned_up(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = AppPaths(root / "state", root / "cache", root / "runtime")
            paths.ensure()
            expected = paths.runtime / f"snapshot-{'a' * 32}-{'e' * 32}.xlsx"
            replacement = paths.runtime / "replacement.xlsx"

            class FakeConnection:
                def __enter__(self):
                    return self

                def __exit__(self, *_):
                    return None

                def settimeout(self, _):
                    pass

                def connect(self, _):
                    pass

                def sendall(self, request):
                    _, session, nonce = request.decode("ascii").strip().split()
                    snapshot = paths.runtime / f"snapshot-{session}-{nonce}.xlsx"
                    with zipfile.ZipFile(snapshot, "w") as archive:
                        archive.writestr("xl/workbook.xml", "<workbook><sheets/></workbook>")
                    snapshot.chmod(0o600)

                def recv(self, _):
                    return b'{"ok":true,"format":"xlsx"}\n'

            original_member_digest = live_bridge._member_digest
            swapped = False

            def swap_path_during_hash(archive, name):
                nonlocal swapped
                if not swapped:
                    with zipfile.ZipFile(replacement, "w") as replacement_archive:
                        replacement_archive.writestr(
                            "xl/workbook.xml",
                            "<workbook><sheets><sheet name='swapped'/></sheets></workbook>",
                        )
                    replacement.chmod(0o600)
                    os.replace(replacement, expected)
                    swapped = True
                return original_member_digest(archive, name)

            with patch("omasheets.live_bridge._validate_socket"), patch(
                "omasheets.live_bridge.socket.socket", return_value=FakeConnection()
            ), patch(
                "omasheets.live_bridge.secrets.token_hex", return_value="e" * 32
            ), patch(
                "omasheets.live_bridge._member_digest", side_effect=swap_path_during_hash
            ):
                with self.assertRaisesRegex(EngineError, "changed while it was hashed"):
                    request_live_snapshot(paths, "a" * 32, ".xlsx")

            self.assertTrue(swapped)
            self.assertFalse(expected.exists())
            self.assertFalse(replacement.exists())

    def test_snapshot_request_is_bounded_and_returns_private_exact_path(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = AppPaths(root / "state", root / "cache", root / "runtime")
            paths.ensure()

            class FakeConnection:
                def __enter__(self):
                    return self

                def __exit__(self, *_):
                    return None

                def settimeout(self, _):
                    pass

                def connect(self, path):
                    self.connected = path

                def sendall(self, request):
                    _, session, nonce = request.decode("ascii").strip().split()
                    snapshot = paths.runtime / f"snapshot-{session}-{nonce}.xlsx"
                    with zipfile.ZipFile(snapshot, "w") as archive:
                        archive.writestr("xl/workbook.xml", "<workbook><bookViews><view activeTab='2'/></bookViews><sheets/></workbook>")
                        archive.writestr("xl/worksheets/sheet1.xml", "<worksheet><sheetData><v>42</v></sheetData></worksheet>")
                    snapshot.chmod(0o600)
                def recv(self, _):
                    return b'{"ok":true,"format":"xlsx"}\n'

            connection = FakeConnection()
            with patch("omasheets.live_bridge._validate_socket"), patch(
                "omasheets.live_bridge.socket.socket", return_value=connection
            ), patch("omasheets.live_bridge.secrets.token_hex", return_value="b" * 32):
                result = request_live_snapshot(paths, "a" * 32, ".xlsx")
            self.assertEqual(connection.connected, str(bridge_path(paths)))
            self.assertEqual(result.format, "xlsx")
            self.assertEqual(len(result.semantic_sha256), 64)
            self.assertIn("a" * 32, result.path.name)

    def test_snapshot_is_removed_when_semantic_hashing_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = AppPaths(root / "state", root / "cache", root / "runtime")
            paths.ensure()

            class FakeConnection:
                def __enter__(self):
                    return self

                def __exit__(self, *_):
                    return None

                def settimeout(self, _):
                    pass

                def connect(self, _):
                    pass

                def sendall(self, request):
                    _, session, nonce = request.decode("ascii").strip().split()
                    snapshot = paths.runtime / f"snapshot-{session}-{nonce}.xlsx"
                    snapshot.write_bytes(b"not-a-workbook-package")
                    snapshot.chmod(0o600)

                def recv(self, _):
                    return b'{"ok":true,"format":"xlsx"}\n'

            with patch("omasheets.live_bridge._validate_socket"), patch(
                "omasheets.live_bridge.socket.socket", return_value=FakeConnection()
            ), patch("omasheets.live_bridge.secrets.token_hex", return_value="b" * 32):
                with self.assertRaisesRegex(EngineError, "valid workbook package"):
                    request_live_snapshot(paths, "a" * 32, ".xlsx")
            self.assertFalse((paths.runtime / f"snapshot-{'a' * 32}-{'b' * 32}.xlsx").exists())

    def test_snapshot_is_removed_when_bridge_returns_invalid_json(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = AppPaths(root / "state", root / "cache", root / "runtime")
            paths.ensure()

            class FakeConnection:
                def __enter__(self):
                    return self

                def __exit__(self, *_):
                    return None

                def settimeout(self, _):
                    pass

                def connect(self, _):
                    pass

                def sendall(self, request):
                    _, session, nonce = request.decode("ascii").strip().split()
                    snapshot = paths.runtime / f"snapshot-{session}-{nonce}.xlsx"
                    snapshot.write_bytes(b"partial-snapshot")
                    snapshot.chmod(0o600)

                def recv(self, _):
                    return b"not-json\n"

            expected = paths.runtime / f"snapshot-{'a' * 32}-{'c' * 32}.xlsx"
            with patch("omasheets.live_bridge._validate_socket"), patch(
                "omasheets.live_bridge.socket.socket", return_value=FakeConnection()
            ), patch("omasheets.live_bridge.secrets.token_hex", return_value="c" * 32):
                with self.assertRaisesRegex(EngineError, "invalid JSON"):
                    request_live_snapshot(paths, "a" * 32, ".xlsx")
            self.assertFalse(expected.exists())

    def test_snapshot_is_removed_when_permissions_are_invalid(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = AppPaths(root / "state", root / "cache", root / "runtime")
            paths.ensure()

            class FakeConnection:
                def __enter__(self):
                    return self

                def __exit__(self, *_):
                    return None

                def settimeout(self, _):
                    pass

                def connect(self, _):
                    pass

                def sendall(self, request):
                    _, session, nonce = request.decode("ascii").strip().split()
                    snapshot = paths.runtime / f"snapshot-{session}-{nonce}.xlsx"
                    snapshot.write_bytes(b"snapshot")
                    snapshot.chmod(0o644)

                def recv(self, _):
                    return b'{"ok":true,"format":"xlsx"}\n'

            expected = paths.runtime / f"snapshot-{'a' * 32}-{'d' * 32}.xlsx"
            with patch("omasheets.live_bridge._validate_socket"), patch(
                "omasheets.live_bridge.socket.socket", return_value=FakeConnection()
            ), patch("omasheets.live_bridge.secrets.token_hex", return_value="d" * 32):
                with self.assertRaisesRegex(EngineError, "permissions are invalid"):
                    request_live_snapshot(paths, "a" * 32, ".xlsx")
            self.assertFalse(expected.exists())


if __name__ == "__main__":
    unittest.main()
