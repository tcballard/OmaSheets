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
omasheets select workbook.xlsx
omasheets convert workbook.xls
omasheets status --json
omasheets mcp serve
omasheets lok status
omasheets lok render workbook.xls --output /tmp/workbook-tile.ppm
```

Installation is documented in [`INSTALL.md`](INSTALL.md); the real Omarchy and
LibreOffice release pass is in [`docs/ACCEPTANCE.md`](docs/ACCEPTANCE.md).

An experimental native-rendering proof is in
[`spikes/libreofficekit/`](spikes/libreofficekit/). It renders a real `.xls`
through LibreOfficeKit into an OmaSheets-owned pixel buffer; it is not yet the
default human workbook window.

## Status

Early development. The `main` branch remains the minimal bootstrap; v0.0.1 is
being built as a reviewable commit stack on `feat/v0.0.1`.
