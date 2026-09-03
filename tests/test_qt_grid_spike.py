import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CHECKER = ROOT / "scripts/check_qt_grid_spike.py"


class QtGridSpikeTests(unittest.TestCase):
    def test_static_contract(self):
        subprocess.run([sys.executable, CHECKER], cwd=ROOT, check=True)

    def test_bounded_report_contract(self):
        report = {
            "schema": 1,
            "fixture": "synthetic-1000000x64",
            "rows": 1_000_000,
            "columns": 64,
            "frames": 180,
            "elapsed_seconds": 3.0,
            "p95_frame_ms": 16.1,
            "worst_frame_ms": 22.0,
            "visible_delegates": 540,
            "cell_reads": 4_200,
            "startup_to_report_ms": 3_500.0,
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.txt"
            path.write_text("OMASHEETS_GRID_BENCHMARK " + json.dumps(report) + "\n")
            subprocess.run([sys.executable, CHECKER, "--report", path], cwd=ROOT, check=True)


if __name__ == "__main__":
    unittest.main()
