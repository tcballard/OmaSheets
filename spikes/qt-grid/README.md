# Qt grid spike

This is an isolated, unreleased M1 experiment for a keyboard-first native grid.
It uses Qt 6 / Qt Quick with a Rust model through CXX-Qt. It is deliberately
excluded from the root Cargo workspace: passing the spike does not make Qt a
product dependency or change the current LibreOfficeKit compatibility window.

The synthetic fixture exposes 1,000,000 rows by 64 columns without allocating a
cell object for every coordinate. QML creates only the delegates around the
visible viewport. It supports mouse selection, arrow/Page/Home/End navigation,
editing with Enter or F2, and accessibility metadata for the grid and visible
cells.

When given a native `.omasheets` path, the grid connects to the authenticated
local service, exposes its sheets as themed tabs and requests 64-row by
16-column sparse tiles for the selected stable sheet ID. At most eight tiles
(8,192 coordinates) stay cached, and the cache is cleared when switching
sheets. Displayed formula results and editable formula source remain separate,
and edits append through the same service API used by the CLI. The service never
receives one request per painted cell.

On Omarchy, the window reads the active normalized palette from
`~/.config/omarchy/current/theme/colors.toml`. Background, foreground, accent,
muted and semantic colours feed Qt/QML theme properties; panel, grid and
selection shades are derived from them. The window checks for a changed palette
every 1.5 seconds, so an Omarchy theme switch is reflected without restarting.
Missing files or individual invalid colours use the built-in OmaSheets palette,
which also keeps the spike runnable away from Omarchy.

## Arch / Omarchy build

```sh
sudo pacman -S --needed base-devel qt6-base qt6-declarative
cargo test --locked --manifest-path spikes/qt-grid/Cargo.toml
cargo build --locked --release --manifest-path spikes/qt-grid/Cargo.toml
spikes/qt-grid/target/release/omasheets-qt-grid-spike
```

To open a real native document, start the service in another terminal and pass
the document path:

```sh
cargo run --release -p omasheets-service -- serve
OMASHEETS_ACTOR="$USER" \
  spikes/qt-grid/target/release/omasheets-qt-grid-spike document.omasheets
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
  spikes/qt-grid/target/release/omasheets-qt-grid-spike \
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
