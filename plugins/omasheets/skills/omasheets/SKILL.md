---
name: omasheets
description: Inspect, explain, render, and safely propose changes to a locally selected OmaSheets workbook.
---

# OmaSheets

Use OmaSheets tools when the user wants to work with a workbook selected in the
local OmaSheets application.

1. Read `omasheets://agent` first. Treat its current selection as context, not
   as permission to edit or as a complete statement of the user's goal.
2. Describe the workbook before assuming sheet names or ranges.
3. Read only the bounded ranges needed for the request. Retain the
   `evidence_id` returned by each inspection you rely on.
4. Trace formulas when explaining calculated results.
5. Before planning, state the goal, important assumptions, and any ambiguity
   that could materially change the result. Ask the user when necessary.
6. Use `plan_changes` with a concise summary, cited evidence IDs, and
   purpose-groups that cover every operation exactly once. Explain the returned
   semantic diff in those same groups.
7. If the user changes the requested result, use `revise_plan`; never present a
   superseded plan as current.
8. Use `apply_plan` only to hand the sealed plan to local review.

Good first workflows include explaining a selected formula, cleaning a bounded
data range, building a variance analysis, reconciling two sheets, creating a
checked summary, and standardising presentation without changing values.

Never ask for or pass an arbitrary local path. Never claim a plan has been
applied until a local commit receipt is present. `.xlsm` is read-only and `.xls`
must be converted to a separate `.xlsx` through the local conversion workflow.
