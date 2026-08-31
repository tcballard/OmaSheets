# Agent protocol v0.0.1

OmaSheets speaks JSON-RPC 2.0 over standard input/output using the Model Context
Protocol. The server advertises a fixed protocol version and strict tool schemas.

## Read tools

- `describe_workbook`: sheets, used ranges, names, formula/error summary.
- `read_range`: bounded values and optionally formulas plus a deduplicated style
  table for ranges of up to 1,000 cells.
- `search_workbook`: bounded literal search over displayed values or formulas.
- `trace_formula`: bounded precedents, dependents, or both.
- `render_workbook`: produce a verified PDF preview.
- `change_history`: plans by default; receipts only for locally committed work.

Each successful inspection returns a sealed `evidence_id`. Evidence is bound to
the selected session, workbook revision and selected-file or live-window
semantic source. It records the tool, bounded arguments and result digest; it
does not contain a source path.

## Planning tools

- `plan_changes`: evidence-cited, explained and purpose-grouped typed operations
  against the selected workbook revision.
- `revise_plan`: create a replacement verified plan from human feedback and
  mark the previous plan as superseded.
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
- `set_range_values`: an exact-shape, typed two-dimensional scalar matrix.
- `set_range_formulas`: an exact-shape two-dimensional formula matrix.
- `format_cells`: number format, bold, text/background colour, and wrapping.
- `insert_rows` and `delete_rows`: bounded, one-based structural row changes.
- `insert_columns` and `delete_columns`: bounded A1-column structural changes.
- `fill_down` and `fill_right`: reference-aware Calc autofill from an explicit
  source row or column count within the target range.
- `sort_range`: bounded one-key ascending or descending sort with explicit
  header handling.

Operations use A1 references, explicit sheet names, exact matrix dimensions, and
a 10,000-cell per-operation ceiling. Unknown operation fields are rejected.
Structural operations that could destroy data are prominently marked in the
review evidence. Staging fingerprints targeted values, formulas, and requested
format properties before save, then requires the same fingerprints after reopen.

Every MCP-created plan requires a workflow object containing a goal, summary,
optional assumptions, one or more evidence IDs, and purpose groups that cover
each operation exactly once. These fields are bounded, sealed with the plan and
shown in native review. They are agent explanations, not trusted verification;
the semantic diff and reopened workbook evidence remain authoritative.

## Resources

- `omasheets://current`: the last workbook explicitly selected locally.
- `omasheets://pending`: the current pending plan, if one exists.
- `omasheets://capabilities`: engine identity, fork status, supported operations,
  and the local-only publication boundary.
- `omasheets://window`: the active native window's bounded sheet, cell/formula,
  zoom, dirty state, and visible rectangle. It contains no source path and
  grants no pointer, keyboard, save, or publication authority.
- `omasheets://session`: the selection-aware entry resource used by **Ask Agent**.
  It combines the public workbook session, live focus when available, suggested
  workflows and the non-publication contract without exposing a source path.

The equivalent provider-neutral command surface is `omasheets agent-session
resource`, `omasheets agent-session tools`, and `omasheets agent-session call`.
It validates against the same schemas and does not add publication authority.

Resources are convenience pointers, not ambient authority. A client cannot
change the selected workbook by constructing a URI or passing a path.

Opening `omasheets window` creates the same immutable workbook session consumed
by MCP. An agent can reason from the person's current selection and viewport,
then use the ordinary bounded read and planning tools against that session.
Window context is observational: agents still propose semantic cell operations
and the person still approves publication locally.

When the window is active, read and planning tools request a private save-copy
from its LibreOfficeKit document over a same-user Unix socket. The copy retains
unsaved values, formulas, formatting and structure without renaming, saving or
moving the visible workbook. A semantic fingerprint seals live-window plans;
approval requests a fresh snapshot and rejects content drift while ignoring
selection-only changes. Live-window plans can publish only as a new copy.

Staging an active-window plan also publishes a bounded, path-free native review
payload. This is a one-way presentation channel: agents cannot open, close,
approve or otherwise control the overlay. Its before/after records come from
the recalculated staged document and are rechecked before any human-approved
copy is published.

## Errors

Protocol, schema, stale revision, conflict, policy, and engine failures use
distinct structured error data. Error responses never include workbook bytes,
home paths, environment values, or worker stderr without redaction.
