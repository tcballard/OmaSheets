# Agent protocol v0.0.1

OmaSheets speaks JSON-RPC 2.0 over standard input/output using the Model Context
Protocol. The server advertises a fixed protocol version and strict tool schemas.

## Read tools

- `describe_workbook`: sheets, used ranges, names, formula/error summary.
- `read_range`: bounded values and optionally formulas.
- `search_workbook`: bounded literal search over displayed values or formulas.
- `trace_formula`: bounded precedents, dependents, or both.
- `render_workbook`: produce a verified PDF preview.
- `change_history`: plans by default; receipts only for locally committed work.

## Planning tools

- `plan_changes`: typed operations against the selected workbook revision.
- `get_plan`: current sealed plan and verification evidence.
- `apply_plan`: verify the expected revision and return local approval instructions.

`apply_plan` is intentionally not an apply primitive. The MCP server has no
approve, commit, reject, replace, or undo tool.

## Initial operation set

- `set_value`
- `set_formula`
- `clear_range`
- `rename_sheet`
- `add_sheet`
- `delete_sheet`

Operations use A1 references, explicit sheet names, and bounded scalar payloads.
Unknown operation fields are rejected. Structural operations that could destroy
data are prominently marked in the review evidence.

## Resources

- `omasheets://current`: the last workbook explicitly selected locally.
- `omasheets://pending`: the current pending plan, if one exists.

Resources are convenience pointers, not ambient authority. A client cannot
change the selected workbook by constructing a URI or passing a path.

## Errors

Protocol, schema, stale revision, conflict, policy, and engine failures use
distinct structured error data. Error responses never include workbook bytes,
home paths, environment values, or worker stderr without redaction.
