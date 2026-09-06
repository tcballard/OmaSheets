"""Exercise release selection against incomplete and unrelated publication data."""
import json
from pathlib import Path
import shutil
import subprocess
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(shutil.which("jq"), "jq is required for the shell installer")
class DevelopmentSelectionTests(unittest.TestCase):
    def select(self, releases):
        source = (ROOT / "bin/omasheets-install").read_text()
        expression = source.split("jq -e '", 1)[1].split("' \\\n", 1)[0]
        return subprocess.run(["jq", "-e", expression], input=json.dumps(releases), text=True, capture_output=True)

    def release(self, **changes):
        sha = "a" * 40
        result = dict(draft=False, prerelease=True, tag_name="dev-" + sha,
                      target_commitish=sha, published_at="2026-09-06T08:57:50Z", assets=[
                          {"name": "omasheets-native-0.0.2-linux-x86_64.tar.gz"},
                          {"name": "omasheets-native-0.0.2-linux-x86_64.tar.gz.sha256"}])
        return {**result, **changes}

    def test_skips_unfinished_stable_and_mismatched_releases(self):
        wanted = self.release()
        result = self.select([self.release(draft=True), self.release(prerelease=False),
                              self.release(target_commitish="main"), self.release(assets=[]), wanted])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), wanted)

    def test_latest_publication_wins_even_when_github_lists_old_release_first(self):
        old = self.release()
        newest = self.release(tag_name="dev-" + "b" * 40, target_commitish="b" * 40,
                              published_at="2026-09-06T10:05:47Z")
        for releases in ([old, newest], [newest, old]):
            result = self.select(releases)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), newest)

    def test_missing_or_ambiguous_bundles_are_not_selected(self):
        for release in [self.release(assets=[]), self.release(assets=self.release()["assets"] * 2)]:
            with self.subTest(release=release):
                self.assertNotEqual(self.select([release]).returncode, 0)
        self.assertNotEqual(self.select([]).returncode, 0)


class UpdateCommandTests(unittest.TestCase):
    def test_update_delegates_to_installed_helper_and_preserves_failure(self):
        from omasheets.cli import main
        with patch("omasheets.cli.Path.is_file", return_value=True), patch(
            "subprocess.run", return_value=Mock(returncode=7),
        ) as run:
            self.assertEqual(main(["update"]), 7)
        self.assertEqual(run.call_args.args[0][0], "/bin/bash")
        self.assertTrue(run.call_args.args[0][1].endswith("/bin/omasheets-update"))
        self.assertFalse(run.call_args.kwargs["check"])
