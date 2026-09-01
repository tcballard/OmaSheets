# ADR-0004: Evaluate dates as Excel 1900 serials behind one explicit boundary

- Status: Proposed (owner decision required before the typed-date work in M4)
- Date: 2026-09-01

## Context

The owned M0 engine in `crates/omasheets-calc` evaluates 49 function names
over numbers, booleans, text, blanks and explicit errors. It has no date
representation, yet every real corpus workbook carries dates: `.xlsx` files
store them as IEEE-754 serial numbers with a number format, and Excel's date
functions operate on those serials directly.

[`ROADMAP.md`](ROADMAP.md) schedules typed dates and timezone-aware timestamps
for M4, after the native document, calculation and safety model is stable.
Implementing date functions before that milestone therefore forces a choice:

1. evaluate dates as raw serials with Excel's exact rules, including the
   1900 leap-year compatibility quirk inherited from Lotus 1-2-3; or
2. introduce a typed date value now and convert at import.

Option 2 is the M4 decision brought forward. It would pre-empt the event model,
storage format and provenance design that M2 and M3 still owe, and it would
create a second representation that stored-value parity comparisons must
silently bridge. The handoff rule for this slice was explicit: do not silently
mix typed dates and raw serials.

## Decision

Until M4 decides the typed representation, the owned engine treats a date as a
plain `Value::Number` holding an Excel 1900-system serial, and confines every
calendar rule to `omasheets_calc::serial_date`:

- Serial `0` is Excel's "1900-01-00". Serial `60` is the fictitious
  1900-02-29. Serials `1..=59` therefore map to the real calendar one day
  behind; serials `>= 61` are aligned with the proleptic Gregorian calendar.
  `DATE`, `YEAR`, `MONTH`, `DAY`, `EDATE`, `EOMONTH` and `WEEKDAY` reproduce
  Excel's answers on both sides of that boundary and are covered by an
  exhaustive round-trip test over every serial from `0` to `2_958_465`.
- Serials below `0` or above `2_958_465` (9999-12-31) are `#NUM!`, surfaced
  by the new `CalcError::InvalidNumber` variant rather than folded into
  `#VALUE!`.
- Date functions accept numbers and blanks only. Booleans and text are
  `#VALUE!`; no text-to-date parsing is attempted, because that parse is
  locale-sensitive and belongs to an explicit `DATEVALUE` decision.
- `TODAY` and `NOW` remain unsupported functions. They will only enter the
  engine once tick events exist, so reopening or recalculating a workbook can
  never silently change a stored value.
- The XLSX importer keeps date-formatted cells as the raw serial the file
  stores and refuses workbooks that declare the 1904 date system with an
  explicit `UnsupportedDateSystem` error. Shifting those serials by 1462 days
  would make formulas in the same workbook disagree with their cached values.
- The deterministic fixture generator can emit a `Dates` sheet whose cached
  values are computed independently in Python from the real calendar, and CI
  requires the owned engine to match every cached value on it.

## Consequences

- Date arithmetic is deterministic, clock-free and bit-identical to the
  serials Excel writes, so corpus parity can be scored without translation.
- Values do not know they are dates. Formatting, typed comparisons and unit
  checks on dates stay out of scope until M4 records the typed design.
- Every future date function must route through `serial_date`; adding a
  second conversion path is a review failure.
- Choosing a typed representation later is an additive change: serials remain
  the interchange form, and the boundary module is the single place where the
  conversion is introduced.

## Alternatives rejected

- **Reject serials below 61.** This would avoid reproducing the Lotus bug but
  would turn `DATE(1900,1,1)` and ordinary January 1900 fixtures into errors
  that Excel and LibreOffice both evaluate successfully.
- **Convert 1904 workbooks on import.** Cached formula results would then be
  compared against a shifted input model, and every date function would need
  a workbook-level epoch parameter before any evidence exists that such
  workbooks matter in the corpus.
- **Accept booleans as serials.** Excel's coercion here is inconsistent across
  functions; producing a plausible number where Excel may raise `#VALUE!`
  violates the explicit-unsupported rule.
