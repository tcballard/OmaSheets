# Native kit investigation

Investigation date: 2026-10-10. Source baseline:
`f333b7c263c61fd5ead4df8dce5d6846a44f79c7`. Tool: `rea-agents@6.3.0`, following
the [REA reverse-engineering skill](https://github.com/morluto/rea/blob/main/.agents/skills/reverse-engineer-anything/SKILL.md).
Complete source was inspected with ordinary code tools; REA was applied to the
shipped native artifact and bounded reference-process scenarios.

## Questions and evidence

| Question | Evidence | Finding |
| --- | --- | --- |
| Does OmaSheets need LOK's complete office ABI? | `native/libreofficekit/{window,lok_render}.cpp` | It needs a spreadsheet document/view boundary; the window delegates input and painting to GTK LOKDocView. |
| Which direct rendering methods are used? | `lok_render.cpp` | Init, load, document type, initialize rendering, size, paint tile, tile mode and parts. |
| Which interaction operations are used? | `window.cpp` | Sheets, zoom, viewport, text clipboard, Undo/Redo/Bold/Italic and native GTK input. |
| What must an unsaved snapshot preserve? | `window.cpp`, `src/omasheets/live_bridge.py` | A complete visible document revision, original source identity and source bytes. Whether LOK saveAs changes dirty state was not established. |
| Is the installed reference a shipped native artifact? | [REA artifact Evidence](evidence/native-kit/rea-artifact.json) | ELF dynamic library, 1,528,376 bytes, SHA-256 `1a26a77a054c57e2fef3db9b22d16a328563fe0e37f8d2127d57bb1513a540e5`. Inventory is not decompilation or behavioural proof. |
| Can the old renderer produce a real spreadsheet tile? | Locally compiled `lok_render.cpp`, installed reference, generated M0 XLSX | Observed one 800×500 BGRA tile, one sheet, a valid RGB P6 output, unchanged source SHA-256. This establishes one bounded scenario only. |
| Can the owned engine preserve a simple editable workbook? | Kit session tests and `examples/round_trip.rs` | Ten formulas retained, A2 changed to 20, dependent C2 recalculated to 22; snapshot/save/reopen digest agrees and source bytes remain unchanged. openpyxl independently reads the formula and cached result. |
| Can unsupported formulas masquerade as supported? | Existing service importer plus new admission tests | General conversion may retain cached literals; native-kit opening refuses any cached-only or omitted formula. |
| Can implicit styles and custom themes disappear? | Package style/theme checks and preservation/refusal regressions | Custom inherited style 0, conflicting disabled apply flags and unresolved/custom referenced themes are explicitly refused; explicit supported styling survives save/reopen. |

The local reference is a development LibreOffice build with build ID
`2c87e51eeaa2b413ff4ae097b2705eea1995d8e5`. Its matching public LOK headers were
used only to compile the reference probe. Distribution builds may merge
`libsofficeapp.so` into a larger library; the hosted probe then inspects the
small GTK bridge used by OmaSheets instead. The artifact hash and exact exported
symbol locations are retained in each run's Evidence.

Local REA native layout/process capture could not establish process ownership
because the runtime's Node PID namespace disagreed with procps. Local Unix socket
creation was also refused. Those failures were not interpreted as successful
observations. The hosted `Native kit contract` workflow runs unchanged REA's
`inspect-artifact`, `inspect-binary-layout` and `capture-process` with complete
Evidence retained, and runs mandatory real socket/window-lifetime tests.
Those include opening a native document with live WAL edits through the
authenticated service and interrupting a supervisor while another window
holds a lease. Standalone native probing remains a private-copy operation.

## Reconstruction choices

The replacement uses the existing owned Rust calculation/core/store/service
and production Qt grid. It adds a strict workbook admission/session facade and
a Rust service supervisor. It does not attempt to translate decompiled LibreOffice
code into a new engine or expose LibreOfficeKit's C ABI. Used product operations
map to native `Action`, `GridPage`, snapshot and export interfaces.

| LOK responsibility | Owned replacement |
| --- | --- |
| Workbook/sheet/cell model | Stable-ID core and event store |
| Calculate after edits | Owned calculation graph through revision-guarded service edits |
| Paint, input, scroll, sheets, clipboard and formatting | Production Qt grid and native sheet presentation |
| Unsaved recovery | Durable edit log plus native digest snapshot; dirty remains set |
| Save a Copy | New XLSX projection with explicit manifest and no-clobber publication |
| Hidden format loss | Package admission and actual import counts before writable open |
| Reference observation | REA investigation dependency in CI; absent from product runtime |

The default compatibility launcher and its LibreOffice dependency remain until
wider admission and desktop gates are established. Native-kit operation itself
does not start LibreOffice/UNO. XLS/XLSM/ODS, VBA, pivots, general printing/PDF,
complete Excel semantics/layout, accessibility and physical Wayland input are
unresolved capabilities, not parity claims. This branch is an initial usable
replacement for supported native/XLSX workbooks.
