# OmaSheets native fast-core spike

This isolated Rust program measures one candidate for the future OmaSheets
workbook kernel. It loads `.xlsx` through Formualizer's Calamine adapter,
constructs the Arrow-backed workbook model, recalculates formulas, and emits a
bounded JSON timing and inventory report.

It is deliberately **not** part of the installed product. LibreOffice
remains the compatibility and publication authority until a candidate passes
the workbook corpus, performance budgets, and save/reopen checks.

```bash
cargo run --manifest-path spikes/fastcore/Cargo.toml --release -- \
  inspect path/to/workbook.xlsx
```

The spike accepts `.xlsx` only, does not write the workbook, does not invoke
network-capable formula functions, and reports no cell contents or local path.

## First local evidence

An Apple Silicon release build was 9.9 MiB. It loaded and evaluated a
LibreOffice-converted workbook containing 2,020 non-formula cells in 17.9 ms
total. A separate generated formula fixture was correctly kept out of the
product path: LibreOfficeDev exported its OpenFormula prefix literally as
`of:=A2+B2`, and the Calamine/Formualizer import rejected that invalid OOXML
formula instead of silently inventing a result.

Those are development observations, not cross-platform product claims. The
candidate must pass the Linux dense/formula corpus, formula-fidelity cases,
resource caps, and LibreOffice save/reopen comparison before any installed
fast path is considered.
