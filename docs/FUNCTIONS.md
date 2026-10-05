# Supported formula functions

The owned M0 engine (`crates/omasheets-calc`) accepts exactly the
119 function names listed below, grouped for reading.
A test in the calc crate fails when this file and the registry disagree, so
the count here is never edited by hand: add the function to the registry and
regenerate this list.

Operators: `+ - * / ^ & %`, unary `+`/`-`, comparisons `= <> < <= > >=`,
error literals (`#REF!`, `#N/A`, `#DIV/0!`, `#VALUE!`, `#NUM!`, `#NAME?`,
`#NULL!`, and `Sheet!#REF!` for a deleted cell on another sheet), omitted arguments,
bounded rectangular ranges (including qualified endpoints and deleted endpoints
such as `A1:#REF!`, which evaluate to `#REF!`), absolute markers,
cross-sheet references, workbook and sheet-scoped defined names (including
`Sheet!LocalName`; tokens past
the grid such as `Table1` are names), implicit intersection of a range in
scalar position, and elementwise evaluation of range expressions inside
aggregate arguments (`SUM(IF(A1:A5=0,0,B1:B5))`, `SUMPRODUCT((A1:A5>2)*B1:B5)`).
Rectangular array constants support numbers, text, booleans and error literals,
comma-separated columns and semicolon-separated rows, up to 1,000,000 values.
They work in aggregates, elementwise expressions and INDEX/MATCH/LOOKUP,
VLOOKUP/HLOOKUP/XLOOKUP. A scalar use takes the first value; spilling into
neighbouring cells is not implemented. `_xlfn.` and `_xlfn._xlws.` prefixes
resolve only to functions already in the registry.

`INDEX` also returns references: `SUM(A1:INDEX(A1:A100,D1))` follows the
selector in D1, and a zero row or column selects that entire axis. The range
operator binds a bounded envelope of all possible endpoint selections. Native
replay preserves that envelope's row and column identities, including after
sorting. Constant row/column selections narrow the calculation dependencies after
stable binding, so unused source columns do not create false cycles. Dynamic axes
retain their bounded envelope; potential cycles within it are still refused. A moved formula
whose current A1 spelling cannot preserve those identities reports a projection
refusal instead of exporting different references.

An external workbook reference (`[1]Sheet!A1`, `[Book.xlsx]Sheet!A1`, or
`'[Book.xlsx]Sheet 1'!A1`) compiles. Import opens a linked file when it is a
relative path under the source workbook's directory, or when an absolute or
`file://` target names a file sitting next to the source. Network targets,
`..`, and a symlink that escapes that directory are not opened. A workbook
already being imported keeps the stored link cache, and that cache is also
used when the file is not opened. Cached external strings keep their decoded
whitespace, including empty values. Occupied cells, cached link records and
opened targets share the importer's cell budget. A single cell with no value
in the opened file or the cache is `#REF!`. A missing cell inside an external
range is blank.
Deliberately unsupported: clock/random evaluation without an explicit tick,
3D references, spilling array formulas, dynamic `INDIRECT`/`OFFSET` arguments,
`CELL`, add-in (`_xll.`) calls, locale-sensitive parsing such as `DATEVALUE`,
and the 1904 date system. `TEXT` accepts only the locale-free codes listed
with the text functions below.

Approximate lookups (`VLOOKUP`/`HLOOKUP` without `FALSE`, `MATCH` types 1 and
-1) binary-search sorted keys per Excel's documented contract; results over
unsorted keys are undefined in Excel and are not promised here.

## Registry

### Explicit tick and bounded references

- `TODAY`
- `NOW`
- `RAND`
- `OFFSET`
- `INDIRECT`

`TODAY`, `NOW` and `RAND` require a persisted `Tick` event before a formula
can be installed. Use Commands → Data → Refresh date and random formulas
to create or update that tick. Tick timestamps are UTC Unix milliseconds. Recalculation and
reopen reuse that tick; a new explicit tick updates the values and dependents.
RAND uses a fixed deterministic mixing algorithm with the tick, stable native
cell identity and call order. It is not cryptographic randomness.

`OFFSET` accepts bounded reference arguments with literal numeric offsets and
sizes. `INDIRECT` accepts literal A1 text within this workbook. They compile to
normal tracked references and retain native stable-ID behavior after edits.
Dynamic text/offset expressions and R1C1 mode are explicitly refused pending a
bounded dynamic-dependency design.

### Matrices and databases

- `TRANSPOSE`
- `MMULT`
- `DAVERAGE`
- `DMAX`
- `DMIN`
- `DSTDEV`

`TRANSPOSE` and `MMULT` produce bounded arrays consumed by aggregates and
lookups; scalar uses take the first element. MMULT requires numeric, nonblank
inputs with matching inner dimensions, at most 1,000,000 output values and
50,000,000 multiply-add terms. Larger products return `#NUM!`.

Database aggregates resolve fields by heading or one-based column number.
Criteria columns on one row are ANDed; rows are ORed. Duplicate headings,
blank criteria, text prefixes, wildcards and comparison operators are supported.
At most 50,000,000 candidate/criterion comparisons are allowed per call.
Formula criteria with nonmatching/blank headings are not implemented and return
`#VALUE!`; they require relative formula evaluation for each database record.

### Aggregates and statistics

- `SUM`
- `AVERAGE`
- `MIN`
- `MAX`
- `COUNT`
- `COUNTA`
- `PRODUCT`
- `SUMPRODUCT`
- `MEDIAN`
- `RANK`
- `SUBTOTAL`
- `STDEV`
- `STDEV.S`
- `STDEVP`
- `STDEV.P`
- `VAR`
- `VAR.S`
- `VARP`
- `VAR.P`
- `AVERAGEA`
- `CORREL`
- `NORMDIST`
- `NORM.DIST`
- `NORMSDIST`
- `NORM.S.DIST`
- `COVAR`
- `COVARIANCE.P`
- `COVARIANCE.S`

### Conditional aggregates

- `COUNTIF`
- `SUMIF`
- `COUNTIFS`
- `SUMIFS`
- `AVERAGEIF`
- `AVERAGEIFS`

### Logical and errors

- `IF`
- `AND`
- `OR`
- `NOT`
- `IFERROR`
- `ISBLANK`
- `ISNUMBER`
- `ISTEXT`
- `ISLOGICAL`
- `ISERROR`
- `N`
- `T`
- `CHOOSE`
- `NA`
- `ISNA`

### Math

- `ABS`
- `ROUND`
- `ROUNDUP`
- `ROUNDDOWN`
- `INT`
- `MOD`
- `POWER`
- `SQRT`
- `SIGN`
- `CEILING`
- `FLOOR`
- `TRUNC`
- `EXP`
- `LN`
- `LOG`
- `LOG10`
- `PI`

### Text

- `LEN`
- `LEFT`
- `RIGHT`
- `MID`
- `TRIM`
- `UPPER`
- `LOWER`
- `CONCAT`
- `CONCATENATE`
- `TEXTJOIN`
- `VALUE`
- `TEXT`
- `HYPERLINK`
- `EXACT`
- `FIND`
- `REPT`

### Lookup and position

- `INDEX`
- `MATCH`
- `VLOOKUP`
- `XLOOKUP`
- `HLOOKUP`
- `ROW`
- `COLUMN`
- `LOOKUP`

### Dates (1900 serial system)

- `DATE`
- `YEAR`
- `MONTH`
- `DAY`
- `EDATE`
- `EOMONTH`
- `WEEKDAY`
- `YEARFRAC`
- `DAYS360`
- `NETWORKDAYS`
- `WORKDAY`

### Financial

- `PMT`
- `PV`
- `IRR`
- `NPV`
- `XNPV`
- `XIRR`
- `RRI`


`TEXTJOIN` joins scalar and bounded range arguments in row order, can skip blanks
and empty strings, propagates errors, and refuses output beyond 32,767 UTF-16 units.

`TEXT` formats a number with one code, compared without regard to case:
`General`, `0`, `0.00`, `#`, `#,##0`, `#,##0.00`, `0%`, `0.00%`, `yyyy-mm-dd`,
or `mm/dd/yyyy`. Any other code, including surrounding whitespace or a literal
suffix such as `0.0x`, is `#VALUE!`. `#` rounds half away from zero to an
integer and shows nothing for zero. Date codes use the 1900 serial, including
the fictitious 1900-02-29.
`HYPERLINK` returns its friendly name, or the link when the name is omitted,
and does not fetch the target. `RANK` is a competition rank over numbers in
the reference (ties share a rank and the next rank is skipped); a zero or
omitted order ranks the largest first. An absent target returns `#N/A`.
`RRI` is `(fv/pv)^(1/nper)-1`; a zero future value returns -1 (total loss).

Native `.omasheets` documents do not yet persist external workbook inputs. Native import therefore retains the source cached value and reports the formula as cached-only; it does not install an external formula that would evaluate to `#REF!` or an empty-range zero. The owned XLSX scorer can resolve linked inputs separately.

Owned XLSX import refuses external scalar formulas without a resolved cell input, and external ranges without any resolved input for their sheet. It retains their source cached values and counts them as unsupported; dependent formulas can still use those caches. Missing cells within an otherwise resolved external range remain blank. This prevents absent workbooks from manufacturing zero or error values and is reflected in the raw coverage denominator.
