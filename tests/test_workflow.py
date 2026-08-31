import unittest

from omasheets.errors import PolicyError
from omasheets.workflow import validate_workflow


def workflow(groups=None):
    return {
        "goal": "Create a checked variance analysis",
        "summary": "Add variance formulas and make large deviations visible.",
        "assumptions": ["Amounts are in the same currency."],
        "evidence_ids": ["a" * 32],
        "groups": groups or [{
            "title": "Calculate variance",
            "purpose": "Compare actual values with budget values.",
            "operation_indexes": [0, 1],
        }],
    }


class WorkflowTests(unittest.TestCase):
    def test_normalizes_explainable_workflow(self):
        result = validate_workflow(workflow(), 2)
        self.assertEqual(result["goal"], "Create a checked variance analysis")
        self.assertEqual(result["groups"][0]["operation_indexes"], [0, 1])

    def test_groups_must_cover_each_operation_exactly_once(self):
        for groups in (
            [{"title": "One", "purpose": "Only one", "operation_indexes": [0]}],
            [{"title": "Twice", "purpose": "Duplicate", "operation_indexes": [0, 0, 1]}],
        ):
            with self.subTest(groups=groups), self.assertRaises(PolicyError):
                validate_workflow(workflow(groups), 2)

    def test_evidence_identifiers_are_bounded_and_unique(self):
        payload = workflow()
        payload["evidence_ids"] = ["a" * 32, "a" * 32]
        with self.assertRaises(PolicyError):
            validate_workflow(payload, 2)


if __name__ == "__main__":
    unittest.main()
