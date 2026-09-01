from contextlib import redirect_stderr
from io import StringIO
import json
import os
from pathlib import Path
import tempfile
import unittest
import zipfile

from scripts import fetch_corpus


def _register(path: Path, **overrides) -> Path:
    payload = {
        "schema": 1,
        "name": "sample",
        "url": "file:///dev/null",
        "archive_sha256": "0" * 64,
        "license": "test",
        "retrieved": "2026-09-01",
        "sampling": "all",
    }
    payload.update(overrides)
    path.write_text(json.dumps(payload))
    return path


def _archive(path: Path) -> str:
    with zipfile.ZipFile(path, "w") as bundle:
        bundle.writestr("nested/book.xlsx", b"workbook one")
        bundle.writestr("UPPER.XLSX", b"workbook two")
        bundle.writestr("notes.txt", b"not a workbook")
        bundle.writestr("../escape.xlsx", b"traversal")
        bundle.writestr("/absolute.xlsx", b"absolute")
        link = zipfile.ZipInfo("link.xlsx")
        link.external_attr = (0o120777 << 16)
        bundle.writestr(link, "nested/book.xlsx")
    return fetch_corpus.sha256_file(path)


class FetchCorpusTests(unittest.TestCase):
    def test_fetch_extracts_only_safe_workbooks_and_records_the_digest(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "source.zip"
            digest = _archive(archive)
            register = _register(
                root / "register.json",
                url=archive.as_uri(),
                archive_sha256=digest,
            )
            report = fetch_corpus.fetch(register, root / "corpus")
            self.assertEqual(report["archive_sha256"], digest)
            self.assertEqual(report["workbooks_extracted"], 2)
            self.assertEqual(report["members_skipped"], 4)
            extracted = sorted(
                path.relative_to(root / "corpus" / "sample").as_posix()
                for path in (root / "corpus" / "sample").rglob("*")
                if path.is_file()
            )
            self.assertEqual(extracted, ["UPPER.xlsx", "nested/book.xlsx"])
            self.assertEqual(
                oct((root / "corpus" / "sample" / "nested" / "book.xlsx").stat().st_mode & 0o777),
                "0o600",
            )
            self.assertFalse((root / "corpus" / "sample" / "notes.txt").exists())
            self.assertFalse((root / "escape.xlsx").exists())
            self.assertTrue((root / "corpus" / "archives" / "sample.zip").is_file())

    def test_digest_drift_removes_the_download_and_stops(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "source.zip"
            _archive(archive)
            register = _register(root / "register.json", url=archive.as_uri())
            with self.assertRaises(fetch_corpus.FetchError) as raised:
                fetch_corpus.fetch(register, root / "corpus")
            self.assertIn("digest drift", str(raised.exception))
            self.assertFalse((root / "corpus" / "archives" / "sample.zip").exists())
            self.assertFalse((root / "corpus" / "sample").exists())

    def test_existing_archive_or_extraction_is_never_replaced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "source.zip"
            digest = _archive(archive)
            register = _register(
                root / "register.json", url=archive.as_uri(), archive_sha256=digest
            )
            fetch_corpus.fetch(register, root / "corpus")
            with self.assertRaises(fetch_corpus.FetchError) as raised:
                fetch_corpus.fetch(register, root / "corpus")
            self.assertIn("refusing to replace", str(raised.exception))

    def test_register_validation_rejects_unsafe_sources(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cases = {
                "url": "http://example.invalid/corpus.zip",
                "archive_sha256": "ABC",
                "name": "../enron",
                "schema": 2,
            }
            for field, value in cases.items():
                register = _register(root / f"{field}.json", **{field: value})
                with self.assertRaises(fetch_corpus.FetchError, msg=field):
                    fetch_corpus.load_register(register)
            incomplete = root / "incomplete.json"
            incomplete.write_text(json.dumps({"schema": 1, "name": "x"}))
            with self.assertRaises(fetch_corpus.FetchError) as raised:
                fetch_corpus.load_register(incomplete)
            self.assertIn("missing", str(raised.exception))

    def test_command_line_reports_bounded_failures(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            register = _register(root / "register.json")
            errors = StringIO()
            with redirect_stderr(errors):
                status = fetch_corpus.main([str(register), str(root / "corpus")])
            self.assertEqual(status, 1)
            self.assertTrue(errors.getvalue().startswith("fetch_corpus: "))
            self.assertNotIn(str(root), errors.getvalue())


if __name__ == "__main__":
    unittest.main()
