from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

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


if __name__ == "__main__":
    unittest.main()
