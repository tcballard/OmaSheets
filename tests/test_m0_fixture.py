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

    def test_date_rows_add_a_styled_dates_sheet_with_excel_stored_values(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "dates.xlsx"
            report = generate_m0_xlsx.generate(workbook, 1, date_rows=3)
            self.assertEqual(report["date_rows"], 3)
            self.assertEqual(report["date_system"], "1900")
            self.assertEqual(report["date_value_cells"], 3)
            self.assertEqual(report["date_formula_cells"], 21)
            self.assertEqual(report["formula_cells"], 22)
            self.assertEqual(report["header_cells"], 11)
            self.assertEqual(report["logical_cells"], 38)
            with zipfile.ZipFile(workbook) as archive:
                names = archive.namelist()
                self.assertIn("xl/worksheets/sheet2.xml", names)
                self.assertIn("xl/styles.xml", names)
                book = archive.read("xl/workbook.xml").decode()
                styles = archive.read("xl/styles.xml").decode()
                sheet = archive.read("xl/worksheets/sheet2.xml").decode()
            self.assertIn('<sheet name="Dates" sheetId="2" r:id="rId2"/>', book)
            self.assertNotIn("date1904", book)
            self.assertIn('<xf numFmtId="14"', styles)
            # 2023-12-31 is serial 45291 and a Sunday; EDATE and EOMONTH both
            # land on 2024-01-31 (serial 45322).
            self.assertIn('<c r="A2" s="1"><v>45291</v></c>', sheet)
            self.assertIn("<f>YEAR(A2)</f><v>2023</v>", sheet)
            self.assertIn("<f>MONTH(A2)</f><v>12</v>", sheet)
            self.assertIn("<f>DAY(A2)</f><v>31</v>", sheet)
            self.assertIn('s="1"><f>EDATE(A2,1)</f><v>45322</v>', sheet)
            self.assertIn('s="1"><f>EOMONTH(A2,1)</f><v>45322</v>', sheet)
            self.assertIn('s="1"><f>DATE(B2,C2,D2)</f><v>45291</v>', sheet)
            self.assertIn("<f>WEEKDAY(A2)</f><v>1</v>", sheet)
            # 2024-01-31 (row 4) shows EDATE clamping to the leap day.
            self.assertIn('<c r="A4" s="1"><v>45322</v></c>', sheet)
            self.assertIn('s="1"><f>EDATE(A4,1)</f><v>45351</v>', sheet)

    def test_date_system_flag_only_marks_the_workbook_epoch(self):
        with tempfile.TemporaryDirectory() as temporary:
            workbook = Path(temporary) / "dates-1904.xlsx"
            report = generate_m0_xlsx.generate(
                workbook, 1, date_rows=1, date_system="1904"
            )
            self.assertEqual(report["date_system"], "1904")
            with zipfile.ZipFile(workbook) as archive:
                book = archive.read("xl/workbook.xml").decode()
            self.assertIn('<workbookPr date1904="1"/><sheets>', book)
            with self.assertRaises(ValueError):
                generate_m0_xlsx.payloads(1, 1, "1901")

    def test_default_report_declares_no_date_rows(self):
        with tempfile.TemporaryDirectory() as temporary:
            report = generate_m0_xlsx.generate(Path(temporary) / "plain.xlsx", 2)
            self.assertEqual(report["date_rows"], 0)
            self.assertEqual(report["date_formula_cells"], 0)
            self.assertEqual(report["formula_cells"], 2)
            self.assertEqual(report["date_system"], "1900")

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
            with self.assertRaises(SystemExit):
                generate_m0_xlsx.main(["fixture.xlsx", "--date-rows", "-1"])
            with self.assertRaises(SystemExit):
                generate_m0_xlsx.main(["fixture.xlsx", "--date-rows", "10001"])
            with self.assertRaises(SystemExit):
                generate_m0_xlsx.main(["fixture.xlsx", "--date-system", "1901"])


if __name__ == "__main__":
    unittest.main()
