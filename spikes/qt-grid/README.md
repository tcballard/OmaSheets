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

## Arch / Omarchy build

```sh
sudo pacman -S --needed base-devel qt6-base qt6-declarative
cargo test --locked --manifest-path spikes/qt-grid/Cargo.toml
cargo build --locked --release --manifest-path spikes/qt-grid/Cargo.toml
spikes/qt-grid/target/release/omasheets-qt-grid-spike
```

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

The smoke scrolls a synthetic 1,000,000-row by 64-column fixture after 30
warm-up frames and reports 180 frame intervals. Its exact metrics are p95 and
worst frame interval, elapsed time, startup-to-report time, visible delegate
count, and model reads. A headless CI result proves wiring and boundedness only;
it is not product performance evidence.

See `docs/M1-GRID-SPIKE.md` for the on-device review and acceptance record.
