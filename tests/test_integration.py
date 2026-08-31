from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from omasheets.errors import ConflictError
from omasheets.integration import DESKTOP_ENTRY, DESKTOP_ID, IntegrationPaths, install, uninstall


class IntegrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.paths = IntegrationPaths(
            root / "data/applications" / DESKTOP_ID,
            root / "config/mimeapps.list",
            root / "state/desktop-integration.json",
        )
        self.refresh = patch("omasheets.integration._refresh_desktop_database")
        self.refresh.start()

    def tearDown(self):
        self.refresh.stop()
        self.temporary.cleanup()

    def test_install_and_uninstall_restore_exact_original(self):
        self.assertIn("Exec=omasheets open %F", DESKTOP_ENTRY)
        original = b"[Default Applications]\napplication/vnd.ms-excel=calc.desktop;\n# keep me\n"
        self.paths.mimeapps.parent.mkdir(parents=True)
        self.paths.mimeapps.write_bytes(original)
        result = install(self.paths)
        self.assertTrue(result["changed"])
        self.assertIn(DESKTOP_ID.encode(), self.paths.mimeapps.read_bytes())
        self.assertFalse(install(self.paths)["changed"])
        uninstall(self.paths)
        self.assertEqual(self.paths.mimeapps.read_bytes(), original)
        self.assertFalse(self.paths.desktop.exists())

    def test_uninstall_preserves_unrelated_mime_change(self):
        install(self.paths)
        with self.paths.mimeapps.open("ab") as handle:
            handle.write(b"\n[X-Added]\nvalue=keep;\n")
        uninstall(self.paths)
        text = self.paths.mimeapps.read_text()
        self.assertIn("[X-Added]", text)
        self.assertNotIn(DESKTOP_ID, text)

    def test_modified_desktop_entry_is_never_deleted(self):
        install(self.paths)
        self.paths.desktop.write_text("user modified this")
        with self.assertRaises(ConflictError):
            uninstall(self.paths)
        self.assertEqual(self.paths.desktop.read_text(), "user modified this")


if __name__ == "__main__":
    unittest.main()
