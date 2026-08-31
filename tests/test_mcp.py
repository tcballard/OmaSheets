import io
import json
import unittest

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

    def test_unknown_tool_uses_invalid_params(self) -> None:
        response = self.server.handle(request("tools/call", {"name": "commit_plan", "arguments": {}}))
        self.assertEqual(response["error"]["code"], -32602)

    def test_stdio_parse_error_does_not_leak_details(self) -> None:
        source = io.StringIO("not json\n")
        destination = io.StringIO()
        serve_stdio(self.service, source, destination)
        response = json.loads(destination.getvalue())
        self.assertEqual(response["error"]["code"], -32700)


if __name__ == "__main__":
    unittest.main()
