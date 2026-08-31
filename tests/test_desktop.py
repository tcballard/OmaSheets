from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from omasheets.desktop import open_workbooks
from omasheets.errors import PolicyError


class DesktopTests(unittest.TestCase):
    def test_open_uses_an_argv_vector_without_a_shell(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "budget $(touch nope).xls"
            workbook.write_bytes(b"xls")
            with patch("omasheets.desktop.calc_executable", return_value="/usr/bin/libreoffice"), patch(
                "omasheets.desktop.subprocess.Popen"
            ) as popen:
                popen.return_value.pid = 42
                self.assertEqual(open_workbooks([workbook]), 42)
            args, kwargs = popen.call_args
            self.assertEqual(args[0], ["/usr/bin/libreoffice", "--calc", "--", str(workbook.resolve())])
            self.assertNotIn("shell", kwargs)

    def test_open_rejects_unsupported_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            document = Path(temporary) / "notes.txt"
            document.write_text("no")
            with self.assertRaises(PolicyError):
                open_workbooks([document])


if __name__ == "__main__":
    unittest.main()
