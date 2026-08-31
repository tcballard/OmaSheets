import json
import unittest
from contextlib import redirect_stdout
from io import StringIO
from unittest.mock import Mock
from unittest.mock import patch

from omasheets.agent_session import AGENT_SESSION_PROMPT, launch_agent_session
from omasheets.cli import main
from omasheets.errors import EngineError


class AgentSessionTests(unittest.TestCase):
    def test_launch_uses_omarchy_default_agent_with_fixed_prompt(self):
        launcher = Mock()
        launcher.return_value.pid = 91
        pid = launch_agent_session(which=lambda name: "/usr/bin/omarchy", launcher=launcher)
        self.assertEqual(pid, 91)
        self.assertEqual(launcher.call_args.args[0], [
            "/usr/bin/omarchy", "agent", "prompt", AGENT_SESSION_PROMPT,
        ])
        self.assertNotIn("codex", launcher.call_args.args[0])
        self.assertNotIn("shell", launcher.call_args.kwargs)
        self.assertIn("omasheets://session", AGENT_SESSION_PROMPT)
        self.assertNotIn("/home/", AGENT_SESSION_PROMPT)

    def test_missing_omarchy_launcher_fails_closed(self):
        with self.assertRaises(EngineError):
            launch_agent_session(which=lambda name: None)

    def test_provider_neutral_command_bridge_uses_bounded_tool_validation(self):
        service = Mock()
        service.get_plan.return_value = {"plan_id": "a" * 32, "status": "verified"}
        output = StringIO()
        with patch("omasheets.cli._service", return_value=service), redirect_stdout(output):
            self.assertEqual(main([
                "agent-session", "call", "get_plan", "--arguments", '{"plan_id":"' + "a" * 32 + '"}',
            ]), 0)
        service.get_plan.assert_called_once_with(plan_id="a" * 32)
        self.assertEqual(json.loads(output.getvalue())["status"], "verified")

    def test_provider_neutral_bridge_exposes_the_same_batched_read_tool(self):
        service = Mock()
        service.query_workbook.return_value = {
            "items": [{"id": "structure", "tool": "describe_workbook", "result": {"sheet_count": 1}}],
            "evidence_id": "b" * 32,
        }
        arguments = {
            "session_id": "a" * 32,
            "queries": [{"id": "structure", "tool": "describe_workbook", "arguments": {}}],
        }
        output = StringIO()
        with patch("omasheets.cli._service", return_value=service), redirect_stdout(output):
            self.assertEqual(main([
                "agent-session", "call", "query_workbook",
                "--arguments", json.dumps(arguments, separators=(",", ":")),
            ]), 0)

        called = service.query_workbook.call_args.kwargs
        self.assertEqual(called["session_id"], "a" * 32)
        self.assertEqual(called["queries"][0]["arguments"], {"include_formulas": False})
        self.assertEqual(json.loads(output.getvalue())["evidence_id"], "b" * 32)


if __name__ == "__main__":
    unittest.main()
