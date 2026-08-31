from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from omasheets import __version__
from omasheets.native_bundle import download_native_bundle, require_exact_version_tag
from scripts import check_release


ROOT = Path(__file__).resolve().parents[1]


class ExactVersionTagTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.repository = Path(self.temporary.name)
        self._git("init", "-q")
        self._git("config", "user.name", "OmaSheets Tests")
        self._git("config", "user.email", "omasheets-tests@example.invalid")
        self._commit("first")

    def tearDown(self):
        self.temporary.cleanup()

    def _git(self, *arguments: str) -> str:
        return subprocess.run(
            ["git", "-C", str(self.repository), *arguments],
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()

    def _commit(self, value: str) -> None:
        (self.repository / "source.txt").write_text(value)
        self._git("add", "source.txt")
        self._git("commit", "-q", "-m", value)

    def test_exact_annotated_version_tag_is_accepted(self):
        self._git("tag", "-a", f"v{__version__}", "-m", "release")

        resolved = require_exact_version_tag(self.repository, __version__)

        self.assertEqual(resolved, self._git("rev-parse", "HEAD"))

    def test_missing_or_older_version_tag_is_rejected_with_explicit_bundle_guidance(self):
        guidance = f"exactly tagged v{__version__}.*explicit bundle"
        with self.assertRaisesRegex(RuntimeError, guidance):
            require_exact_version_tag(self.repository, __version__)
        self._git("tag", f"v{__version__}")
        self._commit("second")
        with self.assertRaisesRegex(RuntimeError, guidance):
            require_exact_version_tag(self.repository, __version__)

    def test_non_repository_is_rejected_with_release_guidance(self):
        with tempfile.TemporaryDirectory() as temporary, self.assertRaisesRegex(
            RuntimeError, "published release tag.*OMASHEETS_NATIVE_BUNDLE_PATH",
        ):
            require_exact_version_tag(Path(temporary), __version__)

    def test_automatic_download_checks_release_boundary_before_network_access(self):
        with tempfile.TemporaryDirectory() as temporary, patch(
            "omasheets.native_bundle.require_exact_version_tag",
            side_effect=RuntimeError("release boundary"),
        ) as require, patch("omasheets.native_bundle._download") as network:
            with self.assertRaisesRegex(RuntimeError, "release boundary"):
                download_native_bundle(
                    __version__, Path(temporary) / "cache", source_root=ROOT,
                )
        require.assert_called_once_with(ROOT, __version__)
        network.assert_not_called()


class ReleaseCheckerTests(unittest.TestCase):
    def test_source_tree_validation_does_not_require_a_release_tag(self):
        output = StringIO()
        with patch.object(check_release, "require_exact_version_tag") as require, redirect_stdout(output):
            self.assertEqual(check_release.main([]), 0)
        require.assert_not_called()
        self.assertIn("source tree contract ok", output.getvalue())

    def test_opt_in_release_gate_requires_the_exact_product_tag(self):
        output = StringIO()
        with patch.object(check_release, "require_exact_version_tag") as require, redirect_stdout(output):
            self.assertEqual(check_release.main(["--require-exact-version-tag"]), 0)
        require.assert_called_once_with(check_release.ROOT, __version__)
        self.assertIn("exact version tag contract ok", output.getvalue())

    def test_opt_in_release_gate_fails_cleanly(self):
        error = StringIO()
        with patch.object(
            check_release, "require_exact_version_tag", side_effect=RuntimeError("not released"),
        ), redirect_stdout(StringIO()), redirect_stderr(error):
            self.assertEqual(check_release.main(["--require-exact-version-tag"]), 1)
        self.assertIn("exact version tag contract failed: not released", error.getvalue())


if __name__ == "__main__":
    unittest.main()
