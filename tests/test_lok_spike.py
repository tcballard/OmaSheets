from pathlib import Path
import json
import tempfile
import unittest
from unittest.mock import Mock, patch

from omasheets.errors import EngineError
from omasheets.lok_spike import render_workbook, status


class LibreOfficeKitSpikeTests(unittest.TestCase):
    def test_status_reports_the_production_install_boundary(self):
        with patch("omasheets.lok_spike.Path.is_dir", return_value=True), patch(
            "omasheets.lok_spike.Path.is_file", return_value=True
        ), patch("omasheets.lok_spike.os.access", return_value=True), patch(
            "omasheets.lok_spike.shutil.which", return_value="/usr/bin/omasheets-lok-render"
        ):
            result = status()
        self.assertFalse(result["experimental"])
        self.assertTrue(result["ready"])
        self.assertEqual(
            [check["name"] for check in result["checks"]],
            ["libreofficekit-program", "omasheets-lok-render"],
        )

    def test_render_uses_an_argv_vector_and_validates_the_report(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "budget $(touch nope).xls"
            destination = Path(temporary) / "tile.ppm"
            source.write_bytes(b"xls")

            def completed(*args, **kwargs):
                destination.write_bytes(b"P6\n1 1\n255\n\0\0\0")
                return Mock(returncode=0, stdout=json.dumps({"engine": "libreofficekit"}), stderr="")

            with patch("omasheets.lok_spike.renderer_executable", return_value=Path("/usr/bin/omasheets-lok-render")), patch(
                "omasheets.lok_spike.subprocess.run", side_effect=completed
            ) as run:
                report = render_workbook(source, destination, width=320, height=200)

            self.assertEqual(report["engine"], "libreofficekit")
            argv = run.call_args.args[0]
            self.assertEqual(argv, [
                "/usr/bin/omasheets-lok-render", str(source.resolve()), str(destination.resolve()), "320", "200"
            ])
            self.assertNotIn("shell", run.call_args.kwargs)

    def test_render_refuses_to_replace_an_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "book.xlsx"
            destination = Path(temporary) / "tile.ppm"
            source.write_bytes(b"xlsx")
            destination.write_bytes(b"existing")
            with self.assertRaises(EngineError):
                render_workbook(source, destination)
            self.assertEqual(destination.read_bytes(), b"existing")

    def test_render_bounds_dimensions(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "book.ods"
            source.write_bytes(b"ods")
            with self.assertRaises(EngineError):
                render_workbook(source, Path(temporary) / "tile.ppm", width=4097)


if __name__ == "__main__":
    unittest.main()
