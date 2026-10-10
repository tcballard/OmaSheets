# OmaSheets Kit

An owned Rust spreadsheet-session replacement for the LibreOfficeKit path used
by OmaSheets. It combines the existing calculation engine, stable-ID document
model, event store, workbook service and Qt grid. The kit has no LibreOffice or
UNO runtime dependency. REA is used to investigate the reference boundary and
record evidence; it is not required to run the replacement.

The development build uses this kit for its default launcher and selected-file
agent jobs. The LibreOfficeKit and UNO product launchers are removed. Native
documents and strictly admitted XLSX are supported; wider format and desktop
acceptance remain release gates. The kit exposes OmaSheets' own API, with no
implementation of LibreOfficeKit's general office-document ABI.

## Run

The development native bundle installs `omasheets-kit` and routes the normal
`omasheets` launcher through the owned engine. It must be built from this source
revision; the published v0.0.2 bundle still uses the older compatibility runtime.

```sh
omasheets launch budget.xlsx
omasheets-kit probe budget.xlsx
omasheets-kit open budget.xlsx
omasheets-kit open budget.xlsx --working budget.omasheets
omasheets-kit import budget.xlsx budget.omasheets
omasheets-kit open budget.omasheets
omasheets-kit export budget.omasheets copy.xlsx
omasheets-kit job request.json result.json
```

`probe` returns JSON with `can_import`, source SHA-256, reasons and the actual
import manifest. A refused admission exits unsuccessfully. `open` runs the
production Qt grid against the authenticated owned document service. Without
`--working`, it chooses a new private working file under
`$XDG_DATA_HOME/omasheets/native-kit` (or `~/.local/share/omasheets/native-kit`)
and reports its path. The file survives window closure for recovery. Save and
reopen that native file to continue; Workbook → Export writes an XLSX copy.
The original XLSX is never overwritten.

For development, build the kit/service and existing grid with Qt development
packages installed:

```sh
cargo build --locked -p omasheets-kit -p omasheets-service
cargo build --locked --manifest-path spikes/qt-grid/Cargo.toml
OMASHEETS_NATIVE_SERVICE="$PWD/target/debug/omasheets-service" \
OMASHEETS_GRID="$PWD/spikes/qt-grid/target/debug/omasheets-grid" \
target/debug/omasheets-kit open budget.xlsx
```

`XDG_RUNTIME_DIR` must identify a private runtime directory, or pass
`--runtime-dir DIR`. All grid supervisors share startup and client leases;
the supervisor that starts a service waits for the other windows before
stopping it. An existing authenticated service is reused and remains running.
Native `open` validates the current document through that service, including
uncheckpointed edits from another window. Terminal interruption is forwarded
to the supervisor's own grid; peer windows retain their service lease.

## Session API

`WorkbookSession` exposes sheets, bounded calculated viewports, presentation,
revision-guarded `Action` edits, undo/redo, recovery snapshots and no-clobber
XLSX-copy export. `open_xlsx_checked` can bind opening to a hash returned by
an earlier probe. A private immutable source snapshot is inspected and imported;
the native working file is published only after admission succeeds and is
verified by replay. Snapshotting captures a native digest without clearing
the session's export-dirty flag. Edits are durably stored immediately.

The UI uses the existing service/grid interfaces, including native agent review.
It does not emulate `.uno` commands, GTK signals or the legacy LOK live-snapshot
socket. Callers of those interfaces must migrate to native document operations.

## Isolated workbook jobs

`job` reads a bounded JSON request and writes a structured result for describe,
read, search, trace, query, audit, PDF cell preview or staged edits. The existing
Python adapter supplies private source copies and the networkless Bubblewrap
sandbox; parsing, calculation, editing and preview generation run in Rust.
Selected-file jobs refuse active native WAL state; live native windows use the
authenticated service instead.

Staging supports cell/range values and formulas, clearing, supported formatting,
sheet and row/column changes and single-key sorting across the full used row.
Selected-file fill, chart and pivot operations are explicitly refused. XLSX
stages export and reopen for preservation checks before a separate human review
and publication. Each query batch preserves order under one evidence record.
Trace reports immediate resolved inputs with an explicit depth limit. PDF
previews disclose cropping and styling limitations; they show at most eight
sheets, fifty rows and twelve columns per sheet.

## Admission and limits

Initial XLSX admission accepts values and formulas that translate completely
to native cells, plus the checked presentation subset. It refuses cached-only
formulas, source error cells, defined names, external links, VBA, pivots, source
tables, drawing/chart/comment parts, validation, array/spill metadata and
unknown XML objects/attributes. More presentation forms can be added after
preservation tests; some already supported by the general converter are
deliberately excluded from this stricter writable-open path.
Implicit custom default styles, custom referenced themes and disabled style
application flags are refused when the native converter cannot preserve them.

Limits: 50 MiB source, 64 sheets, 100,000 occupied-rectangle cells, 2,048 package
parts, 32 MiB per expanded XML part and 128 MiB total expanded content. XLS,
XLSM, ODS and encrypted workbooks are explicit unsupported capabilities.
Native probing uses a private database copy and refuses an active nonempty WAL.
Close an active workbook before standalone probing; native `open` can reuse
its live authenticated service.

Export projects a new XLSX package. Native event history, checks, watches and
lineage remain in `.omasheets`; the export manifest describes omissions. Native
defaults are used for font families, border colours and some layout settings.
The kit refuses export when a stable formula binding cannot be represented in
current A1 coordinates. General printing/PDF, macros, pivots, full layout fidelity
and physical Omarchy/Wayland acceptance are not implemented or established here.

## Verification

```sh
cargo test --locked -p omasheets-kit
```

Tests exercise open/edit/dependent recalculation/save/reopen, unchanged source
bytes, snapshots retaining dirty state, revision conflicts, undo/redo,
no-clobber, loss refusals, malformed colours and service/window lifetimes.
`.github/workflows/native-kit.yml` also checks the minimum Rust toolchain,
reopens exported formulas/caches with openpyxl, opens XLSX through the real Qt
grid, and records complete REA reference evidence as CI artifacts.

See [the investigation ledger](../../docs/NATIVE-KIT-INVESTIGATION.md) for
observations, source anchors and unresolved parity claims.
