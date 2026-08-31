import unittest

from omasheets.errors import PolicyError
from omasheets.operations import destructive_operations, range_shape, validate_operations


class OperationTests(unittest.TestCase):
    def test_scalar_writes_reject_multi_cell_ranges(self):
        for operation in (
            {"type": "set_value", "sheet": "Sheet1", "range": "A1:B2", "value": 1},
            {"type": "set_formula", "sheet": "Sheet1", "range": "A1:A2", "formula": "=1+1"},
        ):
            with self.subTest(operation=operation["type"]), self.assertRaises(PolicyError):
                validate_operations([operation])

    def test_normalizes_a1_ranges(self) -> None:
        result = validate_operations([{"type": "clear_range", "sheet": "Data", "range": "a1:b2"}])
        self.assertEqual(result[0]["range"], "A1:B2")

    def test_formula_must_start_with_equals(self) -> None:
        with self.assertRaises(PolicyError):
            validate_operations([{"type": "set_formula", "sheet": "Data", "range": "A1", "formula": "SUM(A2:A3)"}])

    def test_type_specific_unknown_fields_are_rejected(self) -> None:
        with self.assertRaises(PolicyError):
            validate_operations([{"type": "clear_range", "sheet": "Data", "range": "A1", "value": "hidden"}])

    def test_destructive_operations_are_indexed(self) -> None:
        operations = validate_operations([
            {"type": "set_value", "sheet": "Data", "range": "A1", "value": 1},
            {"type": "delete_sheet", "sheet": "Old"},
        ])
        self.assertEqual(destructive_operations(operations), [1])

    def test_bulk_values_require_an_exact_bounded_matrix(self) -> None:
        operation = {
            "type": "set_range_values",
            "sheet": "Data",
            "range": "B2:C3",
            "values": [[1, 2], [3, None]],
        }
        self.assertEqual(validate_operations([operation])[0]["values"], operation["values"])
        operation["values"] = [[1, 2, 3]]
        with self.assertRaises(PolicyError):
            validate_operations([operation])

    def test_bulk_formulas_must_all_be_formulas(self) -> None:
        operation = {
            "type": "set_range_formulas",
            "sheet": "Data",
            "range": "A1:A2",
            "formulas": [["=1+1"], ["not a formula"]],
        }
        with self.assertRaises(PolicyError):
            validate_operations([operation])

    def test_formatting_is_typed_and_normalized(self) -> None:
        result = validate_operations([{
            "type": "format_cells",
            "sheet": "Data",
            "range": "A1:B2",
            "number_format": "0.00%",
            "bold": True,
            "background_color": "#aabbcc",
            "wrap_text": False,
        }])
        self.assertEqual(result[0]["background_color"], "#AABBCC")
        with self.assertRaises(PolicyError):
            validate_operations([{"type": "format_cells", "sheet": "Data", "range": "A1"}])

    def test_ranges_are_bounded_by_sheet_and_operation_limits(self) -> None:
        self.assertEqual(range_shape("A1:XFD1"), (1, 16384))
        for invalid in ("XFE1", "A1048577", "B2:A1"):
            with self.subTest(invalid=invalid), self.assertRaises(PolicyError):
                range_shape(invalid)
        with self.assertRaises(PolicyError):
            validate_operations([{"type": "clear_range", "sheet": "Data", "range": "A1:A10001"}])


if __name__ == "__main__":
    unittest.main()
