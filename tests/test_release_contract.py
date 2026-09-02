from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
import hashlib
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from omasheets import __version__
from omasheets.native_bundle import (
    MAX_SIDECAR_BYTES,
    RELEASE_SIGNING_KEY,
    asset_name,
    download_native_bundle,
    load_release_public_key,
    require_exact_version_tag,
)
from omasheets.release_signing import format_public_key, parse_public_key, sign_file
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


class SignedDownloadTests(unittest.TestCase):
    """The download path verifies the pinned-key signature before the checksum."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.seed = os.urandom(32)
        self.key_id = os.urandom(8)
        self.checkout = self.root / "checkout"
        (self.checkout / RELEASE_SIGNING_KEY).parent.mkdir(parents=True)
        (self.checkout / RELEASE_SIGNING_KEY).write_text(format_public_key(self.seed, self.key_id))
        self.name = asset_name(__version__, system="linux", architecture="x86_64")
        self.release = self.root / "release"
        self.release.mkdir()
        self.archive = self.release / self.name
        self.archive.write_bytes(os.urandom(4096))
        (self.release / f"{self.name}.minisig").write_text(sign_file(self.archive, self.seed, self.key_id))
        (self.release / f"{self.name}.sha256").write_text(
            f"{hashlib.sha256(self.archive.read_bytes()).hexdigest()}  {self.name}\n"
        )
        self.requests: list[str] = []

    def tearDown(self):
        self.temporary.cleanup()

    def fake_download(self, url: str, destination: Path, *, limit: int = 0) -> None:
        self.requests.append(url.rsplit("/", 1)[1])
        source = self.release / url.rsplit("/", 1)[1]
        if not source.is_file():
            raise RuntimeError(f"HTTP 404: {url}")
        if limit and source.stat().st_size > limit:
            raise RuntimeError("sidecar too large")
        destination.write_bytes(source.read_bytes())

    def download(self):
        with patch("omasheets.native_bundle.require_exact_version_tag"), patch(
            "omasheets.native_bundle.platform_id", return_value="linux",
        ), patch("omasheets.native_bundle.normalized_architecture", return_value="x86_64"), patch(
            "omasheets.native_bundle._download", side_effect=self.fake_download,
        ):
            return download_native_bundle(__version__, self.root / "cache", source_root=self.checkout)

    def test_signed_bundle_is_verified_then_checksummed_and_sidecars_are_removed(self):
        archive = self.download()
        self.assertEqual(archive.read_bytes(), self.archive.read_bytes())
        self.assertEqual(self.requests, [self.name, f"{self.name}.minisig", f"{self.name}.sha256"])
        self.assertEqual(sorted(path.name for path in (self.root / "cache").iterdir()), [self.name])

    def test_missing_pinned_key_downloads_nothing(self):
        (self.checkout / RELEASE_SIGNING_KEY).unlink()
        with self.assertRaisesRegex(RuntimeError, "pinned release signing key.*missing"):
            self.download()
        self.assertEqual(self.requests, [])
        with self.assertRaisesRegex(RuntimeError, "pinned release signing key"):
            load_release_public_key(self.checkout)

    def test_unsigned_or_missigned_bundle_is_deleted_before_the_checksum_is_read(self):
        (self.release / f"{self.name}.minisig").unlink()
        with self.assertRaisesRegex(RuntimeError, "minisig"):
            self.download()
        self.assertEqual(self.requests, [self.name, f"{self.name}.minisig"])
        self.assertEqual(list((self.root / "cache").iterdir()), [])
        self.requests.clear()
        (self.release / f"{self.name}.minisig").write_text(sign_file(self.archive, os.urandom(32), self.key_id))
        with self.assertRaisesRegex(RuntimeError, "signature verification failed.*does not verify"):
            self.download()
        self.assertNotIn(f"{self.name}.sha256", self.requests)
        self.assertEqual(list((self.root / "cache").iterdir()), [])

    def test_signature_for_another_asset_or_key_is_rejected(self):
        other = self.release / "other.tar.gz"
        other.write_bytes(self.archive.read_bytes())
        (self.release / f"{self.name}.minisig").write_text(sign_file(other, self.seed, self.key_id))
        with self.assertRaisesRegex(RuntimeError, f"not bound to {re.escape(self.name)}"):
            self.download()
        (self.release / f"{self.name}.minisig").write_text(sign_file(self.archive, self.seed, os.urandom(8)))
        with self.assertRaisesRegex(RuntimeError, "does not match the pinned release key"):
            self.download()

    def test_tampered_archive_with_matching_checksum_still_fails_the_signature(self):
        self.archive.write_bytes(os.urandom(4096))
        (self.release / f"{self.name}.sha256").write_text(
            f"{hashlib.sha256(self.archive.read_bytes()).hexdigest()}  {self.name}\n"
        )
        with self.assertRaisesRegex(RuntimeError, "signature verification failed"):
            self.download()
        self.assertEqual(list((self.root / "cache").iterdir()), [])

    def test_sidecars_are_bounded(self):
        self.assertEqual(MAX_SIDECAR_BYTES, 8 * 1024)
        (self.release / f"{self.name}.minisig").write_text("untrusted comment: x\n" + "A" * 9000 + "\n")
        with self.assertRaisesRegex(RuntimeError, "too large"):
            self.download()
        self.assertEqual(list((self.root / "cache").iterdir()), [])


class ReleaseWorkflowPinTests(unittest.TestCase):
    def setUp(self):
        self.workflow = (ROOT / ".github/workflows/release.yml").read_text()

    def test_every_action_image_and_package_input_is_pinned(self):
        check_release.check_release_workflow(self.workflow)
        uses = re.findall(r"uses:\s*(\S+)", self.workflow)
        self.assertGreaterEqual(len(uses), 4)
        for reference in uses:
            self.assertRegex(reference, r"^[\w.-]+/[\w.-]+@[0-9a-f]{40}$")
        self.assertIn("archlinux:base-devel@sha256:", self.workflow)
        self.assertIn("attestations: write", self.workflow)
        self.assertIn("id-token: write", self.workflow)
        self.assertNotIn("secrets.", self.workflow)

    def test_checker_rejects_unpinned_inputs(self):
        for broken, message in (
            (self.workflow.replace("@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1", "@v7"), "full commit SHA"),
            (re.sub(r"image: archlinux:base-devel@sha256:[0-9a-f]{64}", "image: archlinux:base-devel", self.workflow), "pinned by digest"),
            (self.workflow.replace("pacman -Syyuu", "pacman -Syu"), "converge on the snapshot"),
            (self.workflow.replace("archive.archlinux.org/repos/", "mirror.example/"), "Archive snapshot"),
            (self.workflow.replace("actions/attest-build-provenance@", "actions/other@"), "not attested"),
            (self.workflow + "        env:\n          KEY: ${{ secrets.SIGNING_KEY }}\n", "no signing secret"),
        ):
            with self.subTest(message=message), self.assertRaisesRegex(AssertionError, message):
                check_release.check_release_workflow(broken)

    def test_pinned_key_when_present_must_parse(self):
        path = ROOT / RELEASE_SIGNING_KEY
        if path.is_file():
            parse_public_key(path.read_text())
        else:
            with self.assertRaisesRegex(AssertionError, "requires the pinned signing key"):
                check_release.check_pinned_release_key(True)
            check_release.check_pinned_release_key(False)


class ReleaseCheckerTests(unittest.TestCase):
    def test_source_tree_validation_does_not_require_a_release_tag(self):
        output = StringIO()
        with patch.object(check_release, "require_exact_version_tag") as require, redirect_stdout(output):
            self.assertEqual(check_release.main([]), 0)
        require.assert_not_called()
        self.assertIn("source tree contract ok", output.getvalue())

    def test_opt_in_release_gate_requires_the_exact_product_tag(self):
        output = StringIO()
        with patch.object(check_release, "require_exact_version_tag") as require, patch.object(
            check_release, "check_pinned_release_key",
        ) as pinned, redirect_stdout(output):
            self.assertEqual(check_release.main(["--require-exact-version-tag"]), 0)
        pinned.assert_called_once_with(True)
        require.assert_called_once_with(check_release.ROOT, __version__)
        self.assertIn("exact version tag contract ok", output.getvalue())

    def test_opt_in_release_gate_fails_cleanly(self):
        error = StringIO()
        with patch.object(
            check_release, "require_exact_version_tag", side_effect=RuntimeError("not released"),
        ), patch.object(check_release, "check_pinned_release_key"), redirect_stdout(StringIO()), redirect_stderr(error):
            self.assertEqual(check_release.main(["--require-exact-version-tag"]), 1)
        self.assertIn("exact version tag contract failed: not released", error.getvalue())


if __name__ == "__main__":
    unittest.main()
