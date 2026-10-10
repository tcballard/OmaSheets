from pathlib import Path
import unittest
from unittest.mock import patch

from omasheets.desktop import open_workbooks
from omasheets.errors import EngineError


class DesktopTests(unittest.TestCase):
    def test_all_documents_use_owned_launchers(self):
        paths = [Path("book.xlsx"), Path("native.omasheets")]
        with patch("omasheets.desktop.open_workbook", side_effect=[41, 42]) as launch:
            self.assertEqual(open_workbooks(paths), 42)
        self.assertEqual([call.args[0] for call in launch.call_args_list], paths)

    def test_open_requires_a_document(self):
        with self.assertRaises(EngineError):
            open_workbooks([])


if __name__ == "__main__":
    unittest.main()
