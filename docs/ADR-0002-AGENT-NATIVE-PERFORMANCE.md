# ADR-0002: Make LibreOffice the compatibility sidecar, not the permanent hot path

- Status: Proposed
- Date: 2026-08-31

## Context

OmaSheets aims to be the spreadsheet built for agents: responsive for an
interactive human, predictable under workbook-wide requests, and bounded when
an agent asks for more work than the machine should accept. Merely repackaging
Calc cannot deliver that outcome. The initial v0.0.1 path deliberately buys
format, calculation, chart, pivot and rendering compatibility from
LibreOffice, but a live workbook window and every isolated agent job each pay
for a substantial general-purpose office engine.

The previous "large" smoke workbook established only a large used coordinate
range. It contained almost no data and measured the UI process rather than the
complete process tree. It is not evidence for dense analysis, formula
recalculation or concurrent agent work.

## Direction

LibreOffice remains the cold compatibility and publication authority while
OmaSheets develops a smaller resident workbook kernel. Engine replacement is a
measured competition behind the existing agent, plan, verification and receipt
contracts; it is not a flag-day rewrite.

The product will separate three workload lanes:

1. **Interactive lane.** Keep scrolling, selection and ordinary edits
   responsive. Today this is LibreOfficeKit. A future native virtualised grid
   must not wait on workbook-wide agent work.
2. **Query lane.** Serve bounded reads, search, profiles and aggregations with
   one immutable workbook generation per request. Batch related reads, scan
   once where possible, retain bounded statistics, and cancel at explicit
   resource limits. A read-only Calamine/Formualizer prototype is being
   evaluated here; a columnar analytical coprocessor may be added for table
   regions rather than for cell semantics.
3. **Compatibility lane.** Use isolated LibreOffice jobs for formats or
   features the native kernel cannot prove, and for the final recalculate,
   save, reopen, object-fingerprint and render checks required before local
   approval.

An engine result is never trusted merely because it is faster. The compatibility
lane remains authoritative until the candidate matches the corpus for the
specific feature it claims.

## Resource contract

Every large operation needs all of the following before its limit is raised:

- a truthful dense, sparse and formula fixture with exact populated-cell
  counts and immutable hashes;
- wall time plus complete process-group RSS, PSS, USS and process count;
- a hard cell/formula/result bound checked before materialisation;
- an execution timeout and a tested cancellation path;
- no unbounded command output, retained samples, row copies or result JSON;
- cold and warm measurements kept separate from UI first-paint measurements;
- output hashes and LibreOffice save/reopen comparison where bytes are written.

When a metric is unavailable it is reported as unavailable. RSS is not relabelled
as PSS, a sparse coordinate range is not called dense, and a process-local
measurement is not called application memory.

## Candidate promotion gates

A native query or formula candidate can enter the installed product only behind
an explicit capability gate and LibreOffice fallback. Promotion requires:

1. Linux builds produced by the release pipeline without requiring compilers
   on the user's machine.
2. Formula and value parity on the compatibility corpus, including errors,
   dates, locale-sensitive inputs, names, cross-sheet references and cycles.
3. Bounded memory and cancellation evidence on the standard corpus.
4. No regression to macro isolation, filesystem confinement, immutable
   evidence, plan sealing or local-only publication.
5. A persisted engine identity in verification evidence so mixed-engine
   results remain auditable.

Charts, pivots, drawing objects, macros and final workbook publication remain
on the compatibility lane until independently proven. An analytical database
may accelerate grouped table questions, but it does not become the workbook's
formula or file-format authority.

## First implementation slices

The initial performance branch deliberately starts with changes that do not
alter workbook semantics:

- stream semantic snapshot hashing while discarding completed XML nodes;
- reject over-budget workbooks before UNO materialises cell matrices;
- derive workbook-audit statistics from one value/formula materialisation;
- avoid loading a value matrix for formula-only inspection;
- batch independent read queries into one snapshot and Calc worker load;
- replace idle diff-overlay polling with filesystem notifications; and
- generate truthful fixtures and process-tree memory evidence in CI.

The isolated Rust spike is evidence gathering, not a shipped engine. Its first
formula import failure is a useful gate working as intended.

## Consequences

- LibreOffice package size is accepted for the first release, but its cost is
  no longer assumed to be the permanent cost of an open OmaSheets workbook.
- Improvements can ship incrementally without weakening the human review loop.
- "Fast" and "lightweight" remain measured product properties, not branding
  claims.
- A native grid is still substantial work. This decision makes its boundary
  explicit instead of hiding it inside a LibreOffice fork.
