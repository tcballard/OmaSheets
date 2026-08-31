import unittest

from omasheets.calc_worker import (
    _apply,
    _column_index,
    _color,
    _matrix_values,
    _named_ranges,
    _style_color,
    _style_table,
    _target_fingerprints,
)


class FakeArea:
    def __init__(self):
        self.values = (("", ""), ("", ""))
        self.formulas = (("", ""), ("", ""))
        self.CharWeight = 100.0
        self.CharColor = 0
        self.CellBackColor = 0
        self.IsTextWrapped = False
        self.fills = []
        self.sort_descriptor = None

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

    def fillAuto(self, direction, count):
        self.fills.append((direction, count))

    def createSortDescriptor(self):
        class Field:
            Field = 0
            IsAscending = True

        class Item:
            def __init__(self, name, value):
                self.Name = name
                self.Value = value

        return [Item("ContainsHeader", False), Item("SortFields", [Field()])]

    def sort(self, descriptor):
        self.sort_descriptor = descriptor


class FakeCollection:
    def __init__(self):
        self.calls = []

    def insertByIndex(self, index, count):
        self.calls.append(("insert", index, count))

    def removeByIndex(self, index, count):
        self.calls.append(("remove", index, count))


class FakeSheet:
    def __init__(self, area):
        self.area = area
        self.rows = FakeCollection()
        self.columns = FakeCollection()

    def getCellRangeByName(self, name):
        del name
        return self.area

    def getRows(self):
        return self.rows

    def getColumns(self):
        return self.columns


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


class FakeFormatProperties:
    def getPropertyValue(self, name):
        self.assert_name = name
        return "0.00%"


class FakeFormats:
    def getByKey(self, key):
        self.key = key
        return FakeFormatProperties()


class FakeStyleCell:
    CellStyle = "Default"
    NumberFormat = 7
    CharWeight = 150.0
    CharColor = -1
    CellBackColor = 0x112233
    IsTextWrapped = True


class FakeStyleArea:
    def getCellByPosition(self, column, row):
        del column, row
        return FakeStyleCell()


class FakeStyleDocument:
    def getNumberFormats(self):
        return FakeFormats()


class FakeNamedRange:
    def __init__(self, content):
        self.content = content

    def getContent(self):
        return self.content


class FakeNamedRanges:
    def __init__(self):
        self.items = {
            "External": FakeNamedRange("'file:///home/tom/private.xlsx'#$Sheet1.$A$1"),
            "Local": FakeNamedRange("$Sheet1.$B$2"),
        }

    def getElementNames(self):
        return tuple(self.items)

    def getByName(self, name):
        return self.items[name]


class FakeNamedDocument:
    def getNamedRanges(self):
        return FakeNamedRanges()


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

    def test_structural_operations_use_zero_based_uno_indexes(self):
        document = FakeDocument()
        _apply(document, [
            {"type": "insert_rows", "sheet": "Data", "row": 2, "count": 3},
            {"type": "delete_columns", "sheet": "Data", "column": "B", "count": 2},
        ])
        sheet = document.sheets.sheet
        self.assertEqual(sheet.rows.calls, [("insert", 1, 3)])
        self.assertEqual(sheet.columns.calls, [("remove", 1, 2)])
        self.assertEqual(_column_index("XFD"), 16383)

    def test_bounded_sort_descriptor_uses_one_relative_key(self):
        document = FakeDocument()
        _apply(document, [{
            "type": "sort_range", "sheet": "Data", "range": "A1:C9",
            "key_column": 2, "ascending": False, "has_header": True,
        }])
        descriptor = document.area.sort_descriptor
        self.assertTrue(descriptor[0].Value)
        self.assertEqual(descriptor[1].Value[0].Field, 1)
        self.assertFalse(descriptor[1].Value[0].IsAscending)

    def test_styles_are_deduplicated_and_automatic_colours_are_explicit(self):
        table = _style_table(FakeStyleDocument(), FakeStyleArea(), 2, 2)
        self.assertEqual(len(table["styles"]), 1)
        self.assertEqual(table["style_ids"], [[0, 0], [0, 0]])
        self.assertEqual(table["styles"][0]["text_color"], "automatic")
        self.assertEqual(_style_color(0xAABBCC), "#AABBCC")

    def test_named_ranges_are_bounded_and_external_paths_are_redacted(self):
        result = _named_ranges(FakeNamedDocument(), 10)
        self.assertEqual(result["total"], 2)
        self.assertTrue(result["items"][0]["content_redacted"])
        self.assertNotIn("/home/tom", str(result))
        self.assertEqual(result["items"][1]["content"], "$Sheet1.$B$2")


if __name__ == "__main__":
    unittest.main()
