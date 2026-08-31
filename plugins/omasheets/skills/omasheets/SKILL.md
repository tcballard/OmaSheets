---
name: omasheets
description: Inspect, explain, render, and safely propose changes to a locally selected OmaSheets workbook.
---

# OmaSheets

Use OmaSheets tools when the user wants to work with a workbook selected in the
local OmaSheets application.

Prefer the installed OmaSheets MCP tools. If this agent does not discover MCP,
use the provider-neutral JSON command bridge instead:

- `omasheets agent-session resource`
- `omasheets agent-session tools`
- `omasheets agent-session call TOOL --arguments 'JSON_OBJECT'`

The bridge exposes the same bounded read, plan, revision and review-handoff
surface as MCP. It does not expose workbook publication.

1. Read `omasheets://session` first. Treat its current selection as context, not
   as permission to edit or as a complete statement of the user's goal.
2. Describe the workbook before assuming sheet names or ranges.
3. Read only the bounded ranges needed for the request. Retain the
   `evidence_id` returned by each inspection you rely on.
   When several independent describe, range, search, or formula-trace reads are
   known up front, prefer `query_workbook` so they share one exact snapshot and
   Calc load. Do not put `session_id` inside its subqueries.
   For workbook-wide audit or management-summary requests, run
   `analyze_workbook`; use its bounded table profiles, findings and summary
   opportunities as evidence before reading any supporting ranges.
4. Trace formulas when explaining calculated results.
5. Before planning, state the goal, important assumptions, and any ambiguity
   that could materially change the result. Ask the user when necessary.
6. Use `plan_changes` with a concise summary, cited evidence IDs, and
   purpose-groups that cover every operation exactly once. Explain the returned
   semantic diff in those same groups.
7. If the user changes the requested result, use `revise_plan`; never present a
   superseded plan as current.
8. Use `apply_plan` only to hand the sealed plan to local review.

Good first workflows are:

- `explain`: explain a selected formula or result.
- `clean`: clean a bounded data range.
- `variance`: build or explain a variance analysis.
- `reconcile`: reconcile values across two sheets.
- `summarise`: create a checked summary.
- `format`: standardise presentation without changing values.
- `analyse`: audit the whole workbook for structure, quality, formula errors,
  anomalies, charts, pivots and summary opportunities.
- `management`: create a reviewable summary sheet using typed pivot, chart,
  value and formatting operations based on cited audit evidence.

Never ask for or pass an arbitrary local path. Never claim a plan has been
applied until a local commit receipt is present. `.xlsm` is read-only and `.xls`
must be converted to a separate `.xlsx` through the local conversion workflow.
