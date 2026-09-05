# Native Qt grid

This is the keyboard-first production grid for native `.omasheets` documents.
It uses Qt 6 / Qt Quick with a Rust model through CXX-Qt. Its dependency graph
and lockfile remain isolated from the calculation workspace, while the release
builder now compiles, provenance-binds and packages `omasheets-grid`. The
LibreOfficeKit window remains the compatibility path for XLS, XLSX, XLSM and ODS.

The synthetic fixture exposes 1,000,000 rows by 64 columns without allocating a
cell object for every coordinate. QML creates only the delegates around the
visible viewport. It supports mouse selection, arrow/Page/Home/End navigation,
editing with Enter or F2, and accessibility metadata for the grid and visible
cells. Typing starts a replacement value; Enter saves and moves down, Shift+Enter
moves up, and Tab/Shift+Tab save and move horizontally. Ctrl+S saves without
moving. Delete or Backspace clears the selected cell. Escape cancels a draft.
Opening and leaving an unchanged editor does not append a document event.
Prefix input with an apostrophe to keep it as literal text, such as `'00123`,
`'TRUE` or `'=1+1`. The apostrophe is an editing marker, not part of the stored
value. Existing text that resembles a number, boolean or formula uses this
marker in the editor so it retains its type; the grid displays the literal value.

For native documents, Shift+arrow keys extend a rectangular selection. Ctrl+C
copies it and Ctrl+V pastes at its top-left cell. Delete clears the selection.
Copy/paste supports quoted tab-separated text (including embedded tabs and line
breaks), up to 1,000 cells and 1 MiB of clipboard text. Pasted rectangles must
fit in the existing sheet; ragged or oversized input is rejected before writing.
Copies from an OmaSheets grid include source-position clipboard metadata, so
relative A1 references move with the destination, including across windows and
sheets. Absolute `$` axes stay fixed; out-of-grid references become `#REF!`.
Strings, names and structured table selectors retain their source spelling.
Plain text from other applications has no source position and is pasted literally.
Clipboard managers that strip custom MIME data also use this plain-text fallback.
Numeric-looking text retains its apostrophe editing marker.

Ctrl+Z undoes a single edit, clear or entire paste. Ctrl+Shift+Z or Ctrl+Y redoes
it. History is local to the open window, limited to 32 edits and 8 MiB, and is
cleared when the window closes. Undo writes compensating events, retaining the
document's audit history. Each mutation checks its expected document revision;
changes from another window or client cause a refusal until the document is
reopened. A failed paste or undo leaves history and document state unchanged
when the service reports a definitive rejection. Uncertain transport outcomes
still require reopening to verify the saved state.

Failed saves retain the draft and block navigation and window closure until
the user saves successfully or explicitly cancels. A failed initial read blocks
editing. After a transport or protocol failure, copy the draft, cancel it and
reopen the document to check whether the previous edit reached the service;
the grid never automatically retries an uncertain write.

When given a native `.omasheets` path, the grid connects to the authenticated
local service, exposes its sheets as themed tabs and requests 64-row by
16-column sparse tiles for the selected stable sheet ID. At most eight tiles
(8,192 coordinates) stay cached, and the cache is cleared when switching
sheets. Displayed formula results and editable formula source remain separate,
and edits append through the same service API used by the CLI. The service never
receives one request per painted cell. Paint and formula-preview reads load
missing tiles on a worker connection and display a loading marker meanwhile.
At most eight reads are outstanding. Completions return through the Qt event
queue; replies from before an edit or sheet switch are discarded. Explicit
edit/copy reads and saves retain their ordered validation path.

On Omarchy, the window reads the active normalized palette from
`~/.config/omarchy/current/theme/colors.toml`. Background, foreground, accent,
muted and semantic colours feed Qt/QML theme properties; panel, grid and
selection shades are derived from them. The window checks for a changed palette
every 1.5 seconds, so an Omarchy theme switch is reflected without restarting.
Missing files or individual invalid colours use the built-in OmaSheets palette,
which also keeps the grid runnable away from Omarchy.

## Arch / Omarchy build

```sh
sudo pacman -S --needed base-devel qt6-base qt6-declarative qt6-wayland
cargo test --locked --manifest-path spikes/qt-grid/Cargo.toml
cargo build --locked --release --manifest-path spikes/qt-grid/Cargo.toml
spikes/qt-grid/target/release/omasheets-grid
```

An installed product starts the authenticated service on demand and dispatches
native files through the production launcher:

```sh
omasheets launch document.omasheets
```

The document must already contain at least one sheet, row and column. Select a
sheet with the tab strip or switch with Ctrl+Page Up and Ctrl+Page Down.

## Evidence smoke

Run this on an otherwise idle machine. The first run is cold; run the command
again for warm evidence. Keep the hardware, compositor, renderer, Qt version,
and cold/warm state with the result.

```sh
mkdir -p out/qt-grid
/usr/bin/time -v -o out/qt-grid/resource.txt \
  env OMASHEETS_GRID_BENCHMARK=1 \
  spikes/qt-grid/target/release/omasheets-grid \
  > out/qt-grid/report.txt 2> out/qt-grid/stderr.txt
python scripts/check_qt_grid_spike.py --report out/qt-grid/report.txt
```

With no document path, the smoke scrolls the synthetic 1,000,000-row by
64-column fixture after 30 warm-up frames and reports 180 frame intervals. Its exact metrics are p95 and
worst frame interval, elapsed time, startup-to-report time, visible delegate
count, and model reads. A headless CI result proves wiring and boundedness only;
it is not product performance evidence.

The report also records `theme_source` as `omarchy` or `fallback`. For an
on-Omarchy evidence run, add `--require-omarchy-theme` to the checker command.
For a native-document run, add `--require-native-document`; this also proves
that service requests are bounded below the number of model cell reads.
For a two-sheet acceptance fixture, add `--require-multi-sheet`; the benchmark
switches to the second stable sheet ID before writing its persistence probe.

See `docs/M1-GRID-SPIKE.md` for the on-device review and acceptance record.
