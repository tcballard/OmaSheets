# Full Calc parity programme

Requested by the maintainer on 2026-10-10. This supersedes the old roadmap's
parked spreadsheet parity scope. Full parity is the target, not a present claim.
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
| Function semantics | Every supported Calc built-in, argument coercion, domains, locale, precision | 122 parser names, including aliases; comprehensive Calc inventory not yet measured |
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
