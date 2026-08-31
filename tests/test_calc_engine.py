from pathlib import Path
import tempfile
import unittest

from omasheets.calc_engine import CalcConfig, CalcEngine, _copy_no_clobber
from omasheets.errors import ConflictError, EngineError
from omasheets.paths import AppPaths


class CalcEngineTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.paths = AppPaths(self.root / "state", self.root / "cache", self.root / "runtime")
        for name in ("bwrap", "python", "soffice", "worker.py"):
            (self.root / name).write_text("placeholder")
        self.config = CalcConfig(
            bwrap=self.root / "bwrap",
            python=self.root / "python",
            soffice=self.root / "soffice",
            worker=self.root / "worker.py",
        )

    def tearDown(self):
        self.temporary.cleanup()

    def test_sandbox_unshares_network_and_clears_environment(self):
        engine = CalcEngine(self.paths, config=self.config)
        job = self.root / "job"
        job.mkdir()
        command = engine._sandbox_command(job)
        self.assertIn("--unshare-all", command)
        self.assertIn("--clearenv", command)
        self.assertNotIn("--share-net", command)
        self.assertNotIn(str(Path.home()), command)
        self.assertIn("/omasheets-worker.py", command)

    def test_missing_bubblewrap_fails_closed(self):
        config = CalcConfig(
            bwrap=self.root / "missing",
            python=self.root / "python",
            soffice=self.root / "soffice",
            worker=self.root / "worker.py",
        )
        with self.assertRaises(EngineError):
            CalcEngine(self.paths, config=config)._sandbox_command(self.root)

    def test_artifact_copy_never_clobbers(self):
        source = self.root / "source"
        destination = self.root / "destination"
        source.write_bytes(b"new")
        destination.write_bytes(b"existing")
        with self.assertRaises(FileExistsError):
            _copy_no_clobber(source, destination)
        self.assertEqual(destination.read_bytes(), b"existing")

    def test_conversion_destination_cannot_be_redirected(self):
        engine = CalcEngine(self.paths, config=self.config)
        source = self.root / "legacy.xls"
        source.write_bytes(b"xls")
        with self.assertRaises(ConflictError):
            engine.convert_legacy(
                source,
                destination=self.root / "elsewhere.xlsx",
                preview=self.root / "preview.pdf",
            )


if __name__ == "__main__":
    unittest.main()
