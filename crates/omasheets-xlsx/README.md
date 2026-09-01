# omasheets-xlsx

This M0 bridge loads `.xlsx` values and formulas into the owned calculation
engine without changing the v0.0.2 LibreOffice compatibility path. Imports are
bounded before cell materialisation, source bytes are identified by SHA-256,
and formulas outside the owned syntax are retained as explicit unsupported
records while their cached source values remain available.

The bridge reports formula load rate and stored-value comparison separately.
An unsupported formula is never counted as a recalculation match.

Inspect one workbook without emitting cell contents or its local path:

```bash
cargo run --locked --release -p omasheets-xlsx \
  --bin omasheets-xlsx-score -- workbook.xlsx
```
