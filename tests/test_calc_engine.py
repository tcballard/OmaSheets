import json
from pathlib import Path
from subprocess import CompletedProcess
import tempfile
import unittest

from omasheets.calc_engine import CalcConfig, CalcEngine, _copy_no_clobber, _runtime_path_arguments
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
        self.assertEqual(command[command.index("SAL_USE_VCLPLUGIN") + 1], "svp")
        if Path("/etc/passwd").exists():
            self.assertIn("/etc/passwd", command)

    def test_missing_bubblewrap_fails_closed(self):
        config = CalcConfig(
            bwrap=self.root / "missing",
            python=self.root / "python",
            soffice=self.root / "soffice",
            worker=self.root / "worker.py",
        )
        with self.assertRaises(EngineError):
            CalcEngine(self.paths, config=config)._sandbox_command(self.root)

    def test_sandbox_recreates_loader_symlinks_and_binds_split_runtime_paths(self):
        merged = self.root / "lib64"
        merged.symlink_to("usr/lib")
        split = self.root / "lib"
        split.mkdir()
        self.assertEqual(_runtime_path_arguments((merged, split)), [
            "--symlink", "usr/lib", str(merged),
            "--ro-bind", str(split), str(split),
        ])

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

    def test_worker_failure_uses_only_its_bounded_structured_error(self):
        source = self.root / "book.xlsx"
        source.write_bytes(b"workbook")

        def fail_worker(command, **options):
            del command
            (Path(options["cwd"]) / "result.json").write_text(
                '{"ok":false,"error":"RuntimeError: bounded worker detail"}'
            )
            from subprocess import CompletedProcess

            return CompletedProcess([], 1, stdout="ignored", stderr="secret stderr")

        engine = CalcEngine(self.paths, config=self.config, runner=fail_worker)
        with self.assertRaisesRegex(EngineError, "bounded worker detail") as raised:
            engine.describe(source, include_formulas=False)
        self.assertNotIn("secret stderr", str(raised.exception))

    def test_read_query_batch_is_one_worker_job(self):
        source = self.root / "book.xlsx"
        source.write_bytes(b"workbook")
        requests = []

        def complete_worker(command, **options):
            del command
            job = Path(options["cwd"])
            requests.append(json.loads((job / "request.json").read_text()))
            (job / "result.json").write_text(json.dumps({
                "ok": True,
                "result": {"items": [
                    {"id": "structure", "tool": "describe_workbook", "result": {"sheet_count": 1}},
                    {"id": "cells", "tool": "read_range", "result": {"values": [[1]]}},
                ]},
                "artifacts": {},
            }))
            return CompletedProcess([], 0, stdout="", stderr="")

        queries = [
            {"id": "structure", "tool": "describe_workbook", "arguments": {"include_formulas": False}},
            {
                "id": "cells", "tool": "read_range",
                "arguments": {
                    "sheet": "Sheet1", "range": "A1",
                    "include_formulas": True, "include_styles": False,
                },
            },
        ]
        engine = CalcEngine(self.paths, config=self.config, runner=complete_worker)
        result = engine.query(source, queries)

        self.assertEqual(len(requests), 1)
        self.assertEqual(requests[0]["action"], "query")
        self.assertEqual(requests[0]["arguments"], {"queries": queries})
        self.assertEqual([item["id"] for item in result["items"]], ["structure", "cells"])


if __name__ == "__main__":
    unittest.main()
