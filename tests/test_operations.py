import unittest

from omasheets.errors import PolicyError
from omasheets.operations import destructive_operations, validate_operations


class OperationTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()

