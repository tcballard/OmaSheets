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

## Commands

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
`omasheets://window`, inspect the relevant cells, and stage semantic changes;
they cannot drive pointer/keyboard input or publish workbook bytes.

While the window is open, those reads and plans are based on private
LibreOfficeKit save-copies of the exact in-memory document, including unsaved
work. Agents never mutate the visible document; verified output still goes
through local review and live plans are copy-only.

Verified proposals appear inside the native window as an OmaSheets-owned diff
overlay. It shows bounded cell-level before/after values, formulas and
formatting, flags destructive operations and truncation, and never paints
changes into LibreOfficeKit. The user can hide it or explicitly approve a new,
no-clobber workbook copy from the overlay.

## Status

Early development. v0.0.1 has an automated Arch install/native/uninstall gate,
but no claim of hands-on Omarchy/Wayland release acceptance or perfect Excel
compatibility.
