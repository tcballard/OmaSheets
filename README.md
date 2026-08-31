# OmaSheets

Native spreadsheets for Omarchy, designed for people and agents.

OmaSheets v0.0.1 combines LibreOffice Calc's native Linux spreadsheet engine
with a constrained agent interface. People can open legacy `.xls` files in a
desktop application; agents can inspect, explain, stage, verify, and propose
workbook changes without receiving silent write authority.

## v0.0.1 scope

- Open `.xls`, `.xlsx`, `.xlsm`, and `.ods` in LibreOffice Calc.
- Inspect workbook structure, values, formulas, named ranges, deduplicated cell
  styles, and formula errors.
- Trace formula precedents and dependents within a bounded request.
- Stage cell-value and formula changes against an immutable source hash.
- Stage bounded bulk values, bulk formulas, and typed cell formatting.
- Recalculate, reopen, render, and compare staged output before approval.
- Require a local, explicit approval before publishing workbook bytes.
- Preserve legacy `.xls` originals and convert only to a new `.xlsx` file.
- Expose read and planning operations to agents over MCP; never expose commit.

OmaSheets does not claim perfect Microsoft Excel compatibility. In particular,
macro-enabled `.xlsm` workbooks are read-only in v0.0.1 and legacy conversion
always requires manual review.

The product and safety contracts are in [`docs/`](docs/).

## Current developer commands

```bash
omasheets doctor
omasheets open workbook.xls
omasheets window workbook.xlsx
omasheets select workbook.xlsx
omasheets convert workbook.xls
omasheets status --json
omasheets mcp serve
omasheets lok status
omasheets lok render workbook.xls --output /tmp/workbook-tile.ppm
```

Installation is documented in [`INSTALL.md`](INSTALL.md); the real Omarchy and
LibreOffice release pass is in [`docs/ACCEPTANCE.md`](docs/ACCEPTANCE.md).

An experimental native workbook window is in
[`spikes/libreofficekit/`](spikes/libreofficekit/). It embeds LibreOfficeKit's
interactive tile engine in OmaSheets-owned GTK chrome with scrolling,
selection, keyboard editing, sheets, zoom and save-copy controls. Calc remains
the file-association fallback until real Omarchy/Wayland acceptance passes.

The native window and MCP server share one immutable workbook session. Agents
can observe its bounded, path-free selection and viewport through
`omasheets://window`, inspect the relevant cells, and stage semantic changes;
they cannot drive pointer/keyboard input or publish workbook bytes.

While the window is open, those reads and plans are based on private
LibreOfficeKit save-copies of the exact in-memory document, including unsaved
work. Agents never mutate the visible document; verified output still goes
through local review and live plans are copy-only.

## Status

Early development. The `main` branch remains the minimal bootstrap; v0.0.1 is
being built as a reviewable commit stack on `feat/v0.0.1`.
