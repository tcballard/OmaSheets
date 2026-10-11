# Full Calc parity programme

Requested by the maintainer on 2026-10-10. This supersedes the old roadmap's
parked spreadsheet parity scope. Full parity is the target, not a present claim.
[Issue #97](https://github.com/tcballard/OmaSheets/issues/97) tracks the full acceptance programme.
The product must not link, load, launch, bundle or require LibreOfficeKit,
LibreOffice or UNO. Calc may run in isolated development/reference jobs.

## Reference and implementation

LibreOfficeKit is an embedding boundary. Calc behaviour lives in the LibreOffice
core, including `sc` (model, interpreter, import/export, view), `formula`,
`oox`, `xmloff`, `svl` (formats), drawing/chart modules and scripting services.
An available source tree is useful specification evidence, not proof that the
owned implementation reproduces that behaviour. OmaSheets keeps its Rust
engine and Qt view. Source reuse would require a separately recorded dependency
and licence decision; this increment copies no LibreOffice implementation.

Initial source inspection: LibreOffice/core commit
`1746b16a564f59fcaf8c5670bb292748408da54e`,
[`sc/source/core/tool/interpr1.cxx`](https://github.com/LibreOffice/core/blob/1746b16a564f59fcaf8c5670bb292748408da54e/sc/source/core/tool/interpr1.cxx).
`ScIsErr`, `ScIsNonString` and `ScErrorType_ODF` inform the first increment.
Calc's legacy `ERRORTYPE` exposes internal error numbers; `ERROR.TYPE` uses
standard 1–7 codes. These are different functions and must not be conflated.

## Acceptance dimensions

Each row requires its own fixture inventory and measured result. An overall
function count, workbook open rate or passing CI is not full parity evidence.

| Dimension | Required behaviour | Current known gap |
| --- | --- | --- |
| Formula language | Calc/Excel/ODF dialects, references, names, arrays, dynamic dependencies, errors | Bounded Excel syntax; no 3D refs or spilling; limited dynamic refs |
| Function semantics | Every supported Calc built-in, argument coercion, domains, locale, precision | 180 parser names, including aliases; comprehensive Calc semantics not yet measured |
| Recalculation | Dirty graph, cycles/iteration, volatility, calculation settings | Iteration and Calc clock/random policies unresolved |
| Document model | All sheet/cell properties, annotations, protection, merges, hidden content | Strict admission refuses many source features |
| Data workflows | Sort/filter, tables, validation, conditional formatting, pivots, links and refresh | Incomplete owned workflows and source admission |
| Formats | ODS, XLSX, XLS, XLSM, CSV/text; preservation and export | Native and strict XLSX subset only |
| Presentation | Locale-aware formats, charts/drawings, themes, rich text and layout | Incomplete renderer and formatting projection |
| Print/export | Page styles, breaks, scaling, headers/footers, faithful PDF | Bounded cell preview only |
| Interaction | Calc keyboard/mouse workflows, clipboard, undo/redo, accessibility | Physical Wayland, assistive technology and broad interaction acceptance pending |
| Automation | Spreadsheet macros, scripting, extensions and automation interfaces | Not implemented; runtime/API compatibility needs an explicit design |
| Scale/recovery | Large/dense/sparse files, cancellation, save/recovery and performance | Existing bounded tests do not establish full Calc-scale parity |

## Execution and promotion

1. Freeze the reference version, settings, source commit and fixture bytes for
   each investigation. Keep external reference code/test licences and provenance.
2. Add live reference scenarios and record values, errors, formulas, document
   properties, rendering and interaction evidence as applicable. Never compare
   only stale cached values from an arbitrary downloaded workbook.
3. Implement in the owned engine/model/view; add boundary cases that distinguish
   the actual semantics. A function is not complete merely because it parses.
4. Compare edit/recalculate/save/reopen results, including unsupported or lost
   features. Record refusal, mismatch and untested as different outcomes.
5. Promote through the normal owned product and compiler-free installation.
   Product dependency audits must continue to pass without LibreOffice present.
6. Require independent review and real desktop acceptance before a full parity
   release claim. No percent or date is assigned without an enumerated denominator.

The first executable reference slice is `scripts/check_calc_parity.mjs` and
`tests/calc-parity/information.json`. It creates fresh FODS formula inputs,
runs isolated headless Calc to XLSX, then invokes the existing Rust XLSX scorer.
All observed/loaded/compared/matching counts must equal the fixture count;
refused formulas, missing results and mismatches fail the job. Evidence includes
the exact Calc version, fixture/source hashes, expressions and typed comparison
summary. Conversion and scoring have time/output bounds. The input uses only
trusted repository fixtures, never an arbitrary user's macro-bearing workbook.

This initial slice covers scalar information/error functions and a few basic
numerical regressions. It does not establish ODS product support, Calc dialect
support, range/array function parity, round-trip preservation or full parity.
Reference infrastructure is deliberately separate from the product build.

Next slices: complete built-in inventory with versioned dialect/name mapping;
coercion/reference/array semantics; date/locale and number formats; named ranges,
structured source features and preservation; ODS model/import/export; charts,
pivots and printing; interactions/accessibility; automation and scale. Every
unimplemented dimension remains a release blocker for full parity.

## First measured blockers

The first live reference run used LibreOffice 24.2.7.2 (420 Build 2), not the
source inventory's newer commit. Its exact version is recorded in evidence.
It observed 29 formulas: 27 compiled, 25 matched, two refused `TRUE()` and two
miscalculated. `TRUE()`/`FALSE()` support is now added, along with lazy `IFNA`.
The reference suite has expanded to 34 expressions and is still required to
report every one.

The initial error-inspection differences were `ERROR.TYPE(SQRT(-1))` and
`ERROR.TYPE(VALUE("bad"))`: Calc returned `#N/A`; the owned engine produced
6 and 3 respectively. The engine now retains Calc's internal illegal-argument
origin instead of flattening it to a standard error. `ERROR.TYPE` returns
`#N/A` for that origin; literal/imported standard errors still return codes 1–7.
The original reference workbook now recalculates with all 653 results matching.
Fresh expanded reference evidence is required before closing [issue #98](https://github.com/tcballard/OmaSheets/issues/98).

`ScSqrt`, `ScValue`, and `ScErrorType_ODF` in the pinned `interpr1.cxx`, plus
`XclTools::GetXclErrorCode` in
[`sc/source/filter/excel/xltools.cxx`](https://github.com/LibreOffice/core/blob/1746b16a564f59fcaf8c5670bb292748408da54e/sc/source/filter/excel/xltools.cxx),
provide the source contract: illegal argument (502) exports as `#VALUE!` but
is not standard `#VALUE!` internally. XLSX caches only contain the projected
error label, so the importer compares that label while retaining origin during
recalculation. Dependent inspection formulas are compared independently.
Supported formulas with error caches can now pass native admission: formula
replay regenerates their values and origins. Literal error cells, unsupported
formulas and empty formula records remain refused. Integration tests cover
source import, edits, native replay, XLSX export caches and reimport.
Other functions' internal error origins remain to be audited; this is not a
claim that every Calc domain error now has identical provenance.

A reproducible source catalogue is retained in
`tests/calc-parity/calc-catalogue.json`, generated with
`scripts/inventory_calc_parity.mjs` from the pinned upstream header. All five
English maps are recorded. The Calc map has 432 named tokens; 170 names match
the owned parser and 262 do not. These are token-name counts, include aliases
and special names, and are **not a function-completion percentage**. Matching
a name does not establish coercion, reference, error, array or locale semantics.

## Common calculation target — 2026-10-10

The maintainer requested an iterative implementation until a reasonable
common-calculation target was reached. The original gate required at least
400 scenarios, 99% fresh Calc agreement, zero unsupported formulas and only
the two documented internal-error differences. That target reached 651/653.
The next increment removes the exception list: **every case must match**, with
zero refusals, in both common and strict information modes. The case count
still has a minimum of 400. No failing case was removed.
Numeric comparisons use the existing XLSX scorer's tolerance:
`|owned-reference| <= 1e-9 * max(|reference|, 1)`. Typed strings, booleans and
errors must match. This is not a bit-for-bit floating-point equality claim.

`tests/calc-parity/common.json` contains 796 deterministic scenarios
in information, mathematics, rounding, statistics, text, dates, finance,
references/arrays, error origins and aggregate coercion. `scripts/generate_common_calc_cases.mjs` reproduces them.
The denominator is this declared suite, not the percentage of real workbooks
or of all Calc features. The owned registry expands to 180 names.

The new common module reuses the engine's graph, reference binding, errors and
array evaluator. FODS test generation maps function names through the pinned
ODF/OOXML catalogue and converts delimiters without rewriting quoted strings.
The test reference is isolated from product bundles and dependencies.

The common job pins the official Linux LibreOffice 26.8.1.1 reference archive
with SHA-256 `30903df3b9f61360d9660cd707de48cd2831469114492a5008ed58a0ac77d044`.
It enables wildcards and disables regular expressions in the generated FODS,
with case-insensitive searches. `CALC_REFERENCE` selects this isolated binary;
the exact binary version and settings are retained in the evidence. The first
594-case pass used Ubuntu's older 24.2 reference and matched 579 cases, with
14 differences and one refused modern `XLOOKUP` name. Those cases remain in
the suite. A fractional `DAYS` regression adds the 595th case. The second pass
with the pinned modern reference matched 592 of 595, with no refused formulas;
one fractional-minute difference remained alongside the two known gaps. That
boundary and additional hour-end cases are included in the next expansion.

The 645-case expansion also checks text and escaped-wildcard criteria, mixed
input ranges, database criteria, statistic aliases, dated cash flows, nonzero
financial future values, lazy `CHOOSE`, `SUBTOTAL`, positions and bounded
reference functions. The corpus reaches all 177 nonvolatile registered names;
the three explicit-tick clock/random functions need a separate reference-clock
contract and are not part of this suite.
That pass matched 642 of 645 with no refusals. One existing `SUBTOTAL` array
flattening bug remained, alongside the two known inspection gaps. The next
eight cases cover array subtotals and live dependent formulas that distinguish
excluding nested subtotal cells from including ordinary formula cells.
That 653-case pass matched 650 with no refusals; its remaining unexpected
difference was direct single-cell subtotal inclusion. Calc excludes nested
subtotal cells in ranges, but includes a directly referenced subtotal cell.
The owned engine now makes that distinction, and the original case remains.

Calc-specific boundaries in this increment include `ATAN2(0,0) = 0`, flooring
fractional `SMALL` ranks (while `LARGE` rounds up), preserving fractional
`TIME` components and `DAYS` differences, rejecting an empty `SEARCH` pattern,
and rejecting `COMBINA` when the selection exceeds the first argument. These
must not be described as complete Excel dialect compatibility. Existing
`SIGN(0)` and negative-divisor `MOD` results are also corrected.
`SECOND` rounds fractional seconds without carrying into `MINUTE` or `HOUR`.

An owned workbook session regression imports new common formulas, changes an
input, checks dependent recalculation, saves with all nine formulas preserved,
and reopens both the native document and exported XLSX. Independent ZIP checks
verify selected formula caches. This exercises the product path as well as
the differential calculation scorer.

## Error origin and aggregate follow-up — 2026-10-11

The first fresh 755-case pass confirmed all 653 previous cases and all 66
new error-origin cases. The 36 aggregate probes exposed 15 numeric/type
mismatches and six malformed fixtures: `TRUE()` was incorrectly used inside
an array constant. Those six inputs now use the valid `TRUE` literal; no
formula-call syntax was added to the array-constant parser.

The aggregate implementation now distinguishes direct arguments from
reference/array members. Direct booleans count numerically; direct text causes
`#VALUE!` in SUM/AVERAGE/MIN/MAX/PRODUCT under the measured Calc settings.
COUNT accepts numeric scalar text and ignores errors; COUNTA counts errors.
MIN/MAX retain the internal parameter-error origin, which differs from the
standard error used by SUM/AVERAGE/PRODUCT. SUBTOTAL preserves collection
semantics after filtering nested subtotals. Seventeen more probes cover error
counting and inspection, bringing the next reference run to 772 cases.

The imported FODS boolean input is serialized by Calc as numeric 1 in XLSX;
its range cases must not be cited as proof of native boolean-reference parity.
Locale-sensitive text conversion and other aggregate/function coercion rules
remain separate acceptance work.

The fresh 772-case run matched 766, with no refused formulas. It isolated six
array differences: Calc counts boolean array members numerically, and array
errors propagate through COUNT/COUNTA even though scalar and reference errors
are ignored/counted. The implementation now preserves that distinction.
Twelve more dependent SUBTOTAL count cases guard against turning reference
members into array arguments during nested-subtotal filtering. The resulting
786-case suite retains every previous valid case and adds two conditional-array
probes for the omitted FALSE branch in AVERAGE/COUNT.

The 786-case reference run matched 784, with no refusals. Only the two new
conditional-array probes differed: an omitted IF branch is an internal empty
path, distinct from an explicit FALSE array member. Array intermediates now
retain that marker; aggregates exclude it while ordinary evaluation still
observes FALSE. TRANSPOSE and INDEX preserve the marker, and arithmetic
consumes the value. Ten regression probes cover explicit versus omitted
branches, inspection, nesting, transpose, selection and arithmetic, yielding
796 reference cases. The source contract is `ScMatrixImpl::IsEmptyPath`,
`CountElements` and `ScMatrixImpl::IfJump` in pinned `scmatrix.cxx`.
