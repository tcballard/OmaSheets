# omasheets-calc

This is the owned M0 calculation experiment. It currently proves a bounded
Excel-style expression parser, explicit calculation errors, dependency edges,
cycle rejection and dirty transitive-closure recalculation. It is deliberately
not connected to the v0.0.2 LibreOffice compatibility product.

The first syntax slice is numeric literals, A1 references (including absolute
markers), arithmetic operators, parentheses and `SUM` over arguments or a
bounded rectangular range. Unsupported functions fail explicitly.

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
