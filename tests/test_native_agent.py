import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from omasheets import native_agent as native
from omasheets.errors import EngineError
from omasheets.mcp import InvalidParams


class NativeAgentTests(unittest.TestCase):
    def setUp(self):
        self.runtime = tempfile.TemporaryDirectory()
        self.addCleanup(self.runtime.cleanup)
        self.environment = patch.dict(os.environ, {"XDG_RUNTIME_DIR": self.runtime.name})
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.directory = Path(self.runtime.name) / "omasheets"
        self.directory.mkdir(mode=0o700)
        self.selected = {"schema": 1, "session_id": "a" * 32, "pid": os.getpid(),
                         "path": "/private/work.omasheets", "selection": {"sheet": "b" * 32, "row": 0, "column": 0, "rows": 1, "columns": 1}}
        self.context = self.directory / "native-agent-session.json"
        self.context.write_text(json.dumps(self.selected))
        self.context.chmod(0o600)

    def test_resource_contains_no_workbook_path(self):
        with patch.object(native, "_request", return_value={"revision": "r", "sheets": []}):
            result = native.resource()
        self.assertNotIn("/private/", json.dumps(result))
        self.assertEqual(result["session_id"], "a" * 32)
        self.assertEqual(result["selection"], self.selected["selection"])

    def test_private_owned_context_and_window_lifetime(self):
        self.context.chmod(0o644)
        with self.assertRaises(EngineError): native.context()
        self.context.unlink()
        target = self.directory / "elsewhere.json"
        target.write_text(json.dumps(self.selected)); target.chmod(0o600)
        self.context.symlink_to(target)
        with self.assertRaises(EngineError): native.context()

    def test_session_switch_refuses_old_agent_and_unknown_authority(self):
        with self.assertRaises(EngineError): native.call("native_overview", {"session_id": "c" * 32})
        for field, value in [("path", "/another"), ("actor", "human"), ("command", "merge")]:
            with self.assertRaises(InvalidParams): native.call("native_overview", {"session_id": "a" * 32, field: value})
        self.assertFalse(any("approve" in tool["name"] or "export" in tool["name"] for tool in native.TOOLS))

    def test_proposal_is_typed_and_revision_is_not_truncated(self):
        args = {"session_id": "a" * 32, "expected_revision": "r" * 130, "goal": "Update", "explanation": "Source changed",
                "assumptions": [], "evidence": ["A1"], "edits": [
                    {"sheet": "b" * 32, "a1": "A1", "value": "15"},
                    {"sheet": "b" * 32, "a1": "B1", "value": "=A1*2"},
                    {"sheet": "b" * 32, "a1": "C1", "value": "TRUE"}]}
        with patch.object(native, "_request", return_value={"branch": "proposal-x"}) as request:
            native.call("native_propose", args)
        sent = request.call_args.kwargs
        self.assertEqual(sent["expected_revision"], "r" * 130)
        self.assertEqual([command["command"] for command in sent["proposal"]["commands"]], ["set_value", "set_formula", "set_value"])
        self.assertEqual(sent["proposal"]["commands"][2]["value"], {"type": "boolean", "value": True})
        self.assertNotIn("session_id", sent["proposal"])

    def test_reads_are_bounded_before_service_call(self):
        with patch.object(native, "_request") as request:
            with self.assertRaises(EngineError):
                native.call("native_read", {"session_id": "a" * 32, "sheet": "b" * 32, "row": 0, "column": 0, "rows": 100, "columns": 100})
            request.assert_not_called()


if __name__ == "__main__":
    unittest.main()
