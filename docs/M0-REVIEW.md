# M0 exit review: native engine and corpus spike

- Status: **Go approved by the maintainer.** The measurements below were taken
  on a hosted container, not on the eight-core x86 baseline the roadmap
  declares. That missing baseline remains release evidence to collect; it is
  not represented as part of the approval.
- Gate under review: `docs/ROADMAP.md`, "M0 — native engine and corpus
  spike". Targets: incremental recalculation below 10 ms p95 for one edit in
  a 100,000-formula document; full recalculation of one million simple
  formulas below 1.5 s. Stop condition: incremental recalculation above
  25 ms p95 after one rearchitecture attempt.

## What M0 built

| Roadmap item | Where it lives |
| --- | --- |
| Rust workspace: core types, parsing, dependency graph, calculation | `crates/omasheets-calc` |
| Fetched, not vendored, Enron corpus with a frozen 1,000-workbook sample | `corpus/`, `scripts/fetch_corpus.py`, `scripts/sample_corpus.py` |
| Open/parse/recalculate scorer, two isolated lanes | `crates/omasheets-corpus` |
| Excel-syntax subset, dependency graph, cycle detection, dirty transitive closure in topological order | `omasheets-calc` (`Workbook`, `RecalcReport`) |
| High-frequency functions with explicit error semantics | 83 registered names, `docs/FUNCTIONS.md` |
| `.xlsx` import through Calamine, source retained, unsupported features recorded | `crates/omasheets-xlsx` |
| Synthetic 100,000 and one-million formula benchmarks | `omasheets-calc-bench` (`linear`, `fan_out`, `sparse` fixtures) |
| Candidate library compared with the owned implementation | corpus `probe` (Formualizer) and `probe-owned` lanes |

Volatile values are excluded by decision, not omission: `docs/ADR-0004`
keeps clock-dependent functions out of the engine, and `RAND`/`OFFSET` are
reported as unsupported rather than approximated. EUSES is not yet
registered; only Enron is scored.

## Method

- Binary: `omasheets-calc-bench`, release profile (`codegen-units = 1`,
  `lto = "thin"`), single thread, one process, no I/O.
- Measurement: wall time of one `Workbook::set_number` on the edited cell,
  which covers dirty marking, scheduling and evaluation of the whole
  transitive closure. `build_ns` covers parsing every formula, inserting it
  into the graph and its first evaluation, and is reported separately as the
  roadmap requires.
- State: warm process; the document is built once, then edited N times.
  p50/p95 use nearest rank over the N samples.
- Fixtures: `linear` chains every formula on the previous one, so one root
  edit re-evaluates the entire document (worst case for closure size);
  `fan_out` points every formula at the root (whole document, no depth);
  `sparse` builds independent chains of 1,000 and edits one root, so one
  edit re-evaluates 1,000 cells, closer to an ordinary keystroke.
- Hardware for the numbers below: hosted Linux container, 4 vCPU Intel Xeon
  at 2.10 GHz, 15 GB RAM, kernel 6.18, rustc 1.94.1. This is slower and
  noisier than the declared baseline; maximum values carry scheduler
  jitter, p50 and p95 were stable across repeated runs.

## Results, hosted container

100,000 formulas, 200 edits each:

| Fixture | Cells re-evaluated per edit | p50 | p95 | max | Build (parse + graph + first eval) |
| --- | ---: | ---: | ---: | ---: | ---: |
| linear | 100,001 | 4.98 ms | 5.85 ms | 17.7 ms | 99 ms |
| fan_out | 100,001 | 5.04 ms | 5.80 ms | 7.1 ms | 85 ms |
| sparse | 1,000 | 36 µs | 47 µs | 105 µs | 85 ms |

1,000,000 formulas, 30 edits each:

| Fixture | Cells re-evaluated per edit | p50 | p95 | max | Build | Peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| linear | 1,000,001 | 102 ms | 119 ms | 235 ms | 1.17 s | 395 MB |
| fan_out | 1,000,001 | 61 ms | 69 ms | 189 ms | 1.48 s | 363 MB |
| sparse | 1,000 | 36 µs | 60 µs | 226 µs | 1.48 s | 375 MB |

Build time at one million formulas varied between 1.1 s and 2.2 s across
runs on this container; the edit percentiles did not.

Resident memory is about 370 to 400 bytes per formula cell including
dependency edges, on a fixture whose formulas each reference one cell.

## Results, frozen Enron sample (already checked in)

From `corpus/sources/enron-figshare.score-summary.json`, engine commit
`0eccaff`, 4 vCPU container, 30 s per-workbook timeout:

| Metric | Owned engine | Candidate (Formualizer + Calamine) |
| --- | ---: | ---: |
| Workbooks opened | 971 / 1000 | 834 / 1000 |
| Formula cells compiled | 662,234 of 730,879 (90.6%) | 419,854 of 419,854 loaded |
| Cached values matched, of compared | 648,639 (97.9%) | not compared |
| Wall time, both lanes together | 270 s | |
| Peak RSS, worst workbook | 1.02 GB | 160 MB |

The 29 owned-lane open failures: 24 workbooks Calamine cannot open (missing
relationship parts, shared with the candidate lane), 2 timeouts at 30 s,
2 rejected 1904 date systems, 1 over the 2,000,000-cell limit. The 1 GB peak
is one workbook whose defined names inline a large range into every formula
that uses them (29,000 formulas over one range).

## Reading against the gate

- **Incremental target, below 10 ms p95 for one edit in a 100,000-formula
  document.** The worst case, an edit that invalidates every formula, is
  5.85 ms p95 on a machine slower than the baseline. An ordinary edit
  touching a 1,000-cell closure is 47 µs p95. Passes with margin; the
  25 ms stop condition is not approached.
- **Full recalculation, one million simple formulas below 1.5 s.** Read as
  re-evaluating every formula cell from an edit, it is 69 to 119 ms p95.
  Read as parsing, building and evaluating the document from source, it is
  1.1 to 2.2 s here, which the roadmap treats as a separate parsing
  measurement rather than the recalculation gate. Both readings are stated
  so the decision does not rest on the favourable one.
- **Corpus.** The owned engine opens more of the sample than the candidate
  library and matches 97.9% of stored values where it compiles, which
  already exceeds the M2 match target; parse coverage (90.6% compiled)
  and open rate (97.1%) are below the M2 targets of 97% and 99%, which are
  not M0 gates.

## What this evidence does not show

- The declared eight-core baseline. Rerun the commands below there and
  replace the tables before deciding.
- Real-formula cost. The fixtures use `=A{n} + 1`, so per-cell time is
  graph overhead, not function evaluation. Range-heavy formulas (`SUM`
  over thousands of cells) cost proportionally more; the corpus wall time
  includes import and both lanes and is not a clean recalculation number.
- Multi-threading. The engine is single-threaded by design at M0.
- Memory under defined names. 400 bytes per cell is comfortable for the
  M1 idle target (100,000 cells), but the inlining behaviour above must be
  fixed before any M1 memory claim.
- Cold start. All numbers are warm-process.

## Recommendation

**Go**, on the recalculation thesis specifically: the owned graph meets the
incremental gate by a factor of about two on slower hardware, meets the
one-million re-evaluation gate by an order of magnitude, and opens more of
the real corpus than the candidate library. Conditions the maintainer should
attach to the decision:

1. Paste baseline-hardware numbers into this document before making a public
   v0.1.0 performance claim.
2. Land shared range nodes so defined names stop inlining ranges, before
   any M1 memory target is claimed.
3. Keep volatile functions excluded until ADR-0004's explicit-tick model is
   implemented in the event core.
4. Treat the M2 corpus targets as open work: 29 open failures, of which 24
   are an importer limitation, and a parse-coverage definition that must
   decide whether external references count as parsed.

## Reproduce

```bash
cargo build --locked --release -p omasheets-calc --bin omasheets-calc-bench
for fixture in linear fan_out sparse; do
  ./target/release/omasheets-calc-bench --formulas 100000 --iterations 200 --fixture "$fixture"
  ./target/release/omasheets-calc-bench --formulas 1000000 --iterations 30 --fixture "$fixture"
done
```

Record CPU model, core count, memory, kernel and rustc version beside the
output, and whether the process was pinned.

## Decision

- Baseline numbers: _not yet recorded; required before the v0.1.0 performance
  claim and release gate can pass_
- Decision: **Go** (maintainer)
- Date: **2026-09-04**
- Notes: The maintainer approved the native calculation-engine spike based on
  the recorded hosted evidence and recommendation. This decision authorizes
  continued native engineering; it does not relabel hosted measurements as the
  declared baseline or waive the remaining release evidence.
