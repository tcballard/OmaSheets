# OmaSheets

Native agentic spreadsheets for Omarchy.

OmaSheets v0.0.1 combines LibreOffice Calc's native Linux spreadsheet engine
with a constrained agent interface. People can open legacy `.xls` files in a
desktop application; agents can inspect, explain, stage, verify, and propose
workbook changes without receiving silent write authority. The first release is
built around a complete agent loop: start from the live selection, inspect the
workbook, cite sealed observations, explain and group a verified proposal,
revise it from human feedback, then hand it back for local approval.

## v0.0.1 scope

- Open `.xls`, `.xlsx`, `.xlsm`, and `.ods` in LibreOffice Calc.
- Inspect workbook structure, values, formulas, named ranges, deduplicated cell
  styles, and formula errors.
- Trace formula precedents and dependents within a bounded request.
- Stage cell-value and formula changes against an immutable source hash.
- Stage bounded bulk values, bulk formulas, and typed cell formatting.
- Insert and delete bounded rows or columns, fill formulas with Calc's
  reference-aware engine, and sort bounded ranges.
- Require every agent proposal to state its goal, summary, assumptions,
  evidence and purpose-grouped operations.
- Revise a verified proposal by superseding it; never silently mutate a plan
  already presented for review.
- Recalculate, reopen, render, and compare staged output before approval.
- Require a local, explicit approval before publishing workbook bytes.
- Preserve legacy `.xls` originals and convert only to a new `.xlsx` file.
- Expose read and planning operations to agents over MCP; never expose commit.

OmaSheets does not claim perfect Microsoft Excel compatibility. In particular,
macro-enabled `.xlsm` workbooks are read-only in v0.0.1 and legacy conversion
always requires manual review.

The product and safety contracts are in [`docs/`](docs/).

## Install on Omarchy

```bash
omarchy plugin add https://github.com/tcballard/OmaSheets.git --enable
```

The command installs and enables the Omarchy bar surface. Choose **Install
OmaSheets** there to run the privilege-free, user-local bootstrap for the native
window, Python service, Codex plugin, MCP server, desktop entry and MIME
associations. Omarchy intentionally runs no plugin install hooks; missing Arch
dependencies are reported with an explicit `omarchy pkg add` command for the
user to approve. Full installation and reversible removal details are in
[`INSTALL.md`](INSTALL.md).

Open a workbook and choose **Ask Agent** from the OmaSheets window or Omarchy
bar. OmaSheets asks `omarchy agent prompt` to start an agent session with the
user's configured default agent. The agent starts from the path-free
`omasheets://session` resource, which carries the live selection and workflow
contract. Agents without OmaSheets MCP discovery can use the equivalent bounded
`omasheets agent-session` JSON command bridge; neither surface exposes
publication. Flagship v0.0.1 workflows
are formula explanation, bounded data cleanup, variance analysis, cross-sheet
reconciliation, checked summaries and formatting-only cleanup.

## Commands

```bash
omasheets doctor
omasheets open workbook.xls
omasheets window workbook.xlsx
omasheets select workbook.xlsx
omasheets convert workbook.xls
omasheets status --json
omasheets agent-session
omasheets agent-session resource
omasheets agent-session tools
omasheets mcp serve
omasheets lok status
omasheets lok render workbook.xls --output /tmp/workbook-tile.ppm
```

The real Omarchy, Wayland and LibreOffice release pass remains in
[`docs/ACCEPTANCE.md`](docs/ACCEPTANCE.md).

The native workbook window source is in
[`native/libreofficekit/`](native/libreofficekit/). It embeds LibreOfficeKit's
interactive tile engine in OmaSheets-owned GTK chrome with scrolling,
selection, keyboard editing, sheets, zoom and save-copy controls. CI exercises
the installed XDG binary under Xvfb; real Omarchy/Wayland acceptance is still a
separate release gate.

The native window and MCP server share one immutable workbook session. Agents
can observe its bounded, path-free selection and viewport through
`omasheets://session`, inspect the relevant cells, and stage semantic changes;
they cannot drive pointer/keyboard input or publish workbook bytes.

While the window is open, those reads and plans are based on private
LibreOfficeKit save-copies of the exact in-memory document, including unsaved
work. Agents never mutate the visible document; verified output still goes
through local review and live plans are copy-only.

Verified proposals appear inside the native window as an OmaSheets-owned diff
overlay. It shows the agent's goal, explanation, assumptions and purpose groups
beside bounded cell-level before/after values, formulas and formatting. It
flags destructive operations and truncation and never paints changes into
LibreOfficeKit. The user can hide it or explicitly approve a new, no-clobber
workbook copy from the overlay.

## Status

Early development. v0.0.1 has an automated Arch install/native/agent/uninstall
gate and has received maintainer hands-on testing. The repository does not yet
record evidence for every item in the complete Omarchy/Wayland release runbook,
and no perfect Excel compatibility claim is made.
