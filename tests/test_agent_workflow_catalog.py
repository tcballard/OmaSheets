import json
from pathlib import Path
import unittest

from omasheets.mcp import TOOLS_BY_NAME
from omasheets.operations import SUPPORTED_OPERATIONS


class AgentWorkflowCatalogTests(unittest.TestCase):
    def test_flagship_workflows_are_implementable_by_the_public_protocol(self):
        root = Path(__file__).parents[1]
        scenarios = json.loads((root / "tests/fixtures/agent_workflows.json").read_text())
        skill = (root / "plugins/omasheets/skills/omasheets/SKILL.md").read_text().casefold()
        self.assertEqual(
            {scenario["id"] for scenario in scenarios},
            {"explain", "clean", "variance", "reconcile", "summarise", "format"},
        )
        for scenario in scenarios:
            with self.subTest(workflow=scenario["id"]):
                self.assertIn(scenario["id"], skill)
                self.assertIn("describe_workbook", scenario["inspection"])
                self.assertTrue(set(scenario["inspection"]) <= set(TOOLS_BY_NAME))
                self.assertTrue(set(scenario["required_operations"]) <= set(SUPPORTED_OPERATIONS))
                self.assertEqual(bool(scenario["required_operations"]), scenario["may_plan"])

    def test_agent_protocol_has_no_publication_primitive(self):
        forbidden = {"approve", "commit", "publish", "replace", "undo"}
        self.assertFalse(forbidden & set(TOOLS_BY_NAME))
        self.assertIn("apply_plan", TOOLS_BY_NAME)
        self.assertIn("revise_plan", TOOLS_BY_NAME)


if __name__ == "__main__":
    unittest.main()
