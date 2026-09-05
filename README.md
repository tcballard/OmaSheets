# OmaSheets

**A spreadsheet for Omarchy, with agent changes you can review.**

OmaSheets gives you a local spreadsheet window and a way to let your configured
agent inspect workbooks, explain formulas and propose changes. You review the
evidence and approve the result. The native document stack adds a Qt grid,
a Rust formula engine and replayable edit history, with branches for proposed
work and checks before a human-approved merge.

**Development preview:** native `.omasheets` documents support keyboard editing,
range copy/paste with relative formulas, undo/redo, bounded XLSX import, and
CSV, XLSX and Parquet export. Existing Excel and OpenDocument workbooks use the
separate LibreOfficeKit compatibility window. Native import/export reports
what cannot be preserved; it does not promise full Excel fidelity.

**Open OmaSheets:** after installing a matching current build, choose OmaSheets
from the app launcher or run `omasheets`. Create a workbook with **Ctrl+N**,
open one with **Ctrl+O**, and press **F1** for the keyboard guide. Native edits
save when you finish each cell; the File menu offers import and export.
The current development checkout needs a source build or a matching development
bundle; the published v0.0.2 download does not contain this native stack.

The [CI workflow](https://github.com/tcballard/OmaSheets/actions/workflows/ci.yml)
checks replay, rejected edits, clipboard round-trips and compiler-free Arch
installation. Real Omarchy acceptance, refreshed corpus evidence and release
signing remain [v0.1.0 release gates](docs/V0.1-RELEASE.md).

## v0.0.2 scope

- Open `.xls`, `.xlsx`, `.xlsm`, and `.ods` in LibreOffice Calc.
- Inspect workbook structure, values, formulas, named ranges, deduplicated cell
  styles, and formula errors.
- Audit every bounded used range for table structure, missing or duplicate
  headers, duplicate rows, sparse columns, numeric outliers, formula errors,
  existing charts and pivots, and management-summary opportunities.
- Trace formula precedents and dependents within a bounded request.
- Stage cell-value and formula changes against an immutable source hash.
- Stage bounded bulk values, bulk formulas, and typed cell formatting.
- Insert and delete bounded rows or columns, fill formulas with Calc's
  reference-aware engine, and sort bounded ranges.
- Create or update typed column, bar, line, pie and scatter charts; create,
  update and refresh typed pivot tables with bounded sources.
- Require every agent proposal to state its goal, summary, assumptions,
  evidence and purpose-grouped operations.
- Revise a verified proposal by superseding it; never silently mutate a plan
  already presented for review.
- Recalculate, reopen, render, and compare staged output before approval.
- Require a local, explicit approval before publishing workbook bytes.
- Preserve legacy `.xls` originals and convert only to a new `.xlsx` file.
- Expose read and planning operations to agents over MCP; never expose commit.

OmaSheets does not claim perfect Microsoft Excel compatibility. In particular,
macro-enabled `.xlsm` workbooks are read-only in v0.0.2 and legacy conversion
always requires manual review.

The product and safety contracts are in [`docs/`](docs/).
The in-place native-core decision and its measured milestone gates are in
[`ADR-0003`](docs/ADR-0003-EVENT-SOURCED-NATIVE-CORE.md) and the
[`OmaSheets roadmap`](docs/ROADMAP.md). The v0.0.2 release remains the
compatibility and agent-safety baseline. The native document stack now exists
on the development branch, but it is not a released compatibility claim.
The evidence required before the first public native alpha is listed separately
in the [`v0.1.0 native release gate`](docs/V0.1-RELEASE.md).
The native service can also convert a bounded `.xlsx` source into a new
replayable `.omasheets` document. It never replaces an output file and returns
a loss manifest; this is an alpha import path, not a full-fidelity Excel claim.
The same authenticated service binary ships inside the verified native bundle;
the compiler-free Arch acceptance runs its complete local review workflow after
installation.

## Install on Omarchy

**Current checkout:** the command below adds the bar widget. Full installation
requires an explicitly built, source-matching development bundle. Automatic
download cannot use the older v0.0.2 bundle with this checkout, and the next
release still needs its pinned signing key and detached signature.

```bash
omarchy plugin add https://github.com/tcballard/OmaSheets.git --enable
```

The command installs and enables the Omarchy bar surface. Choose **Install
OmaSheets** there to run the privilege-free, user-local bootstrap for the native
Qt grid, compatibility window, local services, Codex plugin, MCP server,
desktop entry and MIME associations. The bootstrap downloads the native
executables built by the
matching GitHub release and verifies their maintainer signature against the
key pinned in the checkout, then their checksum, version, architecture,
source commit, tracked-source digest and individual file hashes, before
anything from the bundle runs. Automatic download is available only when the installed checkout
is exactly the matching `v<version>` release tag; a newer development checkout
fails before network access unless its operator supplies an explicitly built,
source-matching bundle. Users do not need a compiler, CMake, `pkgconf`, or the
LibreOffice SDK. Omarchy intentionally runs no plugin install hooks; missing
runtime dependencies are reported with an explicit `omarchy pkg add` command
for the user to approve. Full installation and reversible removal details are in
[`INSTALL.md`](INSTALL.md).

Open a workbook and choose **Ask Agent** from the OmaSheets window or Omarchy
bar. OmaSheets asks `omarchy agent prompt` to start an agent session with the
user's configured default agent. The agent starts from the path-free
`omasheets://session` resource, which carries the live selection and workflow
contract. Agents without OmaSheets MCP discovery can use the equivalent bounded
`omasheets agent-session` JSON command bridge; neither surface exposes
publication. Flagship v0.0.2 workflows
are formula explanation, bounded data cleanup, variance analysis, cross-sheet
reconciliation, checked summaries, formatting-only cleanup, workbook-wide
audit, and audit-backed management summaries with pivots and charts.

## Commands

```bash
omasheets
omasheets doctor
omasheets launch workbook.omasheets
omasheets launch workbook.xlsx
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

`omasheets` and `omasheets launch` without a path open the start screen, with
New, Open, Import Excel and compatibility-window actions. New workbooks start
with one sheet, 1,000 rows and 26 columns, at the filename you choose. Enter,
Tab or Ctrl+S commits a cell draft; Escape cancels it. Close and reopen the
`.omasheets` file to continue working. File → Export writes a new XLSX workbook
or the current sheet as CSV or Parquet and displays the conversion report.
Creation and export refuse to overwrite existing files.

`omasheets launch FILE` is the production desktop-entry boundary. It opens native
`.omasheets` documents in the Qt grid and compatibility formats in the
LibreOfficeKit window. The Qt grid starts an authenticated native service on
demand, reuses an existing user service when one is already running, and stops
only the transient service it owns after the last launched grid closes. Closing
the window that originally started the service does not interrupt other open
grids. Failed service startup is bounded and cleans up the attempted process.

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
beside cited audit findings and bounded cell-level before/after values,
formulas, formatting, charts and pivots. It
flags destructive operations and truncation and never paints changes into
LibreOfficeKit. The user can hide it or explicitly approve a new, no-clobber
workbook copy from the overlay.

## Status

Early development. v0.0.2 has an automated Arch install/native/agent/uninstall
gate and has received maintainer hands-on testing. The repository does not yet
record evidence for every item in the complete Omarchy/Wayland release runbook,
and no perfect Excel compatibility claim is made.

Large-workbook performance claims are likewise evidence-gated. The
dependency-free [performance harness](docs/PERFORMANCE.md) generates truthful
dense, sparse and formula workloads and records Linux process-tree RSS, PSS and
USS rather than relying on a single sparse used-range smoke file.
