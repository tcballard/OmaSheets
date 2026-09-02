from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

from scripts import generate_m0_xlsx


class M0FixtureTests(unittest.TestCase):
    def test_generation_is_deterministic_and_reports_exact_counts(self):
        with tempfile.TemporaryDirectory() as temporary:
            first = Path(temporary) / "first.xlsx"
            second = Path(temporary) / "second.xlsx"
            output = StringIO()
            with redirect_stdout(output):
                self.assertEqual(generate_m0_xlsx.main([str(first), "--rows", "3"]), 0)
            with redirect_stdout(StringIO()):
                self.assertEqual(generate_m0_xlsx.main([str(second), "--rows", "3"]), 0)

            report = json.loads(output.getvalue())
            self.assertEqual(first.read_bytes(), second.read_bytes())
            self.assertEqual(report["sha256"], generate_m0_xlsx.sha256(first))
            self.assertEqual(report["data_rows"], 3)
            self.assertEqual(report["header_cells"], 3)
            self.assertEqual(report["numeric_value_cells"], 6)
            self.assertEqual(report["formula_cells"], 3)
            self.assertEqual(report["logical_cells"], 12)

    def test_workbook_contains_minimal_ooxml_and_excel_formula_syntax(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "fixture.xlsx"
            generate_m0_xlsx.generate(workbook, 2)
            with zipfile.ZipFile(workbook) as archive:
                self.assertEqual(
                    archive.namelist(),
                    [
                        "[Content_Types].xml",
                        "_rels/.rels",
                        "xl/workbook.xml",
                        "xl/_rels/workbook.xml.rels",
                        "xl/worksheets/sheet1.xml",
                    ],
                )
                sheet = archive.read("xl/worksheets/sheet1.xml").decode()
            self.assertIn("<f>A2+B2</f><v>3</v>", sheet)
            self.assertIn("<f>A3+B3</f><v>6</v>", sheet)
            self.assertNotIn("of:=", sheet)

    def test_generation_refuses_to_replace_existing_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "fixture.xlsx"
            workbook.write_bytes(b"owned")
            with self.assertRaises(FileExistsError):
                generate_m0_xlsx.generate(workbook, 1)
            self.assertEqual(workbook.read_bytes(), b"owned")

    def test_row_bounds_are_enforced_by_the_command_line(self):
        with redirect_stderr(StringIO()):
            with self.assertRaises(SystemExit):
                generate_m0_xlsx.main(["fixture.xlsx", "--rows", "0"])
            with self.assertRaises(SystemExit):
                generate_m0_xlsx.main(["fixture.xlsx", "--rows", "100001"])


if __name__ == "__main__":
    unittest.main()
