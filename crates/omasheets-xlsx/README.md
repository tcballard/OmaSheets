# omasheets-xlsx

This M0 bridge loads `.xlsx` values and formulas into the owned calculation
engine without changing the v0.0.2 LibreOffice compatibility path. Imports are
bounded before cell materialisation, source bytes are identified by SHA-256,
and formulas outside the owned syntax are retained as explicit unsupported
records while their cached source values remain available.

All workbook sheet names are registered before formula compilation, so formulas
can resolve forward and backward cross-sheet references independent of worksheet
order. Quoted Excel sheet names and escaped apostrophes are supported by the
owned parser; external-workbook and 3D references remain explicit unsupported
formulas.

Shared formulas are expanded by the bridge itself, from the anchor cell that
carries the template, using Calamine's reference shifting. Calamine's own
`worksheet_formula` shifts from the top-left of the shared `ref` range, which
is a different cell whenever that corner carries its own formula, and the
corpus has sheets whose derived cells came out shifted by a column as a
result. Defined names are read from `xl/workbook.xml` with their sheet scope
(`localSheetId`), which Calamine drops.

The bridge reports formula load rate and stored-value comparison separately.
An unsupported formula is never counted as a recalculation match.

Inspect one workbook without emitting cell contents or its local path:

```bash
cargo run --locked --release -p omasheets-xlsx \
  --bin omasheets-xlsx-score -- workbook.xlsx
```
