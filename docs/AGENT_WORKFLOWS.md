# Agentic workbook workflows in v0.0.1

OmaSheets treats an agent run as a reviewable workbook workflow, not a chat box
with ambient write access.

The person opens or selects a workbook, focuses a cell or range, and starts
**Ask Agent**. Omarchy launches the user's configured default agent for an
agent session. The agent reads `omasheets://session`, inspects only the workbook
regions needed for the goal, and retains the sealed `evidence_id` from each
read. When two or more independent structure, range, search or formula-trace
reads are known up front, `query_workbook` runs up to eight of them against one
live snapshot and one Calc load, returning one ordered result and evidence
record. A proposal must include:

- the outcome the agent believes the person wants;
- a concise explanation of the proposed result;
- material assumptions;
- evidence IDs from the exact selected-file or live-window revision; and
- purpose groups that cover every typed operation exactly once.

OmaSheets recalculates, saves, reopens and fingerprints the staged result. Its
native overlay presents the explanation and the exact bounded before/after
changes without changing the visible workbook. Human feedback produces a new
plan through `revise_plan`; the previous plan becomes immutable, superseded
history. Only the local person can approve publication.

## Flagship workflows

- **Explain:** describe a selected result and trace bounded formula precedents;
  no plan is necessary when the request is read-only.
- **Clean:** normalise bounded imported values and optionally sort the reviewed
  range.
- **Variance:** create formulas, fill them with Calc's reference-aware engine,
  and apply typed number or emphasis formatting.
- **Reconcile:** inspect two sheets, search bounded identifiers, and create a
  separate checked result sheet.
- **Summarise:** create a purpose-labelled summary sheet with exact values or
  formulas and presentation formatting.
- **Format:** standardise number formats, emphasis, colours and wrapping without
  changing cell values.
- **Analyse:** run a deterministic bounded audit across every used sheet and
  return reviewable findings, column profiles, existing workbook objects and
  management-summary opportunities as sealed evidence.
- **Management:** cite the workbook audit, inspect only the supporting ranges,
  then propose a purpose-labelled summary sheet with typed pivot tables and
  charts. The agent chooses the useful presentation; OmaSheets constrains and
  verifies the workbook mutations.

Charts are upserts: a named chart is created or replaced from an explicit
source and anchor range. Pivots can likewise be upserted from named row,
column, filter and aggregate fields, or refreshed by name. These operations
remain staged proposals. In `.xlsx`, pivot compatibility carries an explicit
manual-review warning because Calc and Excel may render pivot details
differently.

The deterministic workflow catalog in `tests/fixtures/agent_workflows.json`
proves that each flagship job is expressible through the public v0.0.1 tools.
The Arch acceptance job additionally drives the installed MCP server through
inspection, an evidence-cited structural/formula/sort proposal, revision and
the non-publishing local-review handoff.

The same tool schemas are exposed through MCP and the provider-neutral command
bridge used by Omarchy's configured default agent. Contract tests cover both
surfaces; release acceptance still repeats the workflows with Codex and a
non-Codex agent because deterministic protocol compatibility is not the same as
model-behaviour proof.

This does not claim that every model run will choose the right interpretation.
Evidence seals prove which observations a proposal cites; they do not prove the
agent's explanation is correct. Ambiguity remains a reason to ask the person,
and the semantic diff remains the authority for what would change.
