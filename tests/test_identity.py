from pathlib import Path
import tempfile
import unittest

from omasheets.errors import ConflictError
from omasheets.identity import identify_regular_file


class IdentityTests(unittest.TestCase):
    def test_hashes_a_regular_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "book.xlsx"
            path.write_bytes(b"workbook")
            identity = identify_regular_file(path)
            self.assertEqual(identity.size, 8)
            self.assertEqual(len(identity.sha256), 64)

    def test_rejects_a_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target.xlsx"
            link = root / "link.xlsx"
            target.write_bytes(b"workbook")
            link.symlink_to(target)
            with self.assertRaises(ConflictError):
                identify_regular_file(link)


if __name__ == "__main__":
    unittest.main()

