# Supported formula functions

The owned M0 engine (`crates/omasheets-calc`) accepts exactly the
83 function names listed below, grouped for reading.
A test in the calc crate fails when this file and the registry disagree, so
the count here is never edited by hand: add the function to the registry and
regenerate this list.

Operators: `+ - * / ^ & %`, unary `+`/`-`, comparisons `= <> < <= > >=`,
error literals (`#REF!`, `#N/A`, `#DIV/0!`, `#VALUE!`, `#NUM!`, `#NAME?`,
`#NULL!`), omitted arguments, bounded rectangular ranges, absolute markers,
cross-sheet references, workbook defined names and implicit intersection of a
range in scalar position.

Deliberately unsupported: `TODAY`, `NOW`, `RAND` and every other volatile
function (until explicit tick events exist), external workbook references,
3D references, array constants and array formulas, `INDIRECT`, `OFFSET`,
`CELL`, add-in (`_xll.`) calls, locale-sensitive parsing such as `DATEVALUE`
and `TEXT`, and the 1904 date system.

Approximate lookups (`VLOOKUP`/`HLOOKUP` without `FALSE`, `MATCH` types 1 and
-1) binary-search sorted keys per Excel's documented contract; results over
unsorted keys are undefined in Excel and are not promised here.

## Registry

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

### Dates (1900 serial system)

- `DATE`
- `YEAR`
- `MONTH`
- `DAY`
- `EDATE`
- `EOMONTH`
- `WEEKDAY`

