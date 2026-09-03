# omasheets-calc

This is the owned M0 calculation experiment. It currently proves a bounded
Excel-style expression parser, explicit calculation errors, dependency edges,
cycle rejection and dirty transitive-closure recalculation. It is deliberately
not connected to the v0.0.2 LibreOffice compatibility product.

The current syntax slice includes numeric, boolean and quoted-text literals;
A1 references (including absolute markers); unquoted (`Inputs!A1`) and quoted
(`'Owner''s Data'!A1`) cross-sheet references; arithmetic and comparison
operators; parentheses; and bounded rectangular ranges. Forty-nine function
names are implemented across the initial head:

- aggregate: `SUM`, `AVERAGE`, `MIN`, `MAX`, `COUNT`, `COUNTA`, `PRODUCT`;
- math: `ABS`, `ROUND`, `ROUNDUP`, `ROUNDDOWN`, `INT`, `MOD`, `POWER`, `SQRT`;
- logical: `IF`, `AND`, `OR`, `NOT`, `IFERROR`.
- conditional aggregate: `COUNTIF(S)`, `SUMIF(S)`, `AVERAGEIF(S)`;
- exact lookup: `INDEX`, `MATCH`, `VLOOKUP`, `XLOOKUP`;
- extended math: `SIGN`, `CEILING`, `FLOOR`, `TRUNC`, `EXP`, `LN`, `LOG`,
  `LOG10`, `PI`;
- text: `LEN`, `LEFT`, `RIGHT`, `MID`, `TRIM`, `UPPER`, `LOWER`, `CONCAT`
  (`CONCATENATE` alias), `VALUE`, `EXACT`.

`IF` and `IFERROR` evaluate only the selected branch. Unsupported functions,
invalid arity, invalid coercions and oversized ranges fail explicitly rather
than silently producing a plausible number.

`COUNTIF`/`SUMIF`-family criteria follow Excel: a comparison prefix (`>=`,
`<>`, ...), case-insensitive text with its spacing kept (`"Ltd "` matches only
`"Ltd "`), and the wildcards `?` and `*` with `~` as the escape. Types never
cross except that numeric text counts as a number; blank cells and errors in
the criteria range satisfy only `<>` tests, `""` and `<>` test for blank, a
criterion read from an empty cell is 0, and an error criterion counts the cells
holding that error. Negative zero is folded into zero everywhere arithmetic
produces it, so `-1*0` compares equal to `0` and prints as `0`. Locale-specific
number parsing is not attempted.

Cross-sheet ranges qualify the first endpoint (`Inputs!A1:A10`). External
workbook references, 3D references, and a separately qualified second range
endpoint are not yet accepted.

The lookup slice is deliberately exact-match only. `MATCH` accepts an omitted
or zero match mode, `VLOOKUP` requires `FALSE`, and `XLOOKUP` supports its
optional not-found result. Approximate, wildcard, reverse, and binary-search
modes remain explicit invalid arguments until their compatibility semantics are
implemented and tested.

Ranges are shared graph nodes. `A1:A1000` in a formula compiles to one node;
every formula over the same rectangle, including every formula that expands
the same defined name, points at that node. A rectangle node keeps no
per-member edges: membership is decided by position, a changed cell finds the
rectangles covering it through a per-sheet row-band index, and evaluation
scans the sheet's position index for the cells that exist inside the
rectangle rather than materialising blanks. A formula therefore costs its own
expression plus one edge per range, however large the range. A rebinding that
no longer forms a rectangle (a row inserted through a range bound to stable
cells) keeps its original members as an explicit list with its own node.

Defined names follow Excel's two rules that the corpus exercises most. A
token is a cell reference only inside the grid (`XFD1048576` is the last
cell): `Table1`, `pipe2` or `Curves1` are names, never the cells in columns
TABLE, PIPE or CURVES. A name scoped to a sheet (`localSheetId` in a
workbook file, `define_sheet_name` here) is seen only by formulas on that
sheet and shadows a workbook-level name of the same spelling there; every
other sheet sees the workbook name.

Inside an aggregate argument, an expression that combines ranges with
operators or scalar functions is evaluated elementwise, as Excel evaluates a
CSE or dynamic-array formula: `SUM(IF(B1:B5=0,0,C1:C5-B1:B5))`,
`SUM(A1:A5*B1:B5)`, `SUMPRODUCT((A1:A5>2)*B1:B5)`, `MAX(ABS(A1:A5-3))`. A
scalar or a single row/column broadcasts to the other operand's shape, cells
past a shorter operand are `#N/A`, and the intermediate array is bounded by
the same cell limit as a range. A range alone in scalar position still
intersects implicitly, and a formula's own result is always one value.

Run its focused checks with:

```bash
cargo test --locked -p omasheets-calc
```

Measure a worst-case linear dirty closure in a release build with:

```bash
cargo run --locked --release -p omasheets-calc \
  --bin omasheets-calc-bench -- --formulas 100000 --iterations 20
```

The command emits bounded JSON with construction time and edit-to-recalculation
p50, p95 and maximum durations. It deliberately reports measurements without
turning the roadmap target into a hardware-independent test assertion.

`--fixture` selects the dependency shape: `linear` (default) chains every
formula on the previous one so one root edit re-evaluates the whole document,
`fan_out` points every formula at the root, and `sparse` builds independent
chains of 1000 and edits one chain's root so one edit re-evaluates about 1000
cells. `docs/M0-REVIEW.md` records the measurements taken with these fixtures
against the M0 exit gate.

This owned implementation exists for comparison against candidate libraries;
passing its unit tests is not evidence of `.xlsx` compatibility or the M0
performance exit gate.
