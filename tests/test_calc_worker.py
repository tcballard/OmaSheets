import unittest

from omasheets.calc_worker import _apply, _color, _matrix_values, _target_fingerprints


class FakeArea:
    def __init__(self):
        self.values = (("", ""), ("", ""))
        self.formulas = (("", ""), ("", ""))
        self.CharWeight = 100.0
        self.CharColor = 0
        self.CellBackColor = 0
        self.IsTextWrapped = False

    def clearContents(self, flags):
        del flags
        self.values = (("", ""), ("", ""))
        self.formulas = (("", ""), ("", ""))

    def setDataArray(self, values):
        self.values = values
        self.formulas = values

    def setFormulaArray(self, formulas):
        self.formulas = formulas
        self.values = formulas

    def getDataArray(self):
        return self.values

    def getFormulaArray(self):
        return self.formulas


class FakeSheet:
    def __init__(self, area):
        self.area = area

    def getCellRangeByName(self, name):
        del name
        return self.area


class FakeSheets:
    def __init__(self, sheet):
        self.sheet = sheet

    def hasByName(self, name):
        return name == "Data"

    def getByName(self, name):
        if not self.hasByName(name):
            raise KeyError(name)
        return self.sheet


class FakeDocument:
    def __init__(self):
        self.area = FakeArea()
        self.sheets = FakeSheets(FakeSheet(self.area))

    def getSheets(self):
        return self.sheets


class CalcWorkerTests(unittest.TestCase):
    def test_bulk_values_are_typed_for_uno(self):
        self.assertEqual(
            _matrix_values([[None, True, False, 2, 2.5, "=literal"]]),
            (("", 1.0, 0.0, 2.0, 2.5, "=literal"),),
        )

    def test_bulk_and_format_operations_have_reopen_fingerprints(self):
        document = FakeDocument()
        operations = [
            {
                "type": "set_range_values",
                "sheet": "Data",
                "range": "A1:B2",
                "values": [[1, 2], [3, 4]],
            },
            {
                "type": "format_cells",
                "sheet": "Data",
                "range": "A1:B2",
                "bold": True,
                "text_color": "#112233",
                "background_color": "#AABBCC",
                "wrap_text": True,
            },
        ]
        _apply(document, operations)
        self.assertEqual(document.area.values, ((1.0, 2.0), (3.0, 4.0)))
        self.assertEqual(_color("#AABBCC"), 0xAABBCC)
        fingerprints = _target_fingerprints(document, operations)
        self.assertEqual(fingerprints[0]["format"], {
            "bold": True,
            "text_color": "#112233",
            "background_color": "#AABBCC",
            "wrap_text": True,
        })

    def test_bulk_formula_matrix_is_passed_without_rewriting(self):
        document = FakeDocument()
        operations = [{
            "type": "set_range_formulas",
            "sheet": "Data",
            "range": "A1:B2",
            "formulas": [["=1+1", "=2+2"], ["=3+3", "=4+4"]],
        }]
        _apply(document, operations)
        self.assertEqual(document.area.formulas[1][1], "=4+4")


if __name__ == "__main__":
    unittest.main()
