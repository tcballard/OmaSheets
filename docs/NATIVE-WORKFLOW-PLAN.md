# Native spreadsheet completion plan

Owner authorisation: 7 September 2026. Implement the gaps identified in the
Gridline comparison, using engineering judgement and asking Tom about taste.
Keep the accepted Qt/Rust stack and Omarchy palette. Work in reviewable stacked
`codex/` branches; merging and release publication still require owner approval.

## 1. A complete native agent workflow

- Ask Agent from the native grid captures the selected document, sheet, range
  and revision and launches the configured Omarchy agent.
- MCP and the bounded command bridge expose path-free native inspection,
  formula lineage and proposed edits. They expose no approval, export or raw
  service request. Each call identifies its session; switching workbooks cannot
  silently redirect an existing agent.
- Proposed changes are created atomically on a separate durable branch, with
  goal, explanation, assumptions and evidence. A stale source is refused.
- The window lists proposals and presents cell before/after values and formulas,
  explanations, check results and conflicts. Review uses the prospective merged
  state. Approval binds both branch revisions and refuses truncation, changed
  evidence, conflicts or failing checks. Rejection preserves proposal history.
- Approval refreshes the grid from the saved result. Closing and reopening must
  recover the same state. Transport uncertainty never triggers automatic writes.

## 2. Everyday spreadsheet tools

- Search/go-to and formula editing; discoverable undo/redo, fill down/right and
  keyboard commands; selection statistics and find/replace.
- Add/rename/duplicate/delete sheets; insert/delete rows and columns; resize,
  autofit, freeze and grid visibility controls.
- Durable cell presentation: emphasis, colours, alignment, wrap, basic borders
  and number/date/currency/percent formats. Format changes participate in replay,
  undo, diff and exports rather than existing only in QML.
- Header-aware sorting, filters and duplicate removal; notes and conditional
  formatting; bounded native charts derived from the document snapshot.
- Preserve stable identities and formula bindings through structural changes.
  Report unsupported operations explicitly rather than approximating them.

## 3. Native interchange and formula coverage

- Preserve the supported style, dimension, merge and pane information in native
  XLSX conversion. Make unavoidable losses readable and specific.
- Keep original files and no-replacement publication. Retain LibreOfficeKit for
  compatibility-sensitive workbooks, legacy XLS and read-only XLSM.
- Add useful deterministic formula gaps, including TEXTJOIN. Clock/random
  functions require stored explicit inputs/ticks, preserving deterministic
  replay; never introduce hidden time or randomness into calculation.
- Keep the corpus denominator and current refusal policy explicit. Refresh
  aggregate evidence when available; never claim an unmeasured corpus delta.

## 4. Verification and owner review

- Exercise the installed create/import, edit, propose, review, reject/approve,
  export, close/reopen workflow, plus stale review and failed-write paths.
- Verify formatting and formula round-trips with independent workbook readers;
  preserve the old native replay fixtures.
- Use declared dense/sparse/formula fixtures for resource measurements. Keep
  container wiring evidence separate from Omarchy hardware measurements.
- Capture real UI examples for Tom to judge control density and proposal review
  presentation. Complete the engineering work before presenting taste choices.
- Produce stacked PRs with exact checks and an honest corpus-delta statement.
  Hardware acceptance, corpus availability and owner release decisions remain
  explicit when this environment cannot supply them.

## Implementation record — 8 September 2026

- #81 (`codex/native-agent-review`): native agent inspection, atomic proposals,
  prospective review, revision-bound approval, durable rejection and installed
  workflow. CI passed at `c976de7`.
- #82 (`codex/native-spreadsheet-controls`): durable presentation, everyday tools,
  native menus/formula bar/viewport, and failed-draft recovery. CI passed at
  `bd1a693`.
- #83 (`codex/native-interchange-formulas`): supported XLSX presentation,
  independently checked round-trips, exact current formula projection and
  TEXTJOIN. The installed Qt/interchange checks passed at `55598a0`; visual
  review then identified and corrected a Qt 6.4 chart compatibility issue and
  initial style-read timing. Capture diagnostics now reject those errors.

The supported limits and intentional refusals are documented in
[NATIVE-SPREADSHEET-SUPPORT.md](NATIVE-SPREADSHEET-SUPPORT.md). The native window
and dialogs are captured for owner taste review. Release, merge, larger-corpus
scoring and target-hardware acceptance remain separate decisions/evidence.

## Keyboard menu follow-up — 8 September 2026

Tom accepted the compact controls, then requested Omarchy's Super+Space menu
interaction for the spreadsheet tools. `codex/keyboard-command-menu` replaces
the toolbar and menu bar with a searchable hierarchy, uses Ctrl+Space inside
the application, and preserves Super+Space for the desktop. One command
catalogue supplies both menu actions and direct shortcuts. Dialog focus,
keyboard search results and exact colour entry complete the keyboard path;
Qt key-event tests cover draft recovery and modal boundaries.
