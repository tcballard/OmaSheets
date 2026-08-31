import json
import hashlib
import os
from pathlib import Path
from subprocess import CompletedProcess
import tempfile
import unittest
from unittest.mock import patch

from omasheets.calc_engine import (
    CalcConfig,
    CalcEngine,
    _copy_no_clobber,
    _copy_stable_input_no_clobber,
    _runtime_path_arguments,
)
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

    def test_job_input_copy_reads_source_once_and_preserves_bytes(self):
        source = self.root / "source.xlsx"
        destination = self.root / "job" / "input.xlsx"
        content = (b"one-pass-workbook" * 100_000) + b"end"
        source.write_bytes(content)
        real_open = os.open
        real_read = os.read
        source_opens = []
        source_descriptors = set()
        source_read_bytes = 0

        def observe_open(path, flags, mode=0o777, *, dir_fd=None):
            if Path(path) == source:
                source_opens.append(flags)
            if dir_fd is None:
                descriptor = real_open(path, flags, mode)
            else:
                descriptor = real_open(path, flags, mode, dir_fd=dir_fd)
            if Path(path) == source:
                source_descriptors.add(descriptor)
            return descriptor

        def observe_read(descriptor, length):
            nonlocal source_read_bytes
            chunk = real_read(descriptor, length)
            if descriptor in source_descriptors:
                source_read_bytes += len(chunk)
            return chunk

        with (
            patch("omasheets.calc_engine.os.open", side_effect=observe_open),
            patch("omasheets.calc_engine.os.read", side_effect=observe_read),
        ):
            identity = _copy_stable_input_no_clobber(source, destination)

        self.assertEqual(len(source_opens), 1)
        self.assertEqual(source_read_bytes, len(content))
        self.assertEqual(destination.read_bytes(), content)
        self.assertEqual(identity.size, len(content))
        self.assertEqual(identity.sha256, hashlib.sha256(content).hexdigest())

    def test_job_input_copy_never_clobbers_existing_destination(self):
        source = self.root / "source.xlsx"
        destination = self.root / "input.xlsx"
        source.write_bytes(b"new")
        destination.write_bytes(b"existing")

        with self.assertRaises(FileExistsError):
            _copy_stable_input_no_clobber(source, destination)

        self.assertEqual(destination.read_bytes(), b"existing")

    def test_job_input_copy_rejects_source_mutation_and_removes_partial_copy(self):
        source = self.root / "source.xlsx"
        destination = self.root / "job" / "input.xlsx"
        source.write_bytes(b"original workbook")
        real_read = os.read
        changed = False

        def mutate_after_read(descriptor, length):
            nonlocal changed
            chunk = real_read(descriptor, length)
            if chunk and not changed:
                changed = True
                source.write_bytes(b"mutated workbook with a different size")
            return chunk

        with patch("omasheets.calc_engine.os.read", side_effect=mutate_after_read):
            with self.assertRaisesRegex(ConflictError, "workbook changed"):
                _copy_stable_input_no_clobber(source, destination)

        self.assertTrue(changed)
        self.assertFalse(destination.exists())

    def test_job_input_copy_rejects_same_size_mutation_with_restored_mtime(self):
        source = self.root / "source.xlsx"
        destination = self.root / "job" / "input.xlsx"
        source.write_bytes(b"A" * (2 * 1024 * 1024))
        original = source.stat()
        real_read = os.read
        changed = False

        def mutate_after_read(descriptor, length):
            nonlocal changed
            chunk = real_read(descriptor, length)
            if chunk and not changed:
                changed = True
                source.write_bytes(b"B" * original.st_size)
                os.utime(source, ns=(original.st_atime_ns, original.st_mtime_ns))
            return chunk

        with patch("omasheets.calc_engine.os.read", side_effect=mutate_after_read):
            with self.assertRaisesRegex(ConflictError, "workbook changed"):
                _copy_stable_input_no_clobber(source, destination)

        self.assertTrue(changed)
        self.assertFalse(destination.exists())

    def test_job_input_copy_failure_does_not_unlink_replacement_destination(self):
        source = self.root / "source.xlsx"
        destination = self.root / "job" / "input.xlsx"
        displaced = self.root / "job" / "displaced.xlsx"
        source.write_bytes(b"original workbook")
        real_read = os.read
        swapped = False

        def swap_destination_and_mutate_source(descriptor, length):
            nonlocal swapped
            chunk = real_read(descriptor, length)
            if chunk and not swapped:
                swapped = True
                destination.rename(displaced)
                destination.write_bytes(b"replacement")
                source.write_bytes(b"mutated workbook with a different size")
            return chunk

        with patch(
            "omasheets.calc_engine.os.read",
            side_effect=swap_destination_and_mutate_source,
        ):
            with self.assertRaisesRegex(ConflictError, "workbook changed"):
                _copy_stable_input_no_clobber(source, destination)

        self.assertTrue(swapped)
        self.assertEqual(destination.read_bytes(), b"replacement")
        self.assertTrue(displaced.exists())

    def test_job_input_copy_rejects_source_swap_and_removes_partial_copy(self):
        source = self.root / "source.xlsx"
        displaced = self.root / "displaced.xlsx"
        destination = self.root / "job" / "input.xlsx"
        source.write_bytes(b"original workbook")
        real_read = os.read
        swapped = False

        def swap_after_read(descriptor, length):
            nonlocal swapped
            chunk = real_read(descriptor, length)
            if chunk and not swapped:
                swapped = True
                source.rename(displaced)
                source.write_bytes(b"replacement workbook")
            return chunk

        with patch("omasheets.calc_engine.os.read", side_effect=swap_after_read):
            with self.assertRaisesRegex(ConflictError, "workbook changed"):
                _copy_stable_input_no_clobber(source, destination)

        self.assertTrue(swapped)
        self.assertFalse(destination.exists())

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
