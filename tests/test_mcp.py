import io
import json
import unittest
from unittest.mock import patch

from omasheets.mcp import McpServer, PROTOCOL_VERSION, serve_stdio


META = {
    "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
    "io.modelcontextprotocol/clientCapabilities": {},
}


class FakeService:
    def __init__(self) -> None:
        self.calls = []

    def __getattr__(self, name):
        def call(**arguments):
            self.calls.append((name, arguments))
            return {"method": name, "arguments": arguments}
        return call

    def current_resource(self):
        return {"selected": False}

    def pending_resource(self):
        return {"pending": False}

    def capabilities_resource(self):
        return {"libreoffice_fork": False, "agent_publish_authority": False}

    def window_context_resource(self):
        return {"active": True, "address": "B7", "agent_control": False}

    def agent_session_resource(self):
        return {"active": True, "selection": {"address": "B7"}}


def request(method, params=None, request_id=1):
    supplied = dict(params or {})
    supplied["_meta"] = META
    return {"jsonrpc": "2.0", "id": request_id, "method": method, "params": supplied}


class McpTests(unittest.TestCase):
    def setUp(self) -> None:
        self.service = FakeService()
        self.server = McpServer(self.service)

    def test_initialize_advertises_current_protocol(self) -> None:
        response = self.server.handle(request("initialize"))
        self.assertEqual(response["result"]["protocolVersion"], PROTOCOL_VERSION)

    def test_request_metadata_is_required(self) -> None:
        response = self.server.handle({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})
        self.assertEqual(response["error"]["code"], -32602)

    def test_hidden_path_and_publish_arguments_are_rejected(self) -> None:
        for hidden in ({"path": "/tmp/book.xlsx"}, {"target_mode": "replace"}, {"agent": False}):
            arguments = {"session_id": "a" * 32, **hidden}
            response = self.server.handle(request("tools/call", {"name": "describe_workbook", "arguments": arguments}))
            self.assertEqual(response["error"]["code"], -32602)
        self.assertEqual(self.service.calls, [])

    def test_apply_plan_is_only_a_handoff(self) -> None:
        response = self.server.handle(request("tools/call", {
            "name": "apply_plan",
            "arguments": {"plan_id": "b" * 32, "expected_revision": 4},
        }))
        self.assertNotIn("error", response)
        self.assertEqual(self.service.calls[0][0], "apply_plan_handoff")

    def test_defaults_are_applied(self) -> None:
        response = self.server.handle(request("tools/call", {
            "name": "trace_formula",
            "arguments": {"session_id": "a" * 32, "sheet": "Sheet1", "cell": "A1"},
        }))
        self.assertNotIn("error", response)
        _, arguments = self.service.calls[0]
        self.assertEqual(arguments["direction"], "both")
        self.assertEqual(arguments["max_depth"], 5)

    def test_workbook_analysis_is_provider_neutral_and_bounded(self) -> None:
        response = self.server.handle(request("tools/call", {
            "name": "analyze_workbook", "arguments": {"session_id": "a" * 32},
        }))
        self.assertNotIn("error", response)
        name, arguments = self.service.calls[0]
        self.assertEqual(name, "analyze_workbook")
        self.assertEqual(arguments, {"session_id": "a" * 32, "focus": "all", "max_findings": 50})

    def test_range_read_style_default_is_explicit(self) -> None:
        response = self.server.handle(request("tools/call", {
            "name": "read_range",
            "arguments": {"session_id": "a" * 32, "sheet": "Sheet1", "range": "A1:B2"},
        }))
        self.assertNotIn("error", response)
        _, arguments = self.service.calls[0]
        self.assertTrue(arguments["include_formulas"])
        self.assertFalse(arguments["include_styles"])

    def test_unknown_operation_fields_are_rejected(self) -> None:
        response = self.server.handle(request("tools/call", {
            "name": "plan_changes",
            "arguments": {
                "session_id": "a" * 32,
                "expected_revision": 1,
                "operations": [{"type": "set_value", "sheet": "S", "range": "A1", "value": 1, "path": "/tmp/x"}],
            },
        }))
        self.assertEqual(response["error"]["code"], -32602)

    def test_agent_plans_require_explainable_workflow_context(self) -> None:
        arguments = {
            "session_id": "a" * 32,
            "expected_revision": 1,
            "operations": [{"type": "set_value", "sheet": "S", "range": "A1", "value": 1}],
        }
        response = self.server.handle(request("tools/call", {"name": "plan_changes", "arguments": arguments}))
        self.assertEqual(response["error"]["code"], -32602)

        arguments["workflow"] = {
            "goal": "Correct the selected total",
            "summary": "Replace the stale scalar after inspecting the workbook.",
            "evidence_ids": ["c" * 32],
            "groups": [{
                "title": "Correct total", "purpose": "Use the verified source value.",
                "operation_indexes": [0],
            }],
        }
        response = self.server.handle(request("tools/call", {"name": "plan_changes", "arguments": arguments}))
        self.assertNotIn("error", response)

    def test_revise_plan_is_a_distinct_non_publication_tool(self) -> None:
        response = self.server.handle(request("tools/call", {
            "name": "revise_plan",
            "arguments": {
                "plan_id": "b" * 32,
                "expected_revision": 1,
                "operations": [{"type": "clear_range", "sheet": "S", "range": "A1"}],
                "workflow": {
                    "goal": "Remove the stale value",
                    "summary": "Clear only the inspected cell.",
                    "evidence_ids": ["c" * 32],
                    "groups": [{
                        "title": "Clear stale value", "purpose": "Remove obsolete input.",
                        "operation_indexes": [0],
                    }],
                },
            },
        }))
        self.assertNotIn("error", response)
        self.assertEqual(self.service.calls[0][0], "revise_plan")

    def test_unknown_tool_uses_invalid_params(self) -> None:
        response = self.server.handle(request("tools/call", {"name": "commit_plan", "arguments": {}}))
        self.assertEqual(response["error"]["code"], -32602)

    def test_stdio_parse_error_does_not_leak_details(self) -> None:
        source = io.StringIO("not json\n")
        destination = io.StringIO()
        serve_stdio(self.service, source, destination)
        response = json.loads(destination.getvalue())
        self.assertEqual(response["error"]["code"], -32700)

    def test_capabilities_make_the_engine_boundary_explicit(self) -> None:
        response = self.server.handle(request("resources/read", {"uri": "omasheets://capabilities"}))
        payload = json.loads(response["result"]["contents"][0]["text"])
        self.assertFalse(payload["libreoffice_fork"])
        self.assertFalse(payload["agent_publish_authority"])

    def test_live_window_context_is_agent_readable_but_not_controllable(self) -> None:
        response = self.server.handle(request("resources/read", {"uri": "omasheets://window"}))
        payload = json.loads(response["result"]["contents"][0]["text"])
        self.assertEqual(payload["address"], "B7")
        self.assertFalse(payload["agent_control"])

    def test_agent_session_resource_is_selection_aware(self) -> None:
        response = self.server.handle(request("resources/read", {"uri": "omasheets://session"}))
        payload = json.loads(response["result"]["contents"][0]["text"])
        self.assertEqual(payload["selection"]["address"], "B7")

    def test_stdio_rejects_and_drains_oversized_messages(self) -> None:
        source = io.StringIO("x" * 40 + "\nnot json\n")
        destination = io.StringIO()
        with patch("omasheets.mcp.MAX_MESSAGE_CHARACTERS", 32):
            serve_stdio(self.service, source, destination)
        responses = [json.loads(line) for line in destination.getvalue().splitlines()]
        self.assertEqual(responses[0]["error"]["message"], "Message too large")
        self.assertEqual(responses[1]["error"]["code"], -32700)


if __name__ == "__main__":
    unittest.main()
