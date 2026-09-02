//! Bounded `.xlsx` import into the owned OmaSheets M0 calculation engine.
//!
//! Date-formatted cells are imported as the raw serial numbers the file stores,
//! matching `omasheets_calc::serial_date`; workbooks that declare the 1904 date
//! system are rejected rather than silently offset by 1462 days.

use calamine::{Data, Range, Reader, Xlsx, open_workbook};
use omasheets_calc::serial_date::DATE_SYSTEM;
use omasheets_calc::{CalcError, CellId, FormulaError, Value, Workbook};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImportLimits {
    pub max_sheets: usize,
    pub max_cells: usize,
    pub max_formulas: usize,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_sheets: 256,
            max_cells: 2_000_000,
            max_formulas: 1_000_000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SheetInfo {
    pub index: u32,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsupportedFormula {
    pub cell: CellId,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParitySummary {
    pub formula_cells_observed: usize,
    pub formula_cells_loaded: usize,
    pub formula_cells_compared: usize,
    pub stored_values_matched: usize,
    pub stored_values_mismatched: usize,
    pub unsupported_formulas: usize,
}

pub struct ImportedWorkbook {
    pub workbook: Workbook,
    pub sheets: Vec<SheetInfo>,
    pub source_sha256: String,
    /// Always `"1900"`: the importer refuses every other date system.
    pub date_system: &'static str,
    pub unsupported: Vec<UnsupportedFormula>,
    stored_formula_values: Vec<(CellId, Value)>,
    formula_cells_observed: usize,
    formula_cells_loaded: usize,
}

impl ImportedWorkbook {
    pub fn parity(&self) -> ParitySummary {
        let stored_values_matched = self
            .stored_formula_values
            .iter()
            .filter(|(cell, stored)| values_match(stored, &self.workbook.value(*cell)))
            .count();
        let formula_cells_compared = self.stored_formula_values.len();
        ParitySummary {
            formula_cells_observed: self.formula_cells_observed,
            formula_cells_loaded: self.formula_cells_loaded,
            formula_cells_compared,
            stored_values_matched,
            stored_values_mismatched: formula_cells_compared - stored_values_matched,
            unsupported_formulas: self.unsupported.len(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportError {
    Open(String),
    Read(String),
    TooManySheets { observed: usize, maximum: usize },
    TooManyCells { observed: usize, maximum: usize },
    TooManyFormulas { observed: usize, maximum: usize },
    UnsupportedDateSystem { observed: &'static str },
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(error) => write!(formatter, "could not open xlsx: {error}"),
            Self::Read(error) => write!(formatter, "could not read xlsx: {error}"),
            Self::TooManySheets { observed, maximum } => {
                write!(
                    formatter,
                    "workbook has {observed} sheets; limit is {maximum}"
                )
            }
            Self::TooManyCells { observed, maximum } => {
                write!(
                    formatter,
                    "workbook spans {observed} cells; limit is {maximum}"
                )
            }
            Self::TooManyFormulas { observed, maximum } => {
                write!(
                    formatter,
                    "workbook has {observed} formulas; limit is {maximum}"
                )
            }
            Self::UnsupportedDateSystem { observed } => {
                write!(
                    formatter,
                    "workbook uses the {observed} date system; only the {DATE_SYSTEM} date system is supported"
                )
            }
        }
    }
}

impl std::error::Error for ImportError {}

pub fn import_xlsx(path: &Path, limits: ImportLimits) -> Result<ImportedWorkbook, ImportError> {
    let source_sha256 = hash_file(path)?;
    let mut source: Xlsx<_> = open_workbook(path)
        .map_err(|error: calamine::XlsxError| ImportError::Open(error.to_string()))?;
    check_date_system(source.has_1904_epoch())?;
    let sheet_names = source.sheet_names();
    if sheet_names.len() > limits.max_sheets {
        return Err(ImportError::TooManySheets {
            observed: sheet_names.len(),
            maximum: limits.max_sheets,
        });
    }

    let mut ranges = Vec::with_capacity(sheet_names.len());
    for name in &sheet_names {
        let values = source
            .worksheet_range(name)
            .map_err(|error| ImportError::Read(error.to_string()))?;
        let formulas = source
            .worksheet_formula(name)
            .map_err(|error| ImportError::Read(error.to_string()))?;
        ranges.push((name.clone(), values, formulas));
    }
    import_ranges(ranges, source_sha256, limits)
}

fn check_date_system(has_1904_epoch: bool) -> Result<(), ImportError> {
    if has_1904_epoch {
        Err(ImportError::UnsupportedDateSystem { observed: "1904" })
    } else {
        Ok(())
    }
}

fn hash_file(path: &Path) -> Result<String, ImportError> {
    let mut source = File::open(path).map_err(|error| ImportError::Open(error.to_string()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| ImportError::Read(error.to_string()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn import_ranges(
    ranges: Vec<(String, Range<Data>, Range<String>)>,
    source_sha256: String,
    limits: ImportLimits,
) -> Result<ImportedWorkbook, ImportError> {
    if ranges.len() > limits.max_sheets {
        return Err(ImportError::TooManySheets {
            observed: ranges.len(),
            maximum: limits.max_sheets,
        });
    }
    let mut observed_cells = 0_usize;
    let mut observed_formulas = 0_usize;
    for (_, values, formulas) in &ranges {
        observed_cells =
            observed_cells.saturating_add(values.width().saturating_mul(values.height()));
        observed_formulas = observed_formulas.saturating_add(
            formulas
                .used_cells()
                .filter(|(_, _, formula)| !formula.is_empty())
                .count(),
        );
    }
    if observed_cells > limits.max_cells {
        return Err(ImportError::TooManyCells {
            observed: observed_cells,
            maximum: limits.max_cells,
        });
    }
    if observed_formulas > limits.max_formulas {
        return Err(ImportError::TooManyFormulas {
            observed: observed_formulas,
            maximum: limits.max_formulas,
        });
    }

    let sheets: Vec<SheetInfo> = ranges
        .iter()
        .enumerate()
        .map(|(index, (name, _, _))| SheetInfo {
            index: index as u32,
            name: name.clone(),
        })
        .collect();
    let mut workbook = Workbook::default();
    for sheet in &sheets {
        workbook.define_sheet(sheet.index, sheet.name.clone());
    }
    let mut formula_records = Vec::with_capacity(observed_formulas);

    for (sheet, (_, values, formulas)) in ranges.iter().enumerate() {
        let (value_row, value_column) = values.start().unwrap_or((0, 0));
        for (row, column, value) in values.used_cells() {
            let cell = CellId::new(
                sheet as u32,
                value_row + row as u32,
                value_column + column as u32,
            );
            set_source_value(&mut workbook, cell, value);
        }
        let (formula_row, formula_column) = formulas.start().unwrap_or((0, 0));
        for (row, column, formula) in formulas.used_cells() {
            if formula.is_empty() {
                continue;
            }
            let absolute = (formula_row + row as u32, formula_column + column as u32);
            let cell = CellId::new(sheet as u32, absolute.0, absolute.1);
            let stored = values
                .get_value(absolute)
                .map(source_value)
                .unwrap_or(Value::Blank);
            formula_records.push((cell, stored, formula.clone()));
        }
    }

    let mut unsupported = Vec::new();
    let mut stored_formula_values = Vec::new();
    for (cell, stored, formula) in formula_records {
        match workbook.set_formula(cell, &formula) {
            Ok(_) => stored_formula_values.push((cell, stored)),
            Err(error) => unsupported.push(UnsupportedFormula {
                cell,
                reason: bounded_formula_error(error),
            }),
        }
    }
    let formula_cells_loaded = stored_formula_values.len();
    Ok(ImportedWorkbook {
        workbook,
        sheets,
        source_sha256,
        date_system: DATE_SYSTEM,
        unsupported,
        stored_formula_values,
        formula_cells_observed: observed_formulas,
        formula_cells_loaded,
    })
}

fn set_source_value(workbook: &mut Workbook, cell: CellId, value: &Data) {
    match value {
        Data::Int(value) => {
            workbook.set_number(cell, *value as f64);
        }
        Data::Float(value) => {
            workbook.set_number(cell, *value);
        }
        Data::Bool(value) => {
            workbook.set_boolean(cell, *value);
        }
        Data::String(value) | Data::DateTimeIso(value) | Data::DurationIso(value) => {
            workbook.set_text(cell, value.clone());
        }
        Data::DateTime(value) => {
            // The raw 1900-system serial; `check_date_system` has already
            // rejected 1904 workbooks, so no epoch shift is applied.
            workbook.set_number(cell, value.as_f64());
        }
        Data::Error(_) => {
            // The first M0 value model does not yet distinguish Excel error codes.
            workbook.set_text(cell, "#SOURCE_ERROR");
        }
        Data::Empty => {
            workbook.clear(cell);
        }
    }
}

fn source_value(value: &Data) -> Value {
    match value {
        Data::Int(value) => Value::Number(*value as f64),
        Data::Float(value) => Value::Number(*value),
        Data::Bool(value) => Value::Boolean(*value),
        Data::String(value) | Data::DateTimeIso(value) | Data::DurationIso(value) => {
            Value::Text(value.clone())
        }
        Data::DateTime(value) => Value::Number(value.as_f64()),
        Data::Error(_) => Value::Error(CalcError::InvalidValue),
        Data::Empty => Value::Blank,
    }
}

fn bounded_formula_error(error: FormulaError) -> String {
    error.to_string().chars().take(256).collect()
}

fn values_match(stored: &Value, calculated: &Value) -> bool {
    match (stored, calculated) {
        (Value::Number(stored), Value::Number(calculated)) => {
            stored.is_finite()
                && calculated.is_finite()
                && (stored - calculated).abs() <= 1e-9 * stored.abs().max(1.0)
        }
        _ => stored == calculated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calamine::{Cell, ExcelDateTime, ExcelDateTimeType};

    fn date_cell(row: u32, column: u32, serial: f64) -> Cell<Data> {
        Cell::new(
            (row, column),
            Data::DateTime(ExcelDateTime::new(
                serial,
                ExcelDateTimeType::DateTime,
                false,
            )),
        )
    }

    fn ranges(
        values: Vec<Cell<Data>>,
        formulas: Vec<Cell<String>>,
    ) -> Vec<(String, Range<Data>, Range<String>)> {
        vec![(
            "Sheet1".into(),
            Range::from_sparse(values),
            Range::from_sparse(formulas),
        )]
    }

    #[test]
    fn imports_formulas_and_compares_calculated_values_with_cached_values() {
        let imported = import_ranges(
            ranges(
                vec![
                    Cell::new((0, 0), Data::Int(2)),
                    Cell::new((1, 0), Data::Int(3)),
                    Cell::new((2, 0), Data::Int(5)),
                ],
                vec![Cell::new((2, 0), "SUM(A1:A2)".into())],
            ),
            "a".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();

        assert_eq!(imported.sheets[0].name, "Sheet1");
        assert_eq!(
            imported.workbook.value(CellId::new(0, 2, 0)),
            Value::Number(5.0)
        );
        assert_eq!(
            imported.parity(),
            ParitySummary {
                formula_cells_observed: 1,
                formula_cells_loaded: 1,
                formula_cells_compared: 1,
                stored_values_matched: 1,
                stored_values_mismatched: 0,
                unsupported_formulas: 0,
            }
        );
    }

    #[test]
    fn keeps_cached_values_when_formulas_are_unsupported() {
        let imported = import_ranges(
            ranges(
                vec![Cell::new((0, 0), Data::Int(2))],
                vec![Cell::new((0, 0), "MEDIAN(1,2,3)".into())],
            ),
            "b".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();

        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(2.0)
        );
        assert_eq!(imported.unsupported.len(), 1);
        assert_eq!(imported.parity().formula_cells_compared, 0);
    }

    #[test]
    fn rejects_large_used_ranges_before_materialising_cells() {
        let error = import_ranges(
            ranges(
                vec![
                    Cell::new((0, 0), Data::Int(1)),
                    Cell::new((10, 10), Data::Int(2)),
                ],
                vec![],
            ),
            "c".repeat(64),
            ImportLimits {
                max_cells: 100,
                ..ImportLimits::default()
            },
        )
        .err()
        .expect("range should be rejected");
        assert_eq!(
            error,
            ImportError::TooManyCells {
                observed: 121,
                maximum: 100,
            }
        );
    }

    #[test]
    fn preserves_absolute_coordinates_for_offset_ranges() {
        let imported = import_ranges(
            ranges(
                vec![
                    Cell::new((2, 2), Data::Int(1)),
                    Cell::new((4, 2), Data::Int(2)),
                ],
                vec![Cell::new((4, 2), "C3+1".into())],
            ),
            "d".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();
        assert_eq!(
            imported.workbook.value(CellId::new(0, 4, 2)),
            Value::Number(2.0)
        );
        assert_eq!(imported.parity().stored_values_matched, 1);
    }

    #[test]
    fn rejects_formula_counts_before_loading_the_owned_graph() {
        let error = import_ranges(
            ranges(
                vec![Cell::new((0, 0), Data::Int(1))],
                vec![Cell::new((0, 0), "1+1".into())],
            ),
            "e".repeat(64),
            ImportLimits {
                max_formulas: 0,
                ..ImportLimits::default()
            },
        )
        .err()
        .expect("formula count should be rejected");
        assert_eq!(
            error,
            ImportError::TooManyFormulas {
                observed: 1,
                maximum: 0,
            }
        );
    }

    #[test]
    fn imports_date_cells_as_serials_and_matches_stored_date_formula_values() {
        let imported = import_ranges(
            ranges(
                vec![
                    date_cell(0, 0, 45_322.0), // 2024-01-31
                    Cell::new((0, 1), Data::Int(2024)),
                    Cell::new((0, 2), Data::Int(1)),
                    Cell::new((0, 3), Data::Int(31)),
                    date_cell(0, 4, 45_351.0), // EDATE clamps to 2024-02-29
                    date_cell(0, 5, 45_351.0),
                    date_cell(0, 6, 45_322.0),
                    Cell::new((0, 7), Data::Int(4)), // Wednesday
                    date_cell(1, 0, 60.0),           // the fictitious 1900-02-29
                    Cell::new((1, 1), Data::Int(1900)),
                    Cell::new((1, 2), Data::Int(2)),
                    Cell::new((1, 3), Data::Int(29)),
                ],
                vec![
                    Cell::new((0, 1), "YEAR(A1)".into()),
                    Cell::new((0, 2), "MONTH(A1)".into()),
                    Cell::new((0, 3), "DAY(A1)".into()),
                    Cell::new((0, 4), "EDATE(A1,1)".into()),
                    Cell::new((0, 5), "EOMONTH(A1,1)".into()),
                    Cell::new((0, 6), "DATE(B1,C1,D1)".into()),
                    Cell::new((0, 7), "WEEKDAY(A1)".into()),
                    Cell::new((1, 1), "YEAR(A2)".into()),
                    Cell::new((1, 2), "MONTH(A2)".into()),
                    Cell::new((1, 3), "DAY(A2)".into()),
                ],
            ),
            "g".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();

        assert_eq!(imported.date_system, "1900");
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(45_322.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 6)),
            Value::Number(45_322.0)
        );
        assert_eq!(
            imported.parity(),
            ParitySummary {
                formula_cells_observed: 10,
                formula_cells_loaded: 10,
                formula_cells_compared: 10,
                stored_values_matched: 10,
                stored_values_mismatched: 0,
                unsupported_formulas: 0,
            }
        );
    }

    #[test]
    fn rejects_the_1904_date_system_before_reading_any_cell() {
        assert_eq!(check_date_system(false), Ok(()));
        let error = check_date_system(true).unwrap_err();
        assert_eq!(
            error,
            ImportError::UnsupportedDateSystem { observed: "1904" }
        );
        assert_eq!(
            error.to_string(),
            "workbook uses the 1904 date system; only the 1900 date system is supported"
        );
    }

    #[test]
    fn registers_sheet_names_before_compiling_cross_sheet_formulas() {
        let imported = import_ranges(
            vec![
                (
                    "Inputs".into(),
                    Range::from_sparse(vec![Cell::new((0, 0), Data::Int(2))]),
                    Range::empty(),
                ),
                (
                    "Summary".into(),
                    Range::from_sparse(vec![Cell::new((0, 0), Data::Int(4))]),
                    Range::from_sparse(vec![Cell::new((0, 0), "Inputs!A1*2".into())]),
                ),
            ],
            "f".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();
        assert_eq!(
            imported.workbook.value(CellId::new(1, 0, 0)),
            Value::Number(4.0)
        );
        assert_eq!(imported.parity().stored_values_matched, 1);
        assert!(imported.unsupported.is_empty());
    }
}
