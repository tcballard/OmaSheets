# OmaSheets

**A spreadsheet for Omarchy, with its own engine and agent changes you can review.**

OmaSheets uses an owned Rust calculation engine, a replayable native document
store, an authenticated local service and a Qt grid. The development build does
not require, start, or ship LibreOffice or LibreOfficeKit. REA was used to inspect
the former document/view boundary; the implementation reuses OmaSheets' owned
Rust stack. See the [investigation and retained evidence](docs/NATIVE-KIT-INVESTIGATION.md).

**Development preview:** native `.omasheets` documents support keyboard editing,
range copy/paste with relative formulas, undo/redo, formatting, charts, checks,
selection-aware Ask Agent and human-reviewed proposals. Supported `.xlsx`
workbooks open through the same grid after strict conversion to a durable native
working copy. The original XLSX stays unchanged. Unsupported source features are
refused before conversion; cached formula results never stand in for unsupported
formulas. XLS, XLSM and ODS are currently refused.

This replaces the product's dependency on LibreOfficeKit. It does not implement
LibreOfficeKit's office-wide C ABI or promise full Excel compatibility. Native
CSV/Parquet exports are explicit value projections; XLSX exports preserve
representable formulas and disclose native-only metadata. They refuse formulas
whose stable references cannot be expressed faithfully in XLSX.

Open **OmaSheets** from the app launcher or run `omasheets`. Create a workbook
with **Ctrl+N**, open one with **Ctrl+O**, and press **F1** for the keyboard guide.
Native edits save when you finish each cell. **Ctrl+Space** opens the searchable
command menu. **Agent → Ask Agent** (Ctrl+Shift+A) starts your configured Omarchy
agent; **Review proposals** (Ctrl+Shift+R) shows proposed edits, derived results
and checks before you approve or reject them.

The [CI workflow](https://github.com/tcballard/OmaSheets/actions/workflows/ci.yml)
builds and installs the product in Arch containers without LibreOffice, verifies
shared-library dependencies, exercises the default XLSX launcher and tests
isolated agent jobs, native review, replay and removal. Physical Wayland input,
accessibility, large-workbook measurements and wider compatibility remain
[v0.1.0 release gates](docs/V0.1-RELEASE.md). The already published v0.0.2 release
is the older compatibility build; these changes are on the development branch.

## Install on Omarchy

1. [Open downloads](https://github.com/tcballard/OmaSheets/releases) and download **omasheets-bin-…-x86_64.pkg.tar.zst** from the newest development build.
2. Update Omarchy, then install the downloaded package with `sudo pacman -U ~/Downloads/omasheets-bin-*.pkg.tar.zst`. If you have several downloads matching that pattern, use the exact filename instead.
3. Open **OmaSheets** from the app launcher, then choose **New workbook**, **Open workbook**, or **Try an example**.

The example is a real, editable budget with a four-step guide. You choose where
to save it. **F1** opens the keyboard reference. Future updates are under
**Help → Updates**. Your workbooks are preserved.

For Omarchy on Linux x86_64. Pacman installs dependencies and registers the app.
No compiler or GitHub login is needed. Download a newer package and repeat
`pacman -U` to update; see [installation and migration details](INSTALL.md).

The optional bar widget is installed separately:

```bash
omarchy plugin add https://github.com/tcballard/OmaSheets.git --enable
```

Development builds are CI prereleases, separate from signed production
releases. [INSTALL.md](INSTALL.md) explains both channels and removal.

Open a workbook and choose **Ask Agent** from the OmaSheets window or Omarchy
bar. OmaSheets asks `omarchy agent prompt` to start an agent session with the
user's configured default agent. The agent starts from the path-free
`omasheets://session` resource, which carries the live selection and workflow
contract. Agents without OmaSheets MCP discovery can use the equivalent bounded
`omasheets agent-session` JSON command bridge; neither surface exposes
publication. Current workflows include formula explanation, bounded data cleanup, variance
analysis, cross-sheet reconciliation, checked summaries and formatting.

## Commands

```bash
omasheets
omasheets doctor
omasheets open workbook.xlsx
omasheets launch workbook.omasheets
omasheets window workbook.xlsx
omasheets select workbook.xlsx
omasheets status --json
omasheets agent-session
omasheets agent-session resource
omasheets agent-session tools
omasheets mcp serve
omasheets-kit probe workbook.xlsx
omasheets-kit import workbook.xlsx workbook.omasheets
omasheets-kit open workbook.omasheets
omasheets-kit export workbook.omasheets copy.xlsx
```

Every file-opening command uses the owned engine. There is no compatibility
fallback and no `lok` or legacy conversion command. XLSX conversion keeps a
working file under the user's OmaSheets data directory; closing the window does
not delete it. Use **Export workbook as Excel** to produce a separate XLSX copy.
The [kit API](crates/omasheets-kit/README.md) exposes the same session boundary.

## Agent work

**Ask Agent** invokes `omarchy agent prompt`. An agent starts from the path-free
`omasheets://session` resource, or the equivalent `omasheets agent-session`
JSON bridge when MCP discovery is unavailable. Live native windows use the
`native_*` tools against the owned service; cell edits persist immediately, and
proposals live on branches until a human approves them in the Qt review window.

For a workbook explicitly selected with `omasheets select`, isolated Rust jobs
provide description, bounded reads, search, tracing, audit, batched queries and
PDF cell previews. Staging supports values/formulas/ranges, clearing, supported
cell formatting, sheet creation/rename/deletion, row/column changes and a
single-key whole-row sort. Fill, chart and pivot operations in this selected-file
job interface are refused. Native UI capabilities have a separate service API.

Selected-file plans bind source identity, operations, evidence and staged output.
The Rust kit exports and reopens a staged workbook before local review; the user
can approve a new copy or an explicit replacement. Agents receive no publication
or undo primitive. `.omasheets` files with live SQLite WAL state must be accessed
through the native service, rather than copied by a selected-file job.

Owned PDF previews show at most eight sheets, fifty rows and twelve columns per
sheet and disclose cropping. They are cell-value previews, not print-layout or
Excel-equivalence evidence.

## Development

```bash
PYTHONPATH=src python -m unittest discover -s tests -q
python scripts/check_release.py
cargo +1.88.0 fmt --all --check
cargo +1.88.0 clippy --workspace --all-targets --locked -- -D warnings
cargo +1.88.0 test --workspace --locked
```

Python remains bootstrap/MCP/process glue; parsing, calculation, workbook jobs
and document editing run in Rust. The release bundle contains only
`omasheets-kit`, `omasheets-service`, `omasheets-grid` and `omasheets-setup`.
The Arch package omits Setup and uses pacman for updates. LibreOffice exists
only in a separate [REA reference investigation job](.github/workflows/native-kit.yml).

The product contracts, roadmap and outstanding native release gates are in
[docs](docs/). This migration stays on a feature branch until review and CI pass.
