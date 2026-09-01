from contextlib import redirect_stderr
from io import StringIO
import json
from pathlib import Path
import tempfile
import unittest

from scripts import sample_corpus


def _tree(root: Path) -> None:
    (root / "b").mkdir()
    (root / "a.xlsx").write_bytes(b"one")
    (root / "b" / "dup.xlsx").write_bytes(b"one")
    (root / "b" / "c.xlsx").write_bytes(b"two")
    (root / "d.xlsx").write_bytes(b"three")
    (root / "notes.txt").write_bytes(b"skip")
    (root / "link.xlsx").symlink_to("a.xlsx")


class SampleCorpusTests(unittest.TestCase):
    def test_sample_is_deterministic_and_collapses_duplicates(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            _tree(root)
            first, report = sample_corpus.sample(root, 2, "enron")
            second, _ = sample_corpus.sample(root, 2, "enron")
            self.assertEqual(first, second)
            self.assertEqual(report["files_seen"], 4)
            self.assertEqual(report["unique_workbooks"], 3)
            self.assertEqual(report["duplicate_files"], 1)
            self.assertEqual(report["sampled"], 2)
            digests = [entry["sha256"] for entry in first]
            self.assertEqual(digests, sorted(digests))
            self.assertEqual([entry["id"] for entry in first], ["enron-0001", "enron-0002"])
            everything, _ = sample_corpus.sample(root, 3, "enron")
            paths = {entry["path"] for entry in everything}
            self.assertIn("a.xlsx", paths)
            self.assertNotIn("b/dup.xlsx", paths)
            self.assertNotIn("link.xlsx", paths)

    def test_manifest_is_written_once_in_scorer_schema(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            _tree(root)
            output = root / "out" / "sample.jsonl"
            status = sample_corpus.main([str(root), str(output), "--count", "3", "--prefix", "x"])
            self.assertEqual(status, 0)
            lines = output.read_text().splitlines()
            self.assertEqual(len(lines), 3)
            entry = json.loads(lines[0])
            self.assertEqual(list(entry), ["id", "path", "sha256"])
            self.assertEqual(len(entry["sha256"]), 64)
            errors = StringIO()
            with redirect_stderr(errors):
                self.assertEqual(sample_corpus.main([str(root), str(output)]), 1)
            self.assertIn("refusing to replace", errors.getvalue())

    def test_bounds_are_enforced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            _tree(root)
            with self.assertRaises(sample_corpus.SampleError):
                sample_corpus.sample(root, 0, "x")
            with self.assertRaises(sample_corpus.SampleError):
                sample_corpus.sample(root, 1001, "x")
            with self.assertRaises(sample_corpus.SampleError):
                sample_corpus.sample(root, 1, "bad prefix")
            with self.assertRaises(sample_corpus.SampleError):
                sample_corpus.sample(root / "missing", 1, "x")


if __name__ == "__main__":
    unittest.main()
