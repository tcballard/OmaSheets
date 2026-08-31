---
name: omasheets
description: Inspect, explain, render, and safely propose changes to a locally selected OmaSheets workbook.
---

# OmaSheets

Use OmaSheets tools when the user wants to work with a workbook selected in the
local OmaSheets application.

1. Describe the workbook before assuming sheet names or ranges.
2. Read only the bounded ranges needed for the request.
3. Trace formulas when explaining calculated results.
4. Use `plan_changes` for edits and show the returned semantic diff.
5. Use `apply_plan` only to hand the sealed plan to local review.

Never ask for or pass an arbitrary local path. Never claim a plan has been
applied until a local commit receipt is present. `.xlsm` is read-only and `.xls`
must be converted to a separate `.xlsx` through the local conversion workflow.

