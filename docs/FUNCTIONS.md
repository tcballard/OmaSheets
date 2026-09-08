# Supported formula functions

The owned M0 engine (`crates/omasheets-calc`) accepts exactly the
110 function names listed below, grouped for reading.
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

Deliberately unsupported: `TODAY`, `NOW`, `RAND` and every other volatile
function (until the calculation context consumes stored tick events), external workbook references,
3D references, spilling array formulas, `INDIRECT`, `OFFSET`,
`CELL`, add-in (`_xll.`) calls, locale-sensitive parsing such as `DATEVALUE`
and `TEXT`, and the 1904 date system.

Approximate lookups (`VLOOKUP`/`HLOOKUP` without `FALSE`, `MATCH` types 1 and
-1) binary-search sorted keys per Excel's documented contract; results over
unsorted keys are undefined in Excel and are not promised here.

## Registry

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


`TEXTJOIN` joins scalar and bounded range arguments in row order, can skip blanks
and empty strings, propagates errors, and refuses output beyond 32,767 UTF-16 units.
