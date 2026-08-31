import unittest
from types import SimpleNamespace
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from omasheets.calc_worker import (
    _analyze,
    _apply,
    _column_index,
    _color,
    _fill_direction,
    _inspect,
    _matrix_values,
    _named_ranges,
    _object_fingerprints,
    _query_workbook,
    _style_color,
    _style_table,
    _startup_diagnostic,
    _search,
    _target_fingerprints,
    run,
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
    NamedRanges = FakeNamedRanges()


class FakeLegacyNamedDocument:
    def getNamedRanges(self):
        return FakeNamedRanges()


class CountingArea:
    def __init__(self, values, formulas=None, *, cell_errors=None, cell_strings=None):
        self.values = tuple(tuple(row) for row in values)
        self.formulas = tuple(tuple(row) for row in (formulas or values))
        self.cell_errors = dict(cell_errors or {})
        self.cell_strings = dict(cell_strings or {})
        self.data_reads = 0
        self.formula_reads = 0

    def getDataArray(self):
        self.data_reads += 1
        return self.values

    def getFormulaArray(self):
        self.formula_reads += 1
        return self.formulas

    def getCellByPosition(self, column, row):
        value = self.values[row][column]
        key = (column, row)
        return SimpleNamespace(
            getError=lambda: self.cell_errors.get(key, 0),
            getString=lambda: self.cell_strings.get(key, str(value)),
        )


class CountingSheet:
    def __init__(self, area, rows, columns):
        self.area = area
        self.address = SimpleNamespace(EndRow=rows - 1, EndColumn=columns - 1)

    def createCursor(self):
        address = self.address
        return SimpleNamespace(gotoEndOfUsedArea=lambda expand: None, getRangeAddress=lambda: address)

    def getCellRangeByPosition(self, start_column, start_row, end_column, end_row):
        self.requested_range = (start_column, start_row, end_column, end_row)
        return self.area

    def getCharts(self):
        return SimpleNamespace(getElementNames=lambda: ())

    def getDataPilotTables(self):
        return SimpleNamespace(getElementNames=lambda: ())


class CountingWorkbook:
    NamedRanges = FakeNamedRanges()

    def __init__(self, items):
        self.items = dict(items)
        self.sheets = SimpleNamespace(
            getElementNames=lambda: tuple(self.items),
            getByName=lambda name: self.items[name],
        )

    def getSheets(self):
        return self.sheets


class CalcWorkerTests(unittest.TestCase):
    def test_read_query_batch_preserves_order_and_uses_each_bounded_handler(self):
        document = object()
        limits = {"max_sheets": 10, "max_cells": 100, "max_formulas": 20, "max_results": 20}
        queries = [
            {"id": "structure", "tool": "describe_workbook", "arguments": {}},
            {"id": "cells", "tool": "read_range", "arguments": {"sheet": "Data", "range": "A1:B2"}},
            {"id": "needle", "tool": "search_workbook", "arguments": {"query": "total"}},
            {"id": "formula", "tool": "trace_formula", "arguments": {"sheet": "Data", "cell": "B2"}},
        ]
        with (
            patch("omasheets.calc_worker._inspect", return_value={"sheet_count": 1}) as inspect,
            patch("omasheets.calc_worker._read_range", return_value={"values": [[1]]}) as read,
            patch("omasheets.calc_worker._search", return_value={"matches": []}) as search,
            patch("omasheets.calc_worker._trace", return_value={"precedents": []}) as trace,
        ):
            result = _query_workbook(document, queries, limits)

        self.assertEqual(
            [(item["id"], item["tool"]) for item in result["items"]],
            [(item["id"], item["tool"]) for item in queries],
        )
        inspect.assert_called_once_with(document, limits, include_formulas=False)
        self.assertEqual(read.call_args.args[1], {
            "sheet": "Data", "range": "A1:B2",
            "include_formulas": True, "include_styles": False,
        })
        self.assertEqual(search.call_args.args[1], {
            "query": "total", "scope": "both", "max_results": 50,
        })
        self.assertEqual(trace.call_args.args[1], {
            "sheet": "Data", "cell": "B2", "direction": "both", "max_depth": 5,
        })

    def test_read_query_batch_preflights_every_item_before_reading(self):
        queries = [
            {"id": "valid", "tool": "describe_workbook", "arguments": {}},
            {
                "id": "invalid", "tool": "read_range",
                "arguments": {"sheet": "Data", "range": "A1", "session_id": "a" * 32},
            },
        ]
        with patch("omasheets.calc_worker._inspect") as inspect:
            with self.assertRaisesRegex(RuntimeError, "item 2 has invalid arguments"):
                _query_workbook(object(), queries, {})
        inspect.assert_not_called()

    def test_read_query_batch_discards_partial_results_when_a_query_fails(self):
        queries = [
            {"id": "structure", "tool": "describe_workbook", "arguments": {}},
            {"id": "cells", "tool": "read_range", "arguments": {"sheet": "Data", "range": "A1"}},
            {"id": "later", "tool": "search_workbook", "arguments": {"query": "x"}},
        ]
        with (
            patch("omasheets.calc_worker._inspect", return_value={"sheet_count": 1}),
            patch("omasheets.calc_worker._read_range", side_effect=RuntimeError("read failed")),
            patch("omasheets.calc_worker._search") as search,
        ):
            with self.assertRaisesRegex(RuntimeError, "read failed"):
                _query_workbook(object(), queries, {})
        search.assert_not_called()

    def test_read_query_action_loads_the_document_once_for_the_whole_batch(self):
        process = SimpleNamespace(terminate=lambda: None, wait=lambda timeout: None)
        document = SimpleNamespace(close=lambda deliver_ownership: None)
        expected = {"items": [{
            "id": "structure", "tool": "describe_workbook", "result": {"sheet_count": 1},
        }]}
        request = {
            "action": "query",
            "source": "input/workbook.xlsx",
            "arguments": {"queries": [{
                "id": "structure", "tool": "describe_workbook", "arguments": {},
            }]},
            "limits": {"max_sheets": 10, "max_cells": 100, "max_formulas": 20, "max_results": 20},
            "soffice": "/usr/bin/soffice",
        }
        with (
            patch.dict("sys.modules", {"uno": SimpleNamespace()}),
            patch("omasheets.calc_worker._connect", return_value=(process, object())),
            patch("omasheets.calc_worker._load", return_value=document) as load,
            patch("omasheets.calc_worker._query_workbook", return_value=expected) as query,
        ):
            result = run(request)

        load.assert_called_once()
        query.assert_called_once_with(document, request["arguments"]["queries"], request["limits"])
        self.assertEqual(result, {"result": expected, "artifacts": {}})

    def test_inspect_reads_only_formulas_and_preserves_formula_errors(self):
        area = CountingArea(
            (("Metric",), ("#DIV/0!",)),
            (("Metric",), ("=1/0",)),
            cell_errors={(0, 1): 532},
            cell_strings={(0, 1): "#DIV/0!"},
        )
        document = CountingWorkbook({"Data": CountingSheet(area, rows=2, columns=1)})
        limits = {"max_sheets": 10, "max_cells": 10, "max_formulas": 10, "max_results": 10}

        result = _inspect(document, limits, include_formulas=False)

        self.assertEqual((area.data_reads, area.formula_reads), (0, 1))
        self.assertEqual(result["formula_count"], 1)
        self.assertEqual(result["formula_errors"], [{
            "sheet": "Data", "row": 2, "column": 1, "formula": "=1/0",
            "error_code": 532, "displayed": "#DIV/0!",
        }])

    def test_analyze_preserves_formula_error_profiles_and_enforces_formula_limit(self):
        first = CountingArea(
            (("Metric",), ("#DIV/0!",)),
            (("Metric",), ("=1/0",)),
            cell_errors={(0, 1): 532},
            cell_strings={(0, 1): "#DIV/0!"},
        )
        document = CountingWorkbook({"Data": CountingSheet(first, rows=2, columns=1)})
        limits = {"max_sheets": 10, "max_cells": 10, "max_formulas": 1, "max_results": 10}

        result = _analyze(document, {"focus": "all", "max_findings": 50}, limits)

        self.assertEqual(result["summary"]["formula_count"], 1)
        self.assertEqual(result["summary"]["formula_error_count"], 1)
        self.assertEqual(result["sheets"][0]["columns"][0]["formula_cells"], 1)
        formula_findings = [item for item in result["findings"] if item["category"] == "formula_error"]
        self.assertEqual(len(formula_findings), 1)
        self.assertEqual(formula_findings[0]["range"], "A2")
        self.assertEqual(formula_findings[0]["metrics"], {"displayed": "#DIV/0!", "error_code": 532})

        second = CountingArea((("Other",), (2.0,)), (("Other",), ("=1+1",)))
        over_limit = CountingWorkbook({
            "Data": CountingSheet(first, rows=2, columns=1),
            "Other": CountingSheet(second, rows=2, columns=1),
        })
        with self.assertRaisesRegex(RuntimeError, "formula limit"):
            _analyze(over_limit, {"focus": "all", "max_findings": 50}, limits)

    def test_management_focus_keeps_numeric_outliers_but_gates_quality_findings(self):
        values = (("Amount",),) + tuple((value,) for value in (1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 100.0))
        area = CountingArea(values)
        document = CountingWorkbook({"Data": CountingSheet(area, rows=len(values), columns=1)})
        limits = {"max_sheets": 10, "max_cells": 100, "max_formulas": 10, "max_results": 10}

        result = _analyze(document, {"focus": "management", "max_findings": 50}, limits)

        self.assertEqual([item["category"] for item in result["findings"]], ["numeric_outliers"])
        self.assertEqual(result["findings"][0]["metrics"], {
            "count": 1, "low": 1.0, "high": 1.0, "examples": [100.0],
        })
        self.assertEqual(result["sheets"][0]["columns"][0]["sum"], 107.0)

    def test_duplicate_headers_are_trimmed_and_casefolded(self):
        area = CountingArea(((" Revenue ", "revenue"),))
        document = CountingWorkbook({"Data": CountingSheet(area, rows=1, columns=2)})
        limits = {"max_sheets": 10, "max_cells": 10, "max_formulas": 10, "max_results": 10}

        result = _analyze(document, {"focus": "all", "max_findings": 50}, limits)

        duplicate = [item for item in result["findings"] if item["category"] == "duplicate_header"]
        self.assertEqual(len(duplicate), 1)
        self.assertEqual(duplicate[0]["metrics"], {"headers": ["revenue"]})

    def test_distinct_profile_samples_only_the_first_ten_thousand_populated_cells(self):
        values = (("Identifier",),) + tuple((f"id-{index}",) for index in range(10_001))
        area = CountingArea(values)
        document = CountingWorkbook({"Data": CountingSheet(area, rows=len(values), columns=1)})
        limits = {"max_sheets": 10, "max_cells": 20_000, "max_formulas": 10, "max_results": 10}

        result = _analyze(document, {"focus": "management", "max_findings": 50}, limits)

        profile = result["sheets"][0]["columns"][0]
        self.assertEqual(profile["populated"], 10_001)
        self.assertEqual(profile["distinct"], 10_000)

    def test_search_rejects_an_over_budget_workbook_before_materializing_any_cells(self):
        first = CountingArea((("needle", ""),))
        second = CountingArea((("", "", ""),))
        document = CountingWorkbook({
            "First": CountingSheet(first, rows=1, columns=2),
            "Second": CountingSheet(second, rows=1, columns=3),
        })
        limits = {"max_sheets": 10, "max_cells": 4, "max_results": 20}

        with self.assertRaisesRegex(RuntimeError, "inspected-cell limit"):
            _search(document, {"query": "needle", "scope": "both"}, limits)

        self.assertEqual((first.data_reads, first.formula_reads), (0, 0))
        self.assertEqual((second.data_reads, second.formula_reads), (0, 0))

    def test_search_enforces_sheet_limit_before_reads_and_accepts_exact_cell_budget(self):
        first = CountingArea((("needle", ""),))
        second = CountingArea((("", "", ""),))
        document = CountingWorkbook({
            "First": CountingSheet(first, rows=1, columns=2),
            "Second": CountingSheet(second, rows=1, columns=3),
        })

        with self.assertRaisesRegex(RuntimeError, "sheet limit"):
            _search(document, {"query": "needle", "scope": "both"}, {
                "max_sheets": 1, "max_cells": 5, "max_results": 20,
            })
        self.assertEqual((first.data_reads, second.data_reads), (0, 0))

        result = _search(document, {"query": "needle", "scope": "both"}, {
            "max_sheets": 2, "max_cells": 5, "max_results": 20,
        })
        self.assertFalse(result["truncated"])
        self.assertEqual(result["matches"][0]["sheet"], "First")
        self.assertEqual((first.data_reads, second.data_reads), (1, 1))

    def test_search_returns_deterministic_matches_and_truncates_at_result_limit(self):
        area = CountingArea(
            (("Needle one", "plain"), ("needle two", "value")),
            (("Needle one", "=\"needle formula\""), ("needle two", "value")),
        )
        document = CountingWorkbook({"Data": CountingSheet(area, rows=2, columns=2)})

        result = _search(document, {"query": "NEEDLE", "scope": "both", "max_results": 2}, {
            "max_sheets": 1, "max_cells": 4, "max_results": 10,
        })

        self.assertTrue(result["truncated"])
        self.assertEqual(result["matches"], [
            {"sheet": "Data", "row": 1, "column": 1, "value": "Needle one", "formula": None},
            {"sheet": "Data", "row": 1, "column": 2, "value": "plain", "formula": "=\"needle formula\""},
        ])

    def test_analyze_materializes_each_sheet_once_and_preserves_profile_findings(self):
        area = CountingArea(
            (
                ("Region", "Revenue"),
                ("North", 10.0),
                ("North", 10.0),
                ("South", ""),
            ),
            (
                ("Region", "Revenue"),
                ("North", "10"),
                ("North", "10"),
                ("South", ""),
            ),
        )
        document = CountingWorkbook({"Data": CountingSheet(area, rows=4, columns=2)})
        limits = {"max_sheets": 10, "max_cells": 100, "max_formulas": 20, "max_results": 20}

        result = _analyze(document, {"focus": "all", "max_findings": 50}, limits)

        self.assertEqual((area.data_reads, area.formula_reads), (1, 1))
        self.assertEqual(result["summary"], {
            "sheet_count": 1,
            "inspected_cells": 8,
            "data_rows": 3,
            "formula_count": 0,
            "formula_error_count": 0,
            "finding_count": 2,
            "finding_total": 2,
            "truncated": False,
        })
        self.assertEqual(
            [finding["category"] for finding in result["findings"]],
            ["duplicate_rows", "sparse_column"],
        )
        self.assertEqual(result["findings"][0]["metrics"], {
            "duplicate_count": 1,
            "examples": [{"row": 3, "matches_row": 2}],
        })
        self.assertEqual(result["sheets"][0]["columns"][1], {
            "column": "B",
            "header": "Revenue",
            "populated": 2,
            "blanks": 1,
            "distinct": 1,
            "numeric": 2,
            "formula_cells": 0,
            "min": 10.0,
            "max": 10.0,
            "sum": 20.0,
            "mean": 10.0,
        })

    def test_xlsx_embedded_chart_fallback_uses_the_visible_title(self):
        model = SimpleNamespace(HasMainTitle=True, Title=SimpleNamespace(String="Revenue by region"))

        class Charts:
            def hasByName(self, name):
                del name
                return False

        class DrawPage:
            def getCount(self):
                return 1

            def getByIndex(self, index):
                del index
                return SimpleNamespace(Model=model)

        sheet = SimpleNamespace(getCharts=lambda: Charts(), getDrawPage=lambda: DrawPage())
        def get_sheet(name):
            if name != "Summary":
                raise AssertionError("non-object operations must not resolve target sheets")
            return sheet

        sheets = SimpleNamespace(getByName=get_sheet, hasByName=lambda name: name == "Summary")
        document = SimpleNamespace(getSheets=lambda: sheets)
        result = _object_fingerprints(document, [{"type": "add_sheet", "sheet": "New"}, {
            "type": "upsert_chart", "sheet": "Summary", "name": "RevenueChart",
            "title": "Revenue by region",
        }])
        self.assertEqual(result["charts"], [{
            "sheet": "Summary", "name": "RevenueChart", "title": "Revenue by region",
        }])

    def test_xlsx_pivot_header_shift_normalizes_to_requested_anchor(self):
        source = SimpleNamespace(Sheet=0, StartColumn=3, StartRow=0, EndColumn=5, EndRow=3)
        anchor = SimpleNamespace(Sheet=1, Column=0, Row=1)

        def fingerprint(output_row):
            output = SimpleNamespace(Sheet=1, StartColumn=0, StartRow=output_row)
            table = SimpleNamespace(getSourceRange=lambda: source, getOutputRange=lambda: output)
            tables = SimpleNamespace(
                hasByName=lambda name: name == "RevenuePivot",
                getByName=lambda name: table,
            )
            sheet = SimpleNamespace(
                getDataPilotTables=lambda: tables,
                getCellRangeByName=lambda name: SimpleNamespace(getCellAddress=lambda: anchor),
            )
            sheets = SimpleNamespace(
                hasByName=lambda name: name == "Summary",
                getByName=lambda name: sheet,
            )
            document = SimpleNamespace(getSheets=lambda: sheets)
            return _object_fingerprints(document, [{
                "type": "upsert_pivot", "sheet": "Summary", "name": "RevenuePivot",
                "output_cell": "A2",
            }])["pivots"]

        expected = [{
            "sheet": "Summary", "name": "RevenuePivot",
            "source": [0, 3, 0, 5, 3], "output_start": [1, 0, 1],
        }]
        self.assertEqual(fingerprint(1), expected)
        self.assertEqual(fingerprint(2), expected)
        self.assertEqual(fingerprint(3)[0]["output_start"], [1, 0, 3])

    def test_fill_directions_are_uno_enums_not_constants(self):
        fake_uno = SimpleNamespace(Enum=lambda type_name, member: (type_name, member))
        with patch.dict("sys.modules", {"uno": fake_uno}):
            self.assertEqual(
                _fill_direction("TO_BOTTOM"),
                ("com.sun.star.sheet.FillDirection", "TO_BOTTOM"),
            )

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
        fake_uno = SimpleNamespace(createUnoStruct=lambda name: SimpleNamespace(type_name=name))
        with patch.dict("sys.modules", {"uno": fake_uno}):
            _apply(document, [{
                "type": "sort_range", "sheet": "Data", "range": "A1:C9",
                "key_column": 2, "ascending": False, "has_header": True,
            }])
        descriptor = document.area.sort_descriptor
        self.assertTrue(descriptor[0].Value)
        self.assertEqual(descriptor[1].Value[0].Field, 1)
        self.assertFalse(descriptor[1].Value[0].SortAscending)
        self.assertEqual(descriptor[1].Value[0].type_name, "com.sun.star.util.SortField")

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
        self.assertEqual(_named_ranges(FakeLegacyNamedDocument(), 1)["total"], 2)

    def test_startup_diagnostic_is_bounded_and_removes_paths_and_urls(self):
        with TemporaryDirectory() as temporary:
            diagnostic = Path(temporary) / "startup.log"
            diagnostic.write_text(
                "ignored\nbootstrap failed at /job/profile because https://example.test/detail was unavailable\n",
                encoding="utf-8",
            )
            result = _startup_diagnostic(diagnostic)
        self.assertIn("bootstrap failed", result)
        self.assertIn("<path>", result)
        self.assertIn("<url>", result)
        self.assertNotIn("/job/profile", result)
        self.assertNotIn("example.test", result)
        self.assertLessEqual(len(result), 256)


if __name__ == "__main__":
    unittest.main()
