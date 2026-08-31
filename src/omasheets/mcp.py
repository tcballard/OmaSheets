"""Dependency-free, strict stdio MCP boundary for OmaSheets."""

from __future__ import annotations

import json
import sys
from dataclasses import dataclass
from typing import Any, BinaryIO, Protocol, TextIO

from . import __version__
from .errors import OmaSheetsError, PolicyError

PROTOCOL_VERSION = "2026-07-28"
SERVER_INFO = {"name": "omasheets", "version": __version__}
MAX_MESSAGE_CHARACTERS = 8 * 1024 * 1024


class AgentService(Protocol):
    def describe_workbook(self, **arguments: Any) -> dict[str, Any]: ...
    def read_range(self, **arguments: Any) -> dict[str, Any]: ...
    def search_workbook(self, **arguments: Any) -> dict[str, Any]: ...
    def trace_formula(self, **arguments: Any) -> dict[str, Any]: ...
    def query_workbook(self, **arguments: Any) -> dict[str, Any]: ...
    def analyze_workbook(self, **arguments: Any) -> dict[str, Any]: ...
    def render_workbook(self, **arguments: Any) -> dict[str, Any]: ...
    def change_history(self, **arguments: Any) -> dict[str, Any]: ...
    def plan_changes(self, **arguments: Any) -> dict[str, Any]: ...
    def revise_plan(self, **arguments: Any) -> dict[str, Any]: ...
    def get_plan(self, **arguments: Any) -> dict[str, Any]: ...
    def apply_plan_handoff(self, **arguments: Any) -> dict[str, Any]: ...
    def current_resource(self) -> dict[str, Any]: ...
    def pending_resource(self) -> dict[str, Any]: ...
    def capabilities_resource(self) -> dict[str, Any]: ...
    def window_context_resource(self) -> dict[str, Any]: ...
    def agent_session_resource(self) -> dict[str, Any]: ...


def _object(properties: dict[str, Any], required: list[str] | None = None) -> dict[str, Any]:
    return {
        "type": "object",
        "properties": properties,
        "required": required or [],
        "additionalProperties": False,
    }


SESSION = {"type": "string", "pattern": "^[a-f0-9]{32}$"}
PLAN = {"type": "string", "pattern": "^[a-f0-9]{32}$"}
REVISION = {"type": "integer", "minimum": 1}
SHEET = {"type": "string", "minLength": 1, "maxLength": 128}
RANGE = {"type": "string", "minLength": 1, "maxLength": 64}
SCALAR = {"type": ["string", "number", "integer", "boolean", "null"]}
VALUE_MATRIX = {
    "type": "array",
    "minItems": 1,
    "maxItems": 10_000,
    "items": {"type": "array", "minItems": 1, "maxItems": 10_000, "items": SCALAR},
}
FORMULA_MATRIX = {
    "type": "array",
    "minItems": 1,
    "maxItems": 10_000,
    "items": {
        "type": "array",
        "minItems": 1,
        "maxItems": 10_000,
        "items": {"type": "string", "minLength": 1, "maxLength": 8192},
    },
}
PIVOT_VALUES = {
    "type": "array", "minItems": 1, "maxItems": 8,
    "items": _object({
        "field": SHEET,
        "function": {"type": "string", "enum": ["sum", "count", "average", "min", "max"]},
        "label": SHEET,
    }, ["field", "function"]),
}

OPERATION_SCHEMA = _object(
    {
        "type": {
            "type": "string",
            "enum": [
                "set_value",
                "set_formula",
                "clear_range",
                "rename_sheet",
                "add_sheet",
                "delete_sheet",
                "set_range_values",
                "set_range_formulas",
                "format_cells",
                "insert_rows",
                "delete_rows",
                "insert_columns",
                "delete_columns",
                "fill_down",
                "fill_right",
                "sort_range",
                "upsert_chart",
                "upsert_pivot",
                "refresh_pivot",
            ],
        },
        "sheet": SHEET,
        "range": RANGE,
        "value": SCALAR,
        "formula": {"type": "string", "minLength": 1, "maxLength": 8192},
        "formulas": FORMULA_MATRIX,
        "number_format": {"type": "string", "minLength": 1, "maxLength": 128},
        "bold": {"type": "boolean"},
        "text_color": {"type": "string", "pattern": "^#[0-9A-Fa-f]{6}$"},
        "background_color": {"type": "string", "pattern": "^#[0-9A-Fa-f]{6}$"},
        "wrap_text": {"type": "boolean"},
        "new_name": SHEET,
        "row": {"type": "integer", "minimum": 1, "maximum": 1048576},
        "column": {"type": "string", "pattern": "^[A-Za-z]{1,3}$"},
        "count": {"type": "integer", "minimum": 1, "maximum": 10000},
        "source_rows": {"type": "integer", "minimum": 1, "maximum": 9999},
        "source_columns": {"type": "integer", "minimum": 1, "maximum": 9999},
        "key_column": {"type": "integer", "minimum": 1, "maximum": 16384},
        "ascending": {"type": "boolean"},
        "has_header": {"type": "boolean"},
        "name": {"type": "string", "minLength": 1, "maxLength": 128},
        "source_sheet": SHEET,
        "source_range": RANGE,
        "anchor_range": RANGE,
        "chart_type": {"type": "string", "enum": ["column", "bar", "line", "pie", "scatter"]},
        "title": {"type": "string", "minLength": 1, "maxLength": 256},
        "has_column_headers": {"type": "boolean"},
        "has_row_headers": {"type": "boolean"},
        "legend": {"type": "boolean"},
        "output_cell": RANGE,
        "rows": {"type": "array", "maxItems": 8, "items": SHEET},
        "columns": {"type": "array", "maxItems": 4, "items": SHEET},
        "filters": {"type": "array", "maxItems": 4, "items": SHEET},
        "values": {"anyOf": [VALUE_MATRIX, PIVOT_VALUES]},
    },
    ["type"],
)

EVIDENCE_IDS = {
    "type": "array",
    "minItems": 1,
    "maxItems": 50,
    "items": {"type": "string", "pattern": "^[a-f0-9]{32}$"},
}
WORKFLOW_SCHEMA = _object(
    {
        "goal": {"type": "string", "minLength": 1, "maxLength": 1000},
        "summary": {"type": "string", "minLength": 1, "maxLength": 2000},
        "assumptions": {
            "type": "array", "maxItems": 20,
            "items": {"type": "string", "minLength": 1, "maxLength": 500},
        },
        "evidence_ids": EVIDENCE_IDS,
        "groups": {
            "type": "array", "minItems": 1, "maxItems": 20,
            "items": _object(
                {
                    "title": {"type": "string", "minLength": 1, "maxLength": 120},
                    "purpose": {"type": "string", "minLength": 1, "maxLength": 500},
                    "operation_indexes": {
                        "type": "array", "minItems": 1, "maxItems": 100,
                        "items": {"type": "integer", "minimum": 0, "maximum": 99},
                    },
                },
                ["title", "purpose", "operation_indexes"],
            ),
        },
    },
    ["goal", "summary", "evidence_ids", "groups"],
)

QUERY_ID = {
    "type": "string",
    "minLength": 1,
    "maxLength": 64,
    "pattern": "^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$",
}
QUERY_WORKBOOK_ITEM = {
    "anyOf": [
        _object(
            {
                "id": QUERY_ID,
                "tool": {"type": "string", "enum": ["describe_workbook"]},
                "arguments": _object(
                    {"include_formulas": {"type": "boolean", "default": False}},
                ),
            },
            ["id", "tool", "arguments"],
        ),
        _object(
            {
                "id": QUERY_ID,
                "tool": {"type": "string", "enum": ["read_range"]},
                "arguments": _object(
                    {
                        "sheet": SHEET,
                        "range": RANGE,
                        "include_formulas": {"type": "boolean", "default": True},
                        "include_styles": {"type": "boolean", "default": False},
                    },
                    ["sheet", "range"],
                ),
            },
            ["id", "tool", "arguments"],
        ),
        _object(
            {
                "id": QUERY_ID,
                "tool": {"type": "string", "enum": ["search_workbook"]},
                "arguments": _object(
                    {
                        "query": {"type": "string", "minLength": 1, "maxLength": 256},
                        "scope": {"type": "string", "enum": ["values", "formulas", "both"], "default": "both"},
                        "max_results": {"type": "integer", "minimum": 1, "maximum": 200, "default": 50},
                    },
                    ["query"],
                ),
            },
            ["id", "tool", "arguments"],
        ),
        _object(
            {
                "id": QUERY_ID,
                "tool": {"type": "string", "enum": ["trace_formula"]},
                "arguments": _object(
                    {
                        "sheet": SHEET,
                        "cell": RANGE,
                        "direction": {"type": "string", "enum": ["precedents", "dependents", "both"], "default": "both"},
                        "max_depth": {"type": "integer", "minimum": 1, "maximum": 10, "default": 5},
                    },
                    ["sheet", "cell"],
                ),
            },
            ["id", "tool", "arguments"],
        ),
    ],
}


TOOLS: list[dict[str, Any]] = [
    {
        "name": "describe_workbook",
        "description": "Describe the locally selected workbook without modifying it.",
        "inputSchema": _object(
            {"session_id": SESSION, "include_formulas": {"type": "boolean", "default": False}},
            ["session_id"],
        ),
    },
    {
        "name": "read_range",
        "description": "Read a bounded A1 range from the selected workbook.",
        "inputSchema": _object(
            {
                "session_id": SESSION,
                "sheet": SHEET,
                "range": RANGE,
                "include_formulas": {"type": "boolean", "default": True},
                "include_styles": {"type": "boolean", "default": False},
            },
            ["session_id", "sheet", "range"],
        ),
    },
    {
        "name": "search_workbook",
        "description": "Case-insensitive literal search over bounded workbook content.",
        "inputSchema": _object(
            {
                "session_id": SESSION,
                "query": {"type": "string", "minLength": 1, "maxLength": 256},
                "scope": {"type": "string", "enum": ["values", "formulas", "both"], "default": "both"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 200, "default": 50},
            },
            ["session_id", "query"],
        ),
    },
    {
        "name": "trace_formula",
        "description": "Trace bounded formula precedents, dependents, or both.",
        "inputSchema": _object(
            {
                "session_id": SESSION,
                "sheet": SHEET,
                "cell": RANGE,
                "direction": {"type": "string", "enum": ["precedents", "dependents", "both"], "default": "both"},
                "max_depth": {"type": "integer", "minimum": 1, "maximum": 10, "default": 5},
            },
            ["session_id", "sheet", "cell"],
        ),
    },
    {
        "name": "query_workbook",
        "description": "Run 1-8 independent read queries against one workbook snapshot and return one sealed evidence record.",
        "inputSchema": _object(
            {
                "session_id": SESSION,
                "queries": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "items": QUERY_WORKBOOK_ITEM,
                },
            },
            ["session_id", "queries"],
        ),
    },
    {
        "name": "analyze_workbook",
        "description": "Audit the whole bounded workbook for structure, quality issues, anomalies, formulas, charts, pivots, and management-summary opportunities without modifying it.",
        "inputSchema": _object(
            {
                "session_id": SESSION,
                "focus": {"type": "string", "enum": ["all", "quality", "management"], "default": "all"},
                "max_findings": {"type": "integer", "minimum": 1, "maximum": 100, "default": 50},
            },
            ["session_id"],
        ),
    },
    {
        "name": "render_workbook",
        "description": "Render the selected workbook to a verified PDF preview.",
        "inputSchema": _object(
            {"session_id": SESSION, "format": {"type": "string", "enum": ["pdf"], "default": "pdf"}},
            ["session_id"],
        ),
    },
    {
        "name": "change_history",
        "description": "List plan history, and locally committed receipts when requested.",
        "inputSchema": _object(
            {
                "session_id": SESSION,
                "since_revision": {"type": "integer", "minimum": 1},
                "include_receipts": {"type": "boolean", "default": False},
            },
            ["session_id"],
        ),
    },
    {
        "name": "plan_changes",
        "description": "Stage an evidence-cited, purpose-grouped proposal; this does not publish workbook bytes.",
        "inputSchema": _object(
            {
                "session_id": SESSION,
                "expected_revision": REVISION,
                "operations": {"type": "array", "minItems": 1, "maxItems": 100, "items": OPERATION_SCHEMA},
                "workflow": WORKFLOW_SCHEMA,
            },
            ["session_id", "expected_revision", "operations", "workflow"],
        ),
    },
    {
        "name": "revise_plan",
        "description": "Replace a verified proposal after human feedback; the prior plan becomes superseded.",
        "inputSchema": _object(
            {
                "plan_id": PLAN,
                "expected_revision": REVISION,
                "operations": {"type": "array", "minItems": 1, "maxItems": 100, "items": OPERATION_SCHEMA},
                "workflow": WORKFLOW_SCHEMA,
            },
            ["plan_id", "expected_revision", "operations", "workflow"],
        ),
    },
    {
        "name": "get_plan",
        "description": "Read a sealed plan and its verification evidence.",
        "inputSchema": _object({"plan_id": PLAN}, ["plan_id"]),
    },
    {
        "name": "apply_plan",
        "description": "Return local review instructions after rechecking a plan revision; never commits.",
        "inputSchema": _object({"plan_id": PLAN, "expected_revision": REVISION}, ["plan_id", "expected_revision"]),
    },
]

TOOLS_BY_NAME = {tool["name"]: tool for tool in TOOLS}


class InvalidParams(ValueError):
    pass


def _matches_type(value: Any, expected: str) -> bool:
    if expected == "null":
        return value is None
    if expected == "boolean":
        return isinstance(value, bool)
    if expected == "integer":
        return isinstance(value, int) and not isinstance(value, bool)
    if expected == "number":
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    if expected == "string":
        return isinstance(value, str)
    if expected == "array":
        return isinstance(value, list)
    if expected == "object":
        return isinstance(value, dict)
    return False


def _validate(value: Any, schema: dict[str, Any], path: str = "arguments") -> Any:
    if "anyOf" in schema:
        for option in schema["anyOf"]:
            try:
                return _validate(value, option, path)
            except InvalidParams:
                pass
        raise InvalidParams(f"{path} does not match an allowed shape")
    expected = schema.get("type")
    expected_types = expected if isinstance(expected, list) else [expected]
    if expected and not any(_matches_type(value, item) for item in expected_types):
        raise InvalidParams(f"{path} has the wrong type")

    if isinstance(value, dict):
        properties = schema.get("properties", {})
        required = schema.get("required", [])
        missing = [name for name in required if name not in value]
        if missing:
            raise InvalidParams(f"{path} is missing: {', '.join(missing)}")
        unknown = sorted(set(value) - set(properties))
        if unknown and schema.get("additionalProperties") is False:
            raise InvalidParams(f"{path} contains unknown fields: {', '.join(unknown)}")
        result = dict(value)
        for name, child_schema in properties.items():
            if name in result:
                result[name] = _validate(result[name], child_schema, f"{path}.{name}")
            elif "default" in child_schema:
                result[name] = child_schema["default"]
        return result

    if isinstance(value, list):
        if len(value) < schema.get("minItems", 0) or len(value) > schema.get("maxItems", len(value)):
            raise InvalidParams(f"{path} has an invalid item count")
        item_schema = schema.get("items", {})
        return [_validate(item, item_schema, f"{path}[{index}]") for index, item in enumerate(value)]

    if isinstance(value, str):
        if len(value) < schema.get("minLength", 0) or len(value) > schema.get("maxLength", len(value)):
            raise InvalidParams(f"{path} has an invalid length")
        pattern = schema.get("pattern")
        if pattern:
            import re
            if re.fullmatch(pattern, value) is None:
                raise InvalidParams(f"{path} has an invalid format")

    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if "minimum" in schema and value < schema["minimum"]:
            raise InvalidParams(f"{path} is below the minimum")
        if "maximum" in schema and value > schema["maximum"]:
            raise InvalidParams(f"{path} is above the maximum")

    if "enum" in schema and value not in schema["enum"]:
        raise InvalidParams(f"{path} is not an allowed value")
    return value


def validate_tool_arguments(name: str, arguments: Any) -> dict[str, Any]:
    tool = TOOLS_BY_NAME.get(name)
    if tool is None:
        raise InvalidParams(f"unknown tool: {name}")
    if not isinstance(arguments, dict):
        raise InvalidParams("arguments must be an object")
    result = _validate(arguments, tool["inputSchema"])
    if name == "query_workbook":
        identifiers = [query["id"] for query in result["queries"]]
        if len(identifiers) != len(set(identifiers)):
            raise InvalidParams("arguments.queries contains duplicate ids")
    return result


@dataclass(slots=True)
class McpServer:
    service: AgentService

    def _meta(self) -> dict[str, Any]:
        return {"io.modelcontextprotocol/serverInfo": SERVER_INFO}

    def _require_request_meta(self, request: dict[str, Any]) -> None:
        params = request.get("params", {})
        meta = params.get("_meta") if isinstance(params, dict) else None
        if not isinstance(meta, dict):
            raise InvalidParams("params._meta is required")
        if meta.get("io.modelcontextprotocol/protocolVersion") != PROTOCOL_VERSION:
            raise InvalidParams("unsupported or missing protocol version")
        if not isinstance(meta.get("io.modelcontextprotocol/clientCapabilities"), dict):
            raise InvalidParams("clientCapabilities must be an object")

    def handle(self, request: Any) -> dict[str, Any] | None:
        if not isinstance(request, dict) or request.get("jsonrpc") != "2.0":
            return self._error(None, -32600, "Invalid Request")
        request_id = request.get("id")
        if request_id is None:
            return None
        method = request.get("method")
        try:
            self._require_request_meta(request)
            if method == "initialize":
                return self._result(request_id, {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {}, "resources": {}},
                    "serverInfo": SERVER_INFO,
                })
            if method == "tools/list":
                return self._result(request_id, {"tools": TOOLS})
            if method == "tools/call":
                return self._tool_call(request_id, request.get("params", {}))
            if method == "resources/list":
                return self._result(request_id, {"resources": [
                    {"uri": "omasheets://current", "name": "Current workbook", "mimeType": "application/json"},
                    {"uri": "omasheets://pending", "name": "Pending plan", "mimeType": "application/json"},
                    {"uri": "omasheets://capabilities", "name": "OmaSheets capabilities", "mimeType": "application/json"},
                    {"uri": "omasheets://window", "name": "Live OmaSheets window context", "mimeType": "application/json"},
                    {"uri": "omasheets://session", "name": "Current OmaSheets agent session", "mimeType": "application/json"},
                ]})
            if method == "resources/read":
                params = request.get("params", {})
                unknown = set(params) - {"uri", "_meta"}
                if unknown or not isinstance(params.get("uri"), str):
                    raise InvalidParams("resources/read has invalid parameters")
                uri = params["uri"]
                if uri == "omasheets://current":
                    payload = self.service.current_resource()
                elif uri == "omasheets://pending":
                    payload = self.service.pending_resource()
                elif uri == "omasheets://capabilities":
                    payload = self.service.capabilities_resource()
                elif uri == "omasheets://window":
                    payload = self.service.window_context_resource()
                elif uri == "omasheets://session":
                    payload = self.service.agent_session_resource()
                else:
                    raise InvalidParams("unknown resource URI")
                return self._result(request_id, {"contents": [{
                    "uri": uri,
                    "mimeType": "application/json",
                    "text": json.dumps(payload, sort_keys=True),
                }]})
            return self._error(request_id, -32601, "Method not found")
        except InvalidParams as exc:
            return self._error(request_id, -32602, "Invalid params", {"detail": str(exc)})
        except (OmaSheetsError, PolicyError) as exc:
            return self._error(request_id, 1001, "OmaSheets request failed", {"detail": str(exc)})
        except Exception:
            return self._error(request_id, -32603, "Internal error")

    def _tool_call(self, request_id: Any, params: Any) -> dict[str, Any]:
        if not isinstance(params, dict):
            raise InvalidParams("params must be an object")
        unknown = set(params) - {"name", "arguments", "_meta"}
        if unknown or not isinstance(params.get("name"), str):
            raise InvalidParams("tools/call has invalid parameters")
        name = params["name"]
        arguments = validate_tool_arguments(name, params.get("arguments", {}))
        method_name = "apply_plan_handoff" if name == "apply_plan" else name
        result = getattr(self.service, method_name)(**arguments)
        return self._result(request_id, {
            "content": [{"type": "text", "text": json.dumps(result, sort_keys=True)}],
            "structuredContent": result,
            "isError": False,
        })

    def _result(self, request_id: Any, result: dict[str, Any]) -> dict[str, Any]:
        result.setdefault("resultType", "complete")
        result.setdefault("_meta", self._meta())
        return {"jsonrpc": "2.0", "id": request_id, "result": result}

    @staticmethod
    def _error(request_id: Any, code: int, message: str, data: Any = None) -> dict[str, Any]:
        error: dict[str, Any] = {"code": code, "message": message}
        if data is not None:
            error["data"] = data
        return {"jsonrpc": "2.0", "id": request_id, "error": error}


def serve_stdio(service: AgentService, stdin: TextIO = sys.stdin, stdout: TextIO = sys.stdout) -> int:
    """Serve newline-delimited JSON-RPC without logging to stdout."""

    server = McpServer(service)
    while True:
        line = stdin.readline(MAX_MESSAGE_CHARACTERS + 1)
        if line == "":
            break
        if len(line) > MAX_MESSAGE_CHARACTERS:
            while line and not line.endswith("\n"):
                line = stdin.readline(MAX_MESSAGE_CHARACTERS + 1)
            response = McpServer._error(None, -32700, "Message too large")
            stdout.write(json.dumps(response, separators=(",", ":")) + "\n")
            stdout.flush()
            continue
        try:
            request = json.loads(line)
            response = server.handle(request)
        except json.JSONDecodeError:
            response = McpServer._error(None, -32700, "Parse error")
        if response is not None:
            stdout.write(json.dumps(response, separators=(",", ":")) + "\n")
            stdout.flush()
    return 0
