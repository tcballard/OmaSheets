from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
import tempfile
import unittest

from scripts import check_import_corpus, sample_corpus


class ImportCorpusTests(unittest.TestCase):
    def test_generation_reproduces_workbook_bytes_and_frozen_manifest(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first, second = root / "first", root / "second"
            with redirect_stdout(StringIO()):
                self.assertEqual(check_import_corpus.main([str(first)]), 0)
                self.assertEqual(check_import_corpus.main([str(second)]), 0)
            self.assertEqual((first / "manifest.jsonl").read_bytes(),
                             (second / "manifest.jsonl").read_bytes())
            workbooks = sorted((first / "workbooks").glob("*.xlsx"))
            self.assertEqual(len(workbooks), 4)
            for workbook in workbooks:
                self.assertEqual(workbook.read_bytes(),
                                 (second / "workbooks" / workbook.name).read_bytes())
            entries, _ = sample_corpus.sample(first / "workbooks", 4, "import")
            for entry in entries:
                self.assertEqual(entry["sha256"],
                                 sample_corpus.sha256_file(first / "workbooks" / entry["path"]))

    def test_generation_refuses_to_replace_existing_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "evidence"
            check_import_corpus.generate(output)
            before = {p.relative_to(output): p.read_bytes()
                      for p in output.rglob("*") if p.is_file()}
            with self.assertRaises(FileExistsError):
                check_import_corpus.generate(output)
            self.assertEqual(before, {p.relative_to(output): p.read_bytes()
                                     for p in output.rglob("*") if p.is_file()})

    def test_score_requires_every_manifest_entry(self):
        entries = [{"id": "import-0001", "path": "sparse-grid.xlsx", "sha256": "a" * 64}]
        with self.assertRaisesRegex(ValueError, "every generated workbook"):
            check_import_corpus.check_score({"entries": []}, entries)


if __name__ == "__main__":
    unittest.main()
