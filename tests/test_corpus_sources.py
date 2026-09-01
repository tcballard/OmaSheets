import json
from pathlib import Path
import unittest

from scripts import fetch_corpus

ROOT = Path(__file__).resolve().parents[1]
SOURCES = ROOT / "corpus" / "sources"


class CorpusSourceTests(unittest.TestCase):
    def registers(self):
        # Registers are `<name>.json`; derived files carry a dotted suffix such
        # as `<name>.score-summary.json` and are checked separately.
        return sorted(path for path in SOURCES.glob("*.json") if "." not in path.stem)

    def test_score_summaries_are_aggregate_only(self):
        summaries = sorted(SOURCES.glob("*.score-summary.json"))
        for summary in summaries:
            with self.subTest(summary=summary.name):
                payload = json.loads(summary.read_text(encoding="utf-8"))
                self.assertEqual(payload["schema"], 1)
                self.assertTrue((SOURCES / f"{payload['source']}.json").is_file())
                self.assertEqual(payload["manifest"], f"{payload['source']}.jsonl")
                self.assertRegex(payload["manifest_sha256"], r"^[0-9a-f]{64}$")
                for key in ("candidate_summary", "owned_summary", "owned_unsupported_functions"):
                    self.assertIn(key, payload)
                text = summary.read_text(encoding="utf-8")
                for forbidden in ('"path"', '"entries"', ".xlsx"):
                    self.assertNotIn(forbidden, text)

    def test_every_register_validates_and_names_a_frozen_manifest(self):
        registers = self.registers()
        self.assertTrue(registers, "at least one register is expected")
        for register in registers:
            with self.subTest(register=register.name):
                payload = fetch_corpus.load_register(register)
                self.assertEqual(payload["name"], register.stem)
                self.assertTrue(payload["url"].startswith("https://"))
                self.assertIn("license", payload)
                manifest = SOURCES / payload["manifest"]
                self.assertTrue(manifest.is_file(), f"{manifest.name} is missing")
                lines = manifest.read_text(encoding="utf-8").splitlines()
                self.assertEqual(len(lines), payload["sample_count"])
                self.assertLessEqual(len(lines), 1000)
                self.assertLessEqual(manifest.stat().st_size, 1024 * 1024)
                ids = set()
                paths = set()
                digests = set()
                for line in lines:
                    entry = json.loads(line)
                    self.assertEqual(list(entry), ["id", "path", "sha256"])
                    self.assertRegex(entry["id"], r"^[A-Za-z0-9._-]{1,64}$")
                    self.assertRegex(entry["sha256"], r"^[0-9a-f]{64}$")
                    self.assertTrue(entry["path"].endswith(".xlsx"))
                    self.assertFalse(entry["path"].startswith("/"))
                    self.assertNotIn("..", entry["path"].split("/"))
                    ids.add(entry["id"])
                    paths.add(entry["path"])
                    digests.add(entry["sha256"])
                self.assertEqual(len(ids), len(lines))
                self.assertEqual(len(paths), len(lines))
                self.assertEqual(len(digests), len(lines))
                self.assertEqual(sorted(digests), [json.loads(line)["sha256"] for line in lines])


if __name__ == "__main__":
    unittest.main()
