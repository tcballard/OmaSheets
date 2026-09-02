# omasheets-calc

This is the owned M0 calculation experiment. It currently proves a bounded
Excel-style expression parser, explicit calculation errors, dependency edges,
cycle rejection and dirty transitive-closure recalculation. It is deliberately
not connected to the v0.0.2 LibreOffice compatibility product.

The current syntax slice includes numeric, boolean and quoted-text literals;
A1 references (including absolute markers); unquoted (`Inputs!A1`) and quoted
(`'Owner''s Data'!A1`) cross-sheet references; arithmetic and comparison
operators; parentheses; and bounded rectangular ranges. Forty-five function
names are implemented across the initial head:

- aggregate: `SUM`, `AVERAGE`, `MIN`, `MAX`, `COUNT`, `COUNTA`, `PRODUCT`;
- math: `ABS`, `ROUND`, `ROUNDUP`, `ROUNDDOWN`, `INT`, `MOD`, `POWER`, `SQRT`;
- logical: `IF`, `AND`, `OR`, `NOT`, `IFERROR`.
- conditional aggregate: `COUNTIF`, `SUMIF`;
- exact lookup: `INDEX`, `MATCH`, `VLOOKUP`, `XLOOKUP`;
- extended math: `SIGN`, `CEILING`, `FLOOR`, `TRUNC`, `EXP`, `LN`, `LOG`,
  `LOG10`, `PI`;
- text: `LEN`, `LEFT`, `RIGHT`, `MID`, `TRIM`, `UPPER`, `LOWER`, `CONCAT`
  (`CONCATENATE` alias), `VALUE`, `EXACT`.

`IF` and `IFERROR` evaluate only the selected branch. Unsupported functions,
invalid arity, invalid coercions and oversized ranges fail explicitly rather
than silently producing a plausible number.

The first `COUNTIF`/`SUMIF` criteria slice supports comparison prefixes and
case-insensitive literal text. Wildcards and locale-specific number parsing are
not yet accepted.

Cross-sheet ranges qualify the first endpoint (`Inputs!A1:A10`). External
workbook references, 3D references, and a separately qualified second range
endpoint are not yet accepted.

The lookup slice is deliberately exact-match only. `MATCH` accepts an omitted
or zero match mode, `VLOOKUP` requires `FALSE`, and `XLOOKUP` supports its
optional not-found result. Approximate, wildcard, reverse, and binary-search
modes remain explicit invalid arguments until their compatibility semantics are
implemented and tested.

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

This owned implementation exists for comparison against candidate libraries;
passing its unit tests is not evidence of `.xlsx` compatibility or the M0
performance exit gate.
