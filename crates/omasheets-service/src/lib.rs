//! One local API over native documents, for the CLI, the future grid and
//! agents alike.
//!
//! [`Service`] is the in-process form: it owns open [`Store`]s and answers
//! [`Request`]s with [`Response`]s, both plain data that serialise to JSON.
//! The `omasheets-service` binary serves the same API over a Unix socket in
//! the user's runtime directory, gated by a per-session token, and its
//! `call` subcommand is the CLI client. Nothing here publishes a document
//! or grants an agent merge authority: merges need a human approver, and an
//! agent may not append to the `main` branch at all.

use arrow_array::{ArrayRef, BooleanArray, Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use omasheets_calc::Value;
use omasheets_core::{
    Actor, ActorKind, CellInput, CellRef, CellState, CellValue, CheckResult, ColumnId, ColumnType,
    Command, Document, DocumentId, Event, InferredColumnType, Lineage, Literal, MAX_BATCH,
    ObjectId, Operation, SheetId,
};
use omasheets_store::{BranchDiff, LoadReport, MergeReport, Store, StoreError};
use omasheets_xlsx::{ImportLimits, import_xlsx};
use parquet::arrow::ArrowWriter;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Largest cell page one `cells` request returns.
pub const MAX_CELL_PAGE: usize = 10_000;
/// Largest rectangular grid page one `grid_page` request may inspect.
pub const MAX_GRID_PAGE_CELLS: usize = 10_000;
/// Largest native CSV projection, checked before the output file is created.
pub const MAX_CSV_EXPORT_CELLS: usize = 10_000_000;
/// Largest native XLSX projection, checked before the package is created.
pub const MAX_XLSX_EXPORT_CELLS: usize = 10_000_000;
/// Largest native Parquet projection, checked before output is created.
pub const MAX_PARQUET_EXPORT_CELLS: usize = 10_000_000;
const PARQUET_BATCH_ROWS: usize = 65_536;
/// Largest occupied rectangle accepted into one native alpha document.
pub const MAX_NATIVE_IMPORT_CELLS: usize = 100_000;
/// Largest source package read by the local import endpoint.
pub const MAX_NATIVE_IMPORT_BYTES: u64 = 50 * 1024 * 1024;
pub const MAX_NATIVE_IMPORT_SHEETS: usize = 64;
pub const MAX_NATIVE_IMPORT_FORMULAS: usize = 100_000;
const DEFAULT_CELL_PAGE: usize = 1_000;
const MAX_ACTOR_CHARS: usize = 128;
const MAX_CSV_FIELD_BYTES: usize = 1_000_000;
const MAIN_BRANCH: &str = "main";
static EXPORT_NONCE: AtomicU64 = AtomicU64::new(0);

/// What a client asks for. Every request names the document by path, so
/// one service can hold several documents open at once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    /// Create a new document file. Refuses to replace an existing file.
    Create {
        path: PathBuf,
        name: String,
        actor: Actor,
    },
    Open {
        path: PathBuf,
    },
    /// Write a final snapshot and release the file.
    Close {
        path: PathBuf,
    },
    /// Sheets, branches, head and counts of one branch.
    Document {
        path: PathBuf,
        #[serde(default)]
        branch: Option<String>,
    },
    /// A bounded page of the non-blank cells of one sheet in row-major
    /// view order.
    Cells {
        path: PathBuf,
        #[serde(default)]
        branch: Option<String>,
        sheet: String,
        #[serde(default)]
        start: usize,
        #[serde(default)]
        limit: Option<usize>,
    },
    /// A bounded rectangular page in current row/column view order. Blank
    /// cells are omitted from the sparse result.
    GridPage {
        path: PathBuf,
        #[serde(default)]
        branch: Option<String>,
        sheet: String,
        row_start: usize,
        column_start: usize,
        rows: usize,
        columns: usize,
    },
    Cell {
        path: PathBuf,
        #[serde(default)]
        branch: Option<String>,
        sheet: String,
        a1: String,
    },
    Lineage {
        path: PathBuf,
        #[serde(default)]
        branch: Option<String>,
        sheet: String,
        a1: String,
    },
    /// Append one command as `actor`. Agents may not append to `main`.
    Append {
        path: PathBuf,
        #[serde(default)]
        branch: Option<String>,
        actor: Actor,
        command: Command,
    },
    Branch {
        path: PathBuf,
        name: String,
        #[serde(default)]
        from: Option<String>,
        actor: Actor,
    },
    Check {
        path: PathBuf,
        #[serde(default)]
        branch: Option<String>,
    },
    Diff {
        path: PathBuf,
        source: String,
        #[serde(default)]
        target: Option<String>,
    },
    /// Merge `source` into `target`; only a human may approve.
    Merge {
        path: PathBuf,
        source: String,
        #[serde(default)]
        target: Option<String>,
        approver: Actor,
    },
    /// Project one native sheet's calculated values to a new CSV file. The
    /// destination is never overwritten and formulas are disclosed as values.
    ExportCsv {
        path: PathBuf,
        #[serde(default)]
        branch: Option<String>,
        sheet: String,
        output: PathBuf,
    },
    /// Project every native sheet to a new XLSX package. Supported formulas
    /// are retained; formulas whose stable bindings cannot be represented are
    /// flattened to their calculated value and counted in the manifest.
    ExportXlsx {
        path: PathBuf,
        #[serde(default)]
        branch: Option<String>,
        output: PathBuf,
    },
    /// Project one native sheet to typed nullable Parquet columns. Mixed
    /// columns are refused rather than silently coerced.
    ExportParquet {
        path: PathBuf,
        #[serde(default)]
        branch: Option<String>,
        sheet: String,
        output: PathBuf,
    },
    /// Convert one bounded XLSX package into a new replayable native file.
    /// The destination is never overwritten.
    ImportXlsx {
        source: PathBuf,
        output: PathBuf,
        actor: Actor,
        #[serde(default)]
        name: Option<String>,
    },
    Snapshot {
        path: PathBuf,
        #[serde(default)]
        branch: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SheetSummary {
    pub id: SheetId,
    pub name: String,
    pub rows: usize,
    pub columns: usize,
    pub cells: usize,
    #[serde(default)]
    pub column_types: Vec<ColumnSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnSummary {
    pub id: ColumnId,
    pub position: usize,
    pub declared: ColumnType,
    pub inferred: InferredColumnType,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub id: DocumentId,
    pub name: String,
    pub branch: String,
    pub branches: Vec<String>,
    pub head: Option<omasheets_core::EventId>,
    pub event_count: u64,
    pub digest: String,
    pub sheets: Vec<SheetSummary>,
    pub checks: usize,
    pub watches: usize,
    pub load: Option<LoadReport>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellReport {
    pub cell: CellRef,
    pub a1: Option<String>,
    pub value: CellValue,
    pub state: Option<CellState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellPage {
    pub sheet: SheetId,
    pub start: usize,
    pub total: usize,
    pub cells: Vec<CellReport>,
    /// Row-major position of the next page, or `None` at the end.
    pub next: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridCell {
    pub row: usize,
    pub column: usize,
    pub a1: Option<String>,
    pub value: CellValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridPage {
    pub sheet: SheetId,
    pub row_start: usize,
    pub column_start: usize,
    pub rows: usize,
    pub columns: usize,
    pub cells: Vec<GridCell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CsvExportManifest {
    pub format: String,
    pub output: PathBuf,
    pub branch: String,
    pub sheet: SheetId,
    pub sheet_name: String,
    pub rows: usize,
    pub columns: usize,
    pub document_digest: String,
    pub formula_cells: usize,
    pub potential_formula_injection_cells: usize,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct XlsxExportSheetManifest {
    pub id: SheetId,
    pub name: String,
    pub rows: usize,
    pub columns: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct XlsxExportManifest {
    pub format: String,
    pub output: PathBuf,
    pub branch: String,
    pub document_digest: String,
    pub sheets: Vec<XlsxExportSheetManifest>,
    pub formula_cells: usize,
    pub formula_cells_preserved: usize,
    pub formula_cells_flattened: usize,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParquetColumnManifest {
    pub id: ColumnId,
    pub position: usize,
    pub name: String,
    pub inferred: InferredColumnType,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParquetExportManifest {
    pub format: String,
    pub output: PathBuf,
    pub branch: String,
    pub sheet: SheetId,
    pub sheet_name: String,
    pub rows: usize,
    pub columns: Vec<ParquetColumnManifest>,
    pub document_digest: String,
    pub formula_cells: usize,
    pub error_cells_as_null: usize,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportedSheetManifest {
    pub id: SheetId,
    pub name: String,
    pub rows: usize,
    pub columns: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct XlsxImportManifest {
    pub format: String,
    pub output: PathBuf,
    pub document: DocumentId,
    pub document_digest: String,
    pub source_sha256: String,
    pub date_system: String,
    pub sheets: Vec<ImportedSheetManifest>,
    pub occupied_rectangle_cells: usize,
    pub value_cells_imported: usize,
    pub formula_cells_observed: usize,
    pub formula_cells_native: usize,
    pub formula_cells_cached_only: usize,
    pub formula_cells_omitted: usize,
    pub owned_engine_unsupported_formulas: usize,
    pub error_cells_omitted: usize,
    pub rejected_value_cells_omitted: usize,
    pub skipped_source_sheets: usize,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    Created {
        document: DocumentId,
        branch: String,
    },
    Opened {
        document: DocumentId,
        branches: Vec<String>,
    },
    Closed,
    Document(DocumentSummary),
    Cells(CellPage),
    GridPage(GridPage),
    Cell(CellReport),
    Lineage(Option<Lineage>),
    Appended(Event),
    Branched {
        branch: String,
        id: String,
    },
    Checked {
        passed: bool,
        results: Vec<CheckResult>,
    },
    Diff(BranchDiff),
    Merged(MergeReport),
    ExportedCsv(CsvExportManifest),
    ExportedXlsx(XlsxExportManifest),
    ExportedParquet(ParquetExportManifest),
    ImportedXlsx(XlsxImportManifest),
    Snapshot {
        digest: String,
    },
}

/// A refusal, with a stable machine-readable code and a sentence for people.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ServiceError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ServiceError {}

impl From<StoreError> for ServiceError {
    fn from(error: StoreError) -> Self {
        let code = match &error {
            StoreError::NotADocumentStore(_) => "not_a_document",
            StoreError::UnknownBranch(_) => "unknown_branch",
            StoreError::DuplicateBranch(_) => "duplicate_branch",
            StoreError::Unauthorized(_) => "unauthorized",
            StoreError::ChecksFailed(_) => "checks_failed",
            StoreError::Conflicts(_) => "conflicts",
            StoreError::NothingToMerge => "nothing_to_merge",
            StoreError::Apply(_) => "rejected",
            _ => "store",
        };
        let details = match &error {
            StoreError::ChecksFailed(results) => serde_json::to_value(results).ok(),
            StoreError::Conflicts(touches) => serde_json::to_value(touches).ok(),
            _ => None,
        };
        Self {
            code: code.into(),
            message: error.to_string(),
            details,
        }
    }
}

/// The in-process service: open stores keyed by canonical path.
pub struct Service {
    stores: BTreeMap<PathBuf, Store>,
    clock: Box<dyn Fn() -> i64 + Send>,
}

impl fmt::Debug for Service {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Service")
            .field("open", &self.stores.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Default for Service {
    fn default() -> Self {
        Self::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as i64)
                .unwrap_or(0)
        })
    }
}

fn canonical(path: &Path) -> Result<PathBuf, ServiceError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let name = path
        .file_name()
        .ok_or_else(|| ServiceError::new("invalid_path", "document path has no file name"))?;
    let parent = parent.canonicalize().map_err(|error| {
        ServiceError::new("invalid_path", format!("{}: {error}", path.display()))
    })?;
    Ok(parent.join(name))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CsvExportStats {
    formula_cells: usize,
    potential_formula_injection_cells: usize,
}

fn export_csv(
    document: &omasheets_core::Document,
    sheet: SheetId,
    output: &Path,
) -> Result<(PathBuf, CsvExportStats), ServiceError> {
    let output = canonical(output)?;
    if output.exists() {
        return Err(ServiceError::new(
            "output_exists",
            "CSV output already exists",
        ));
    }
    let rows = document.rows(sheet).unwrap_or(&[]);
    let columns = document.columns(sheet).unwrap_or(&[]);
    let cell_count = rows
        .len()
        .checked_mul(columns.len())
        .ok_or_else(|| ServiceError::new("export_too_large", "CSV dimensions overflow"))?;
    if cell_count > MAX_CSV_EXPORT_CELLS {
        return Err(ServiceError::new(
            "export_too_large",
            format!("CSV export may cover at most {MAX_CSV_EXPORT_CELLS} cells"),
        ));
    }

    let parent = output.parent().expect("canonical output has a parent");
    let file_name = output
        .file_name()
        .expect("canonical output has a file name")
        .to_string_lossy();
    let mut temporary = None;
    for _ in 0..16 {
        let nonce = EXPORT_NONCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{file_name}.{}.{}.part",
            std::process::id(),
            nonce
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(export_io_error(error)),
        }
    }
    let (temporary_path, mut file) = temporary
        .ok_or_else(|| ServiceError::new("export_io", "could not allocate a temporary CSV file"))?;

    let written = (|| -> Result<CsvExportStats, ServiceError> {
        let mut stats = CsvExportStats::default();
        for (row_index, row) in rows.iter().enumerate() {
            for (column_index, column) in columns.iter().enumerate() {
                if column_index != 0 {
                    file.write_all(b",").map_err(export_io_error)?;
                }
                let cell = CellRef {
                    sheet,
                    row: *row,
                    column: *column,
                };
                if matches!(
                    document.cell(cell).map(|state| &state.input),
                    Some(CellInput::Formula { .. })
                ) {
                    stats.formula_cells += 1;
                }
                let value = document.value(cell);
                let text = csv_value(&value);
                if text.len() > MAX_CSV_FIELD_BYTES {
                    return Err(ServiceError::new(
                        "field_too_large",
                        format!("CSV fields may contain at most {MAX_CSV_FIELD_BYTES} bytes"),
                    ));
                }
                if matches!(value, CellValue::Text(_))
                    && text
                        .chars()
                        .next()
                        .is_some_and(|first| matches!(first, '=' | '+' | '-' | '@' | '\t' | '\r'))
                {
                    stats.potential_formula_injection_cells += 1;
                }
                write_csv_field(&mut file, &text).map_err(export_io_error)?;
            }
            if row_index + 1 < rows.len() {
                file.write_all(b"\r\n").map_err(export_io_error)?;
            }
        }
        file.sync_all().map_err(export_io_error)?;
        Ok(stats)
    })();
    drop(file);
    let stats = match written {
        Ok(stats) => stats,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary_path);
            return Err(error);
        }
    };
    let linked = std::fs::hard_link(&temporary_path, &output);
    let _ = std::fs::remove_file(&temporary_path);
    match linked {
        Ok(()) => Ok((output, stats)),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(ServiceError::new(
            "output_exists",
            "CSV output already exists",
        )),
        Err(error) => Err(export_io_error(error)),
    }
}

fn csv_value(value: &CellValue) -> String {
    match value {
        CellValue::Blank => String::new(),
        CellValue::Number(number) if *number == 0.0 => "0".into(),
        CellValue::Number(number) => number.to_string(),
        CellValue::Text(text) | CellValue::Error(text) => text.clone(),
        CellValue::Boolean(true) => "TRUE".into(),
        CellValue::Boolean(false) => "FALSE".into(),
    }
}

fn write_csv_field(writer: &mut impl Write, text: &str) -> io::Result<()> {
    if text.contains([',', '"', '\r', '\n']) {
        writer.write_all(b"\"")?;
        writer.write_all(text.replace('"', "\"\"").as_bytes())?;
        writer.write_all(b"\"")
    } else {
        writer.write_all(text.as_bytes())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ParquetExportStats {
    formula_cells: usize,
    error_cells_as_null: usize,
}

fn parquet_error(error: impl fmt::Display) -> ServiceError {
    ServiceError::new("parquet_export", error.to_string())
}

fn parquet_type(inferred: InferredColumnType) -> Result<DataType, ServiceError> {
    match inferred {
        InferredColumnType::Number | InferredColumnType::DateSerial => Ok(DataType::Float64),
        InferredColumnType::Text | InferredColumnType::Empty => Ok(DataType::Utf8),
        InferredColumnType::Boolean => Ok(DataType::Boolean),
        InferredColumnType::Mixed => Err(ServiceError::new(
            "mixed_column",
            "Parquet export refuses mixed columns instead of coercing values",
        )),
    }
}

fn parquet_column_array(
    document: &Document,
    sheet: SheetId,
    rows: &[omasheets_core::RowId],
    column: ColumnId,
    inferred: InferredColumnType,
    stats: &mut ParquetExportStats,
) -> Result<ArrayRef, ServiceError> {
    let values = rows.iter().map(|row| {
        let cell = CellRef {
            sheet,
            row: *row,
            column,
        };
        if matches!(
            document.cell(cell).map(|state| &state.input),
            Some(CellInput::Formula { .. })
        ) {
            stats.formula_cells += 1;
        }
        let value = document.value(cell);
        if matches!(value, CellValue::Error(_)) {
            stats.error_cells_as_null += 1;
        }
        value
    });
    match inferred {
        InferredColumnType::Number | InferredColumnType::DateSerial => {
            let values = values
                .map(|value| match value {
                    CellValue::Blank | CellValue::Error(_) => Ok(None),
                    CellValue::Number(number) => Ok(Some(number)),
                    _ => Err(ServiceError::new(
                        "parquet_type_changed",
                        "column type changed while building the Parquet projection",
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Arc::new(Float64Array::from(values)))
        }
        InferredColumnType::Boolean => {
            let values = values
                .map(|value| match value {
                    CellValue::Blank | CellValue::Error(_) => Ok(None),
                    CellValue::Boolean(flag) => Ok(Some(flag)),
                    _ => Err(ServiceError::new(
                        "parquet_type_changed",
                        "column type changed while building the Parquet projection",
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Arc::new(BooleanArray::from(values)))
        }
        InferredColumnType::Text => {
            let values = values
                .map(|value| match value {
                    CellValue::Blank | CellValue::Error(_) => Ok(None),
                    CellValue::Text(text) => Ok(Some(text)),
                    _ => Err(ServiceError::new(
                        "parquet_type_changed",
                        "column type changed while building the Parquet projection",
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Arc::new(StringArray::from_iter(
                values.iter().map(Option::as_deref),
            )))
        }
        InferredColumnType::Empty => Ok(Arc::new(StringArray::new_null(rows.len()))),
        InferredColumnType::Mixed => Err(ServiceError::new(
            "mixed_column",
            "Parquet export refuses mixed columns instead of coercing values",
        )),
    }
}

fn export_parquet(
    document: &Document,
    sheet: SheetId,
    output: &Path,
) -> Result<(PathBuf, Vec<ParquetColumnManifest>, ParquetExportStats), ServiceError> {
    let output = canonical(output)?;
    if output.exists() {
        return Err(ServiceError::new(
            "output_exists",
            "Parquet output already exists",
        ));
    }
    let rows = document.rows(sheet).unwrap_or(&[]);
    let columns = document.columns(sheet).unwrap_or(&[]);
    if columns.is_empty() {
        return Err(ServiceError::new(
            "empty_sheet",
            "Parquet export needs at least one column",
        ));
    }
    let cell_count = rows
        .len()
        .checked_mul(columns.len())
        .ok_or_else(|| ServiceError::new("export_too_large", "Parquet dimensions overflow"))?;
    if cell_count > MAX_PARQUET_EXPORT_CELLS {
        return Err(ServiceError::new(
            "export_too_large",
            format!("Parquet export may cover at most {MAX_PARQUET_EXPORT_CELLS} cells"),
        ));
    }

    let mut manifests = Vec::with_capacity(columns.len());
    let mut fields = Vec::with_capacity(columns.len());
    for (position, column) in columns.iter().enumerate() {
        let inferred = document
            .inferred_column_type(sheet, *column)
            .ok_or_else(|| ServiceError::new("unknown_column", "Parquet column disappeared"))?;
        let name = a1(0, position as u32).trim_end_matches('1').to_string();
        fields.push(Field::new(&name, parquet_type(inferred)?, true));
        manifests.push(ParquetColumnManifest {
            id: *column,
            position,
            name,
            inferred,
        });
    }
    let schema = Arc::new(Schema::new(fields));

    let parent = output.parent().expect("canonical output has a parent");
    let file_name = output
        .file_name()
        .expect("canonical output has a file name")
        .to_string_lossy();
    let mut temporary = None;
    for _ in 0..16 {
        let nonce = EXPORT_NONCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{file_name}.{}.{}.part",
            std::process::id(),
            nonce
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(export_io_error(error)),
        }
    }
    let (temporary_path, file) = temporary.ok_or_else(|| {
        ServiceError::new("export_io", "could not allocate a temporary Parquet file")
    })?;

    let written = (|| -> Result<ParquetExportStats, ServiceError> {
        let mut stats = ParquetExportStats::default();
        let mut writer = ArrowWriter::try_new(file, schema.clone(), None).map_err(parquet_error)?;
        for batch_rows in rows.chunks(PARQUET_BATCH_ROWS) {
            let arrays = columns
                .iter()
                .zip(&manifests)
                .map(|(column, manifest)| {
                    parquet_column_array(
                        document,
                        sheet,
                        batch_rows,
                        *column,
                        manifest.inferred,
                        &mut stats,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let batch = RecordBatch::try_new(schema.clone(), arrays).map_err(parquet_error)?;
            writer.write(&batch).map_err(parquet_error)?;
            writer.flush().map_err(parquet_error)?;
        }
        let file = writer.into_inner().map_err(parquet_error)?;
        file.sync_all().map_err(export_io_error)?;
        Ok(stats)
    })();
    let stats = match written {
        Ok(stats) => stats,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary_path);
            return Err(error);
        }
    };
    let linked = std::fs::hard_link(&temporary_path, &output);
    let _ = std::fs::remove_file(&temporary_path);
    match linked {
        Ok(()) => Ok((output, manifests, stats)),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(ServiceError::new(
            "output_exists",
            "Parquet output already exists",
        )),
        Err(error) => Err(export_io_error(error)),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct XlsxExportStats {
    formula_cells: usize,
    formula_cells_preserved: usize,
    formula_cells_flattened: usize,
}

fn xml_text(value: &str) -> Result<String, ServiceError> {
    if value.chars().any(|character| {
        !matches!(
            character,
            '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}'
                | '\u{10000}'..='\u{10FFFF}'
        )
    }) {
        return Err(ServiceError::new(
            "unsupported_xml_text",
            "XLSX cannot represent XML control characters",
        ));
    }
    Ok(value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;"))
}

fn valid_xlsx_sheet_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 31
        && !name.chars().any(|character| "[]:*?/\\".contains(character))
        && !name.starts_with('\'')
        && !name.ends_with('\'')
}

fn xlsx_error(error: impl fmt::Display) -> ServiceError {
    ServiceError::new("xlsx_export", error.to_string())
}

fn xlsx_formula_is_portable(
    document: &Document,
    cell: CellRef,
    formula: &omasheets_core::CompiledFormula,
) -> bool {
    formula.current_table.is_none()
        && formula.table_bindings.is_empty()
        && document
            .compile_formula(cell.sheet, &formula.source)
            .is_ok_and(|current| current.references == formula.references)
}

fn write_xlsx_value(
    writer: &mut impl Write,
    address: &str,
    value: &CellValue,
    formula: Option<&str>,
) -> Result<(), ServiceError> {
    let formula = formula
        .map(|source| source.strip_prefix('=').unwrap_or(source))
        .map(xml_text)
        .transpose()?;
    let formula_xml = formula
        .as_deref()
        .map(|source| format!("<f>{source}</f>"))
        .unwrap_or_default();
    let address = xml_text(address)?;
    match value {
        CellValue::Blank if formula.is_none() => Ok(()),
        CellValue::Blank => {
            write!(writer, "<c r=\"{address}\">{formula_xml}</c>").map_err(export_io_error)
        }
        CellValue::Number(number) => write!(
            writer,
            "<c r=\"{address}\">{formula_xml}<v>{}</v></c>",
            if *number == 0.0 { 0.0 } else { *number },
        )
        .map_err(export_io_error),
        CellValue::Boolean(flag) => write!(
            writer,
            "<c r=\"{address}\" t=\"b\">{formula_xml}<v>{}</v></c>",
            if *flag { 1 } else { 0 },
        )
        .map_err(export_io_error),
        CellValue::Text(text) if formula.is_some() => write!(
            writer,
            "<c r=\"{address}\" t=\"str\">{formula_xml}<v>{}</v></c>",
            xml_text(text)?,
        )
        .map_err(export_io_error),
        CellValue::Text(text) => write!(
            writer,
            "<c r=\"{address}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
            xml_text(text)?,
        )
        .map_err(export_io_error),
        CellValue::Error(error)
            if matches!(
                error.as_str(),
                "#DIV/0!" | "#REF!" | "#N/A" | "#VALUE!" | "#NUM!" | "#NAME?" | "#NULL!"
            ) =>
        {
            write!(
                writer,
                "<c r=\"{address}\" t=\"e\">{formula_xml}<v>{}</v></c>",
                xml_text(error)?,
            )
            .map_err(export_io_error)
        }
        CellValue::Error(error) => write!(
            writer,
            "<c r=\"{address}\" t=\"str\">{formula_xml}<v>{}</v></c>",
            xml_text(error)?,
        )
        .map_err(export_io_error),
    }
}

fn start_xlsx_file(
    writer: &mut zip::ZipWriter<std::fs::File>,
    name: &str,
) -> Result<(), ServiceError> {
    writer
        .start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .map_err(xlsx_error)
}

fn export_xlsx(
    document: &Document,
    output: &Path,
) -> Result<(PathBuf, Vec<XlsxExportSheetManifest>, XlsxExportStats), ServiceError> {
    let output = canonical(output)?;
    if output.exists() {
        return Err(ServiceError::new(
            "output_exists",
            "XLSX output already exists",
        ));
    }
    let mut sheets = Vec::with_capacity(document.sheets().len());
    let mut total_cells = 0_usize;
    let mut folded_names = std::collections::BTreeSet::new();
    for sheet in document.sheets() {
        let name = document.sheet_name(*sheet).unwrap_or("");
        if !valid_xlsx_sheet_name(name) || !folded_names.insert(name.to_lowercase()) {
            return Err(ServiceError::new(
                "unsupported_sheet_name",
                format!("sheet name cannot be represented in XLSX: {name:?}"),
            ));
        }
        xml_text(name)?;
        let rows = document.rows(*sheet).map_or(0, <[_]>::len);
        let columns = document.columns(*sheet).map_or(0, <[_]>::len);
        if rows > 1_048_576 || columns > 16_384 {
            return Err(ServiceError::new(
                "export_too_large",
                "XLSX sheets may contain at most 1,048,576 rows and 16,384 columns",
            ));
        }
        total_cells =
            total_cells
                .checked_add(rows.checked_mul(columns).ok_or_else(|| {
                    ServiceError::new("export_too_large", "XLSX dimensions overflow")
                })?)
                .ok_or_else(|| ServiceError::new("export_too_large", "XLSX dimensions overflow"))?;
        sheets.push(XlsxExportSheetManifest {
            id: *sheet,
            name: name.into(),
            rows,
            columns,
        });
    }
    if sheets.is_empty() {
        return Err(ServiceError::new(
            "empty_workbook",
            "XLSX export needs at least one sheet",
        ));
    }
    if total_cells > MAX_XLSX_EXPORT_CELLS {
        return Err(ServiceError::new(
            "export_too_large",
            format!("XLSX export may cover at most {MAX_XLSX_EXPORT_CELLS} cells"),
        ));
    }

    let parent = output.parent().expect("canonical output has a parent");
    let file_name = output
        .file_name()
        .expect("canonical output has a file name")
        .to_string_lossy();
    let mut temporary = None;
    for _ in 0..16 {
        let nonce = EXPORT_NONCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{file_name}.{}.{}.part",
            std::process::id(),
            nonce
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(export_io_error(error)),
        }
    }
    let (temporary_path, file) = temporary.ok_or_else(|| {
        ServiceError::new("export_io", "could not allocate a temporary XLSX file")
    })?;

    let written = (|| -> Result<XlsxExportStats, ServiceError> {
        let mut writer = zip::ZipWriter::new(file);
        start_xlsx_file(&mut writer, "[Content_Types].xml")?;
        write!(writer, "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>").map_err(export_io_error)?;
        for index in 1..=sheets.len() {
            write!(writer, "<Override PartName=\"/xl/worksheets/sheet{index}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>").map_err(export_io_error)?;
        }
        writer.write_all(b"</Types>").map_err(export_io_error)?;

        start_xlsx_file(&mut writer, "_rels/.rels")?;
        writer.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>").map_err(export_io_error)?;

        start_xlsx_file(&mut writer, "xl/workbook.xml")?;
        writer.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><workbookPr date1904=\"0\"/><sheets>").map_err(export_io_error)?;
        for (index, sheet) in sheets.iter().enumerate() {
            write!(
                writer,
                "<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
                xml_text(&sheet.name)?,
                index + 1,
                index + 1
            )
            .map_err(export_io_error)?;
        }
        writer.write_all(b"</sheets><calcPr calcMode=\"auto\" fullCalcOnLoad=\"1\" forceFullCalc=\"1\"/></workbook>").map_err(export_io_error)?;

        start_xlsx_file(&mut writer, "xl/_rels/workbook.xml.rels")?;
        writer.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">").map_err(export_io_error)?;
        for index in 1..=sheets.len() {
            write!(writer, "<Relationship Id=\"rId{index}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{index}.xml\"/>").map_err(export_io_error)?;
        }
        writer
            .write_all(b"</Relationships>")
            .map_err(export_io_error)?;

        let mut stats = XlsxExportStats::default();
        for (sheet_index, sheet_manifest) in sheets.iter().enumerate() {
            start_xlsx_file(
                &mut writer,
                &format!("xl/worksheets/sheet{}.xml", sheet_index + 1),
            )?;
            writer.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>").map_err(export_io_error)?;
            let rows = document.rows(sheet_manifest.id).unwrap_or(&[]);
            let columns = document.columns(sheet_manifest.id).unwrap_or(&[]);
            for (row_index, row) in rows.iter().enumerate() {
                let mut row_open = false;
                for (column_index, column) in columns.iter().enumerate() {
                    let cell = CellRef {
                        sheet: sheet_manifest.id,
                        row: *row,
                        column: *column,
                    };
                    let Some(state) = document.cell(cell) else {
                        continue;
                    };
                    if !row_open {
                        write!(writer, "<row r=\"{}\">", row_index + 1).map_err(export_io_error)?;
                        row_open = true;
                    }
                    let value = document.value(cell);
                    let formula = match &state.input {
                        CellInput::Formula { formula } => {
                            stats.formula_cells += 1;
                            if xlsx_formula_is_portable(document, cell, formula) {
                                stats.formula_cells_preserved += 1;
                                Some(formula.source.as_str())
                            } else {
                                stats.formula_cells_flattened += 1;
                                None
                            }
                        }
                        CellInput::Value { .. } => None,
                    };
                    write_xlsx_value(
                        &mut writer,
                        &a1(row_index as u32, column_index as u32),
                        &value,
                        formula,
                    )?;
                }
                if row_open {
                    writer.write_all(b"</row>").map_err(export_io_error)?;
                }
            }
            writer
                .write_all(b"</sheetData></worksheet>")
                .map_err(export_io_error)?;
        }
        let file = writer.finish().map_err(xlsx_error)?;
        file.sync_all().map_err(export_io_error)?;
        Ok(stats)
    })();
    let stats = match written {
        Ok(stats) => stats,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary_path);
            return Err(error);
        }
    };
    let linked = std::fs::hard_link(&temporary_path, &output);
    let _ = std::fs::remove_file(&temporary_path);
    match linked {
        Ok(()) => Ok((output, sheets, stats)),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(ServiceError::new(
            "output_exists",
            "XLSX output already exists",
        )),
        Err(error) => Err(export_io_error(error)),
    }
}

fn export_io_error(error: io::Error) -> ServiceError {
    ServiceError::new("export_io", error.to_string())
}

fn plan_required(
    document: &mut Document,
    actor: &Actor,
    timestamp: i64,
    commands: &mut Vec<Command>,
    command: Command,
) -> Result<Event, ServiceError> {
    let event = document.command(actor.clone(), timestamp, command.clone())?;
    commands.push(command);
    Ok(event)
}

fn plan_optional(
    document: &mut Document,
    actor: &Actor,
    timestamp: i64,
    commands: &mut Vec<Command>,
    command: Command,
) -> bool {
    match document.command(actor.clone(), timestamp, command.clone()) {
        Ok(_) => {
            commands.push(command);
            true
        }
        Err(_) => false,
    }
}

fn a1(row: u32, column: u32) -> String {
    let mut value = column as usize + 1;
    let mut letters = Vec::new();
    while value != 0 {
        value -= 1;
        letters.push((b'A' + (value % 26) as u8) as char);
        value /= 26;
    }
    letters.reverse();
    format!("{}{}", letters.into_iter().collect::<String>(), row + 1)
}

fn source_literal(value: &Value) -> Option<Literal> {
    match value {
        Value::Blank => None,
        Value::Number(number) => Some(Literal::Number(*number)),
        Value::Text(text) => Some(Literal::Text(text.clone())),
        Value::Boolean(flag) => Some(Literal::Boolean(*flag)),
        Value::Error(_) => None,
    }
}

fn cleanup_store_files(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let candidate = PathBuf::from(format!("{}{suffix}", path.display()));
        let _ = std::fs::remove_file(candidate);
    }
}

fn temporary_store_path(output: &Path) -> Result<PathBuf, ServiceError> {
    let parent = output.parent().expect("canonical output has a parent");
    let name = output
        .file_name()
        .expect("canonical output has a file name")
        .to_string_lossy();
    for _ in 0..16 {
        let nonce = EXPORT_NONCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{name}.{}.{}.importing",
            std::process::id(),
            nonce
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                drop(file);
                std::fs::remove_file(&candidate).map_err(export_io_error)?;
                return Ok(candidate);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(export_io_error(error)),
        }
    }
    Err(ServiceError::new(
        "import_io",
        "could not allocate a temporary native document",
    ))
}

fn import_native_xlsx(
    source: &Path,
    output: &Path,
    actor: Actor,
    name: Option<String>,
    timestamp: i64,
) -> Result<(Store, XlsxImportManifest), ServiceError> {
    let source = source.canonicalize().map_err(|error| {
        ServiceError::new("invalid_source", format!("{}: {error}", source.display()))
    })?;
    let metadata = source.metadata().map_err(|error| {
        ServiceError::new("invalid_source", format!("{}: {error}", source.display()))
    })?;
    if !metadata.is_file() {
        return Err(ServiceError::new(
            "invalid_source",
            "XLSX source is not a file",
        ));
    }
    if metadata.len() > MAX_NATIVE_IMPORT_BYTES {
        return Err(ServiceError::new(
            "import_too_large",
            format!("XLSX source may contain at most {MAX_NATIVE_IMPORT_BYTES} bytes"),
        ));
    }

    let output = canonical(output)?;
    if output.exists() {
        return Err(ServiceError::new(
            "output_exists",
            "native output already exists",
        ));
    }
    let imported = import_xlsx(
        &source,
        ImportLimits {
            max_sheets: MAX_NATIVE_IMPORT_SHEETS,
            max_cells: MAX_NATIVE_IMPORT_CELLS,
            max_formulas: MAX_NATIVE_IMPORT_FORMULAS,
        },
    )
    .map_err(|error| ServiceError::new("xlsx_import", error.to_string()))?;

    let occupied_rectangle_cells = imported.sheets.iter().try_fold(0_usize, |total, sheet| {
        sheet
            .rows
            .checked_mul(sheet.columns)
            .and_then(|cells| total.checked_add(cells))
    });
    let Some(occupied_rectangle_cells) = occupied_rectangle_cells else {
        return Err(ServiceError::new(
            "import_too_large",
            "XLSX dimensions overflow",
        ));
    };
    if occupied_rectangle_cells > MAX_NATIVE_IMPORT_CELLS {
        return Err(ServiceError::new(
            "import_too_large",
            format!("XLSX occupied rectangles may cover at most {MAX_NATIVE_IMPORT_CELLS} cells"),
        ));
    }

    let document_name = name.unwrap_or_else(|| {
        source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.trim().is_empty())
            .unwrap_or("Imported workbook")
            .to_string()
    });
    let seed = format!(
        "{}:{}:{timestamp}",
        output.display(),
        imported.source_sha256
    );
    let document_id = DocumentId(ObjectId::from_seed(&seed));
    let (mut planning, _) =
        Document::create(document_id, &document_name, actor.clone(), timestamp)?;
    let branch = planning.branch();
    let import_actor = Actor::new(ActorKind::Import, actor.id.clone());
    let mut commands = Vec::new();
    plan_required(
        &mut planning,
        &import_actor,
        timestamp,
        &mut commands,
        Command::Import {
            source_sha256: imported.source_sha256.clone(),
            format: "xlsx".into(),
        },
    )?;

    let mut sheet_ids = BTreeMap::new();
    let mut sheets = Vec::with_capacity(imported.sheets.len());
    for source_sheet in &imported.sheets {
        let event = plan_required(
            &mut planning,
            &import_actor,
            timestamp,
            &mut commands,
            Command::AddSheet {
                name: source_sheet.name.clone(),
            },
        )?;
        let Operation::AddSheet { sheet, .. } = event.operation else {
            unreachable!("AddSheet resolves to AddSheet")
        };
        sheet_ids.insert(source_sheet.index, sheet);
        for at in (0..source_sheet.columns).step_by(MAX_BATCH) {
            plan_required(
                &mut planning,
                &import_actor,
                timestamp,
                &mut commands,
                Command::AddColumns {
                    sheet,
                    count: (source_sheet.columns - at).min(MAX_BATCH),
                    at,
                },
            )?;
        }
        for at in (0..source_sheet.rows).step_by(MAX_BATCH) {
            plan_required(
                &mut planning,
                &import_actor,
                timestamp,
                &mut commands,
                Command::AddRows {
                    sheet,
                    count: (source_sheet.rows - at).min(MAX_BATCH),
                    at,
                    table: None,
                },
            )?;
        }
        sheets.push(ImportedSheetManifest {
            id: sheet,
            name: source_sheet.name.clone(),
            rows: source_sheet.rows,
            columns: source_sheet.columns,
        });
    }

    let mut value_cells_imported = 0;
    let mut formula_cells_observed = 0;
    let mut formula_cells_native = 0;
    let mut formula_cells_cached_only = 0;
    let mut formula_cells_omitted = 0;
    let mut error_cells_omitted = 0;
    let mut rejected_value_cells_omitted = 0;
    for source_cell in imported.source_cells() {
        let sheet = sheet_ids[&source_cell.cell.sheet];
        let address = a1(source_cell.cell.row, source_cell.cell.column);
        let cached_loaded = match source_literal(&source_cell.stored) {
            Some(value) => {
                let loaded = plan_optional(
                    &mut planning,
                    &import_actor,
                    timestamp,
                    &mut commands,
                    Command::SetValue {
                        sheet,
                        a1: address.clone(),
                        value,
                    },
                );
                if loaded {
                    value_cells_imported += 1;
                } else {
                    rejected_value_cells_omitted += 1;
                }
                loaded
            }
            None => {
                if matches!(source_cell.stored, Value::Error(_)) {
                    error_cells_omitted += 1;
                }
                false
            }
        };
        if let Some(formula) = &source_cell.formula {
            formula_cells_observed += 1;
            if plan_optional(
                &mut planning,
                &import_actor,
                timestamp,
                &mut commands,
                Command::SetFormula {
                    sheet,
                    a1: address,
                    source: formula.clone(),
                },
            ) {
                formula_cells_native += 1;
            } else if cached_loaded {
                formula_cells_cached_only += 1;
            } else {
                formula_cells_omitted += 1;
            }
        }
    }

    let expected_digest = planning.digest();
    let temporary = temporary_store_path(&output)?;
    let built = (|| -> Result<(), ServiceError> {
        let mut store = Store::create(&temporary, document_id, &document_name, actor, timestamp)?;
        store.append_batch(branch, import_actor, timestamp, commands)?;
        if store.document(branch)?.digest() != expected_digest {
            return Err(ServiceError::new(
                "import_replay_mismatch",
                "planned and persisted native documents differ",
            ));
        }
        store.close()?;
        Ok(())
    })();
    if let Err(error) = built {
        cleanup_store_files(&temporary);
        return Err(error);
    }
    let linked = std::fs::hard_link(&temporary, &output);
    cleanup_store_files(&temporary);
    match linked {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(ServiceError::new(
                "output_exists",
                "native output already exists",
            ));
        }
        Err(error) => return Err(ServiceError::new("import_io", error.to_string())),
    }
    let mut store = Store::open(&output)?;
    let reopened_branch = store.branch_id(MAIN_BRANCH)?;
    let reopened_digest = store.document(reopened_branch)?.digest();
    if reopened_digest != expected_digest {
        cleanup_store_files(&output);
        return Err(ServiceError::new(
            "import_replay_mismatch",
            "reopened native document differs from the imported state",
        ));
    }
    let owned_engine_unsupported_formulas = imported.unsupported.len();
    let skipped_source_sheets = imported.skipped_sheets.len();
    let manifest = XlsxImportManifest {
        format: "omasheets-native-v1".into(),
        output,
        document: document_id,
        document_digest: reopened_digest,
        source_sha256: imported.source_sha256,
        date_system: imported.date_system.into(),
        sheets,
        occupied_rectangle_cells,
        value_cells_imported,
        formula_cells_observed,
        formula_cells_native,
        formula_cells_cached_only,
        formula_cells_omitted,
        owned_engine_unsupported_formulas,
        error_cells_omitted,
        rejected_value_cells_omitted,
        skipped_source_sheets,
        limitations: vec![
            "styles_tables_charts_pivots_macros_not_imported".into(),
            "defined_names_not_imported".into(),
            "unsupported_formulas_use_cached_values_when_available".into(),
            "cached_error_values_not_imported".into(),
        ],
    };
    Ok((store, manifest))
}

fn check_actor(actor: &Actor) -> Result<(), ServiceError> {
    if actor.id.trim().is_empty() || actor.id.chars().count() > MAX_ACTOR_CHARS {
        return Err(ServiceError::new(
            "invalid_actor",
            "actor ids are 1 to 128 characters",
        ));
    }
    Ok(())
}

impl Service {
    /// A service whose event timestamps come from `clock` (milliseconds).
    pub fn new(clock: impl Fn() -> i64 + Send + 'static) -> Self {
        Self {
            stores: BTreeMap::new(),
            clock: Box::new(clock),
        }
    }

    pub fn open_documents(&self) -> Vec<&Path> {
        self.stores.keys().map(PathBuf::as_path).collect()
    }

    fn store(&mut self, path: &Path) -> Result<&mut Store, ServiceError> {
        let key = canonical(path)?;
        if !self.stores.contains_key(&key) {
            let store = Store::open(&key)?;
            self.stores.insert(key.clone(), store);
        }
        Ok(self.stores.get_mut(&key).expect("inserted above"))
    }

    fn branch(
        store: &Store,
        name: Option<&str>,
    ) -> Result<(String, omasheets_core::BranchId), ServiceError> {
        let name = name.unwrap_or(MAIN_BRANCH).to_string();
        let id = store.branch_id(&name)?;
        Ok((name, id))
    }

    fn sheet(document: &omasheets_core::Document, sheet: &str) -> Result<SheetId, ServiceError> {
        if let Some(id) = ObjectId::parse(sheet).map(SheetId)
            && document.sheets().contains(&id)
        {
            return Ok(id);
        }
        document
            .sheets()
            .iter()
            .copied()
            .find(|candidate| document.sheet_name(*candidate) == Some(sheet))
            .ok_or_else(|| ServiceError::new("unknown_sheet", format!("no sheet {sheet}")))
    }

    /// Answers one request. Errors never change a document: a rejected
    /// append writes nothing, a refused merge replays nothing.
    pub fn handle(&mut self, request: Request) -> Result<Response, ServiceError> {
        let now = (self.clock)();
        match request {
            Request::Create { path, name, actor } => {
                check_actor(&actor)?;
                let key = canonical(&path)?;
                if self.stores.contains_key(&key) {
                    return Err(ServiceError::new("exists", "document is already open"));
                }
                let seed = format!("{}:{now}", key.display());
                let mut store = Store::create(
                    &key,
                    DocumentId(ObjectId::from_seed(&seed)),
                    &name,
                    actor,
                    now,
                )?;
                let main = store.branch_id(MAIN_BRANCH)?;
                let document = store.document(main)?.id();
                self.stores.insert(key, store);
                Ok(Response::Created {
                    document,
                    branch: MAIN_BRANCH.into(),
                })
            }
            Request::Open { path } => {
                let store = self.store(&path)?;
                let main = store.branch_id(MAIN_BRANCH)?;
                let document = store.document(main)?.id();
                Ok(Response::Opened {
                    document,
                    branches: store.branch_names()?,
                })
            }
            Request::Close { path } => {
                let key = canonical(&path)?;
                match self.stores.remove(&key) {
                    Some(store) => {
                        store.close()?;
                        Ok(Response::Closed)
                    }
                    None => Err(ServiceError::new("not_open", "document is not open")),
                }
            }
            Request::Document { path, branch } => {
                let store = self.store(&path)?;
                let (branch_name, branch) = Self::branch(store, branch.as_deref())?;
                let branches = store.branch_names()?;
                let document = store.document(branch)?;
                let sheets = document
                    .sheets()
                    .iter()
                    .map(|sheet| {
                        let rows = document.rows(*sheet).map_or(0, <[_]>::len);
                        let column_ids = document.columns(*sheet).unwrap_or(&[]);
                        let columns = column_ids.len();
                        let column_types = column_ids
                            .iter()
                            .enumerate()
                            .map(|(position, column)| ColumnSummary {
                                id: *column,
                                position,
                                declared: document
                                    .column_type(*sheet, *column)
                                    .expect("listed column has a type"),
                                inferred: document
                                    .inferred_column_type(*sheet, *column)
                                    .expect("listed column can be inferred"),
                            })
                            .collect();
                        SheetSummary {
                            id: *sheet,
                            name: document.sheet_name(*sheet).unwrap_or("").to_string(),
                            rows,
                            columns,
                            cells: document.cell_count(*sheet),
                            column_types,
                        }
                    })
                    .collect();
                Ok(Response::Document(DocumentSummary {
                    id: document.id(),
                    name: document.name().to_string(),
                    branch: branch_name,
                    branches,
                    head: document.head(),
                    event_count: document.event_count(),
                    digest: document.digest(),
                    sheets,
                    checks: document.checks().len(),
                    watches: document.watches().len(),
                    load: store.load_report(branch).cloned(),
                }))
            }
            Request::Cells {
                path,
                branch,
                sheet,
                start,
                limit,
            } => {
                let limit = limit.unwrap_or(DEFAULT_CELL_PAGE);
                if limit == 0 || limit > MAX_CELL_PAGE {
                    return Err(ServiceError::new(
                        "invalid_limit",
                        format!("limit must be between 1 and {MAX_CELL_PAGE}"),
                    ));
                }
                let store = self.store(&path)?;
                let (_, branch) = Self::branch(store, branch.as_deref())?;
                let document = store.document(branch)?;
                let sheet = Self::sheet(document, &sheet)?;
                let rows = document.rows(sheet).unwrap_or(&[]);
                let columns = document.columns(sheet).unwrap_or(&[]);
                let mut all = Vec::new();
                for row in rows {
                    for column in columns {
                        let cell = CellRef {
                            sheet,
                            row: *row,
                            column: *column,
                        };
                        if document.cell(cell).is_some() {
                            all.push(cell);
                        }
                    }
                }
                let total = all.len();
                let page: Vec<CellReport> = all
                    .iter()
                    .skip(start)
                    .take(limit)
                    .map(|cell| CellReport {
                        cell: *cell,
                        a1: document.project_a1(*cell),
                        value: document.value(*cell),
                        state: document.cell(*cell).cloned(),
                    })
                    .collect();
                let next = (start + page.len() < total).then_some(start + page.len());
                Ok(Response::Cells(CellPage {
                    sheet,
                    start,
                    total,
                    cells: page,
                    next,
                }))
            }
            Request::GridPage {
                path,
                branch,
                sheet,
                row_start,
                column_start,
                rows: requested_rows,
                columns: requested_columns,
            } => {
                let page_cells = requested_rows.checked_mul(requested_columns);
                if requested_rows == 0
                    || requested_columns == 0
                    || page_cells.is_none_or(|count| count > MAX_GRID_PAGE_CELLS)
                {
                    return Err(ServiceError::new(
                        "invalid_grid_page",
                        format!(
                            "rows and columns must be positive and cover at most {MAX_GRID_PAGE_CELLS} cells"
                        ),
                    ));
                }
                let store = self.store(&path)?;
                let (_, branch) = Self::branch(store, branch.as_deref())?;
                let document = store.document(branch)?;
                let sheet = Self::sheet(document, &sheet)?;
                let all_rows = document.rows(sheet).unwrap_or(&[]);
                let all_columns = document.columns(sheet).unwrap_or(&[]);
                let rows = requested_rows.min(all_rows.len().saturating_sub(row_start));
                let columns = requested_columns.min(all_columns.len().saturating_sub(column_start));
                let mut cells = Vec::new();
                for (row_offset, row) in all_rows.iter().skip(row_start).take(rows).enumerate() {
                    for (column_offset, column) in all_columns
                        .iter()
                        .skip(column_start)
                        .take(columns)
                        .enumerate()
                    {
                        let cell = CellRef {
                            sheet,
                            row: *row,
                            column: *column,
                        };
                        if let Some(state) = document.cell(cell) {
                            let formula = match &state.input {
                                CellInput::Formula { formula } => Some(formula.source.clone()),
                                CellInput::Value { .. } => None,
                            };
                            cells.push(GridCell {
                                row: row_start + row_offset,
                                column: column_start + column_offset,
                                a1: document.project_a1(cell),
                                value: document.value(cell),
                                formula,
                            });
                        }
                    }
                }
                Ok(Response::GridPage(GridPage {
                    sheet,
                    row_start,
                    column_start,
                    rows,
                    columns,
                    cells,
                }))
            }
            Request::Cell {
                path,
                branch,
                sheet,
                a1,
            } => {
                let store = self.store(&path)?;
                let (_, branch) = Self::branch(store, branch.as_deref())?;
                let document = store.document(branch)?;
                let sheet = Self::sheet(document, &sheet)?;
                let cell = document.resolve_a1(sheet, &a1)?;
                Ok(Response::Cell(CellReport {
                    cell,
                    a1: document.project_a1(cell),
                    value: document.value(cell),
                    state: document.cell(cell).cloned(),
                }))
            }
            Request::Lineage {
                path,
                branch,
                sheet,
                a1,
            } => {
                let store = self.store(&path)?;
                let (_, branch) = Self::branch(store, branch.as_deref())?;
                let document = store.document(branch)?;
                let sheet = Self::sheet(document, &sheet)?;
                let cell = document.resolve_a1(sheet, &a1)?;
                Ok(Response::Lineage(document.lineage(cell)))
            }
            Request::Append {
                path,
                branch,
                actor,
                command,
            } => {
                check_actor(&actor)?;
                let store = self.store(&path)?;
                let (branch_name, branch) = Self::branch(store, branch.as_deref())?;
                if actor.kind == ActorKind::Agent && branch_name == MAIN_BRANCH {
                    return Err(ServiceError::new(
                        "agent_on_main",
                        "agents append on their own branches; main changes through a human-approved merge",
                    ));
                }
                let event = store.append(branch, actor, now, command)?;
                Ok(Response::Appended(event))
            }
            Request::Branch {
                path,
                name,
                from,
                actor,
            } => {
                check_actor(&actor)?;
                let store = self.store(&path)?;
                let (_, from) = Self::branch(store, from.as_deref())?;
                let id = store.create_branch(from, &name, actor, now)?;
                Ok(Response::Branched {
                    branch: name,
                    id: id.to_string(),
                })
            }
            Request::Check { path, branch } => {
                let store = self.store(&path)?;
                let (_, branch) = Self::branch(store, branch.as_deref())?;
                let results = store.check(branch)?;
                let passed = !results.iter().any(|result| {
                    result.severity == omasheets_core::Severity::Error && !result.passed
                });
                Ok(Response::Checked { passed, results })
            }
            Request::Diff {
                path,
                source,
                target,
            } => {
                let store = self.store(&path)?;
                let (_, source) = Self::branch(store, Some(&source))?;
                let (_, target) = Self::branch(store, target.as_deref())?;
                Ok(Response::Diff(store.diff(source, target)?))
            }
            Request::Merge {
                path,
                source,
                target,
                approver,
            } => {
                check_actor(&approver)?;
                if approver.kind != ActorKind::Human {
                    return Err(ServiceError::new(
                        "unauthorized",
                        "only a human may approve a merge",
                    ));
                }
                let store = self.store(&path)?;
                let (_, source) = Self::branch(store, Some(&source))?;
                let (_, target) = Self::branch(store, target.as_deref())?;
                Ok(Response::Merged(
                    store.merge(source, target, approver, now)?,
                ))
            }
            Request::ExportCsv {
                path,
                branch,
                sheet,
                output,
            } => {
                let store = self.store(&path)?;
                let (branch_name, branch) = Self::branch(store, branch.as_deref())?;
                let document = store.document(branch)?;
                let sheet = Self::sheet(document, &sheet)?;
                let sheet_name = document.sheet_name(sheet).unwrap_or("").to_string();
                let rows = document.rows(sheet).map_or(0, <[_]>::len);
                let columns = document.columns(sheet).map_or(0, <[_]>::len);
                let document_digest = document.digest();
                let (output, stats) = export_csv(document, sheet, &output)?;
                Ok(Response::ExportedCsv(CsvExportManifest {
                    format: "csv-rfc4180".into(),
                    output,
                    branch: branch_name,
                    sheet,
                    sheet_name,
                    rows,
                    columns,
                    document_digest,
                    formula_cells: stats.formula_cells,
                    potential_formula_injection_cells: stats.potential_formula_injection_cells,
                    limitations: vec![
                        "formula_source_omitted".into(),
                        "styles_tables_checks_lineage_omitted".into(),
                        "potential_formula_injection_cells_are_not_rewritten".into(),
                    ],
                }))
            }
            Request::ExportXlsx {
                path,
                branch,
                output,
            } => {
                let store = self.store(&path)?;
                let (branch_name, branch) = Self::branch(store, branch.as_deref())?;
                let document = store.document(branch)?;
                let document_digest = document.digest();
                let (output, sheets, stats) = export_xlsx(document, &output)?;
                Ok(Response::ExportedXlsx(XlsxExportManifest {
                    format: "xlsx-2007".into(),
                    output,
                    branch: branch_name,
                    document_digest,
                    sheets,
                    formula_cells: stats.formula_cells,
                    formula_cells_preserved: stats.formula_cells_preserved,
                    formula_cells_flattened: stats.formula_cells_flattened,
                    limitations: vec![
                        "styles_and_number_formats_omitted".into(),
                        "tables_checks_watches_lineage_and_branch_history_omitted".into(),
                        "formulas_with_table_or_stale_positional_bindings_flattened".into(),
                        "date_serials_exported_as_unformatted_1900_system_numbers".into(),
                    ],
                }))
            }
            Request::ExportParquet {
                path,
                branch,
                sheet,
                output,
            } => {
                let store = self.store(&path)?;
                let (branch_name, branch) = Self::branch(store, branch.as_deref())?;
                let document = store.document(branch)?;
                let sheet = Self::sheet(document, &sheet)?;
                let sheet_name = document.sheet_name(sheet).unwrap_or("").to_string();
                let rows = document.rows(sheet).map_or(0, <[_]>::len);
                let document_digest = document.digest();
                let (output, columns, stats) = export_parquet(document, sheet, &output)?;
                Ok(Response::ExportedParquet(ParquetExportManifest {
                    format: "parquet-arrow-58.4".into(),
                    output,
                    branch: branch_name,
                    sheet,
                    sheet_name,
                    rows,
                    columns,
                    document_digest,
                    formula_cells: stats.formula_cells,
                    error_cells_as_null: stats.error_cells_as_null,
                    limitations: vec![
                        "one_sheet_per_file".into(),
                        "formula_source_omitted".into(),
                        "error_cells_exported_as_null".into(),
                        "date_serials_are_1900_system_float64_values".into(),
                        "styles_tables_checks_watches_lineage_and_branch_history_omitted".into(),
                    ],
                }))
            }
            Request::ImportXlsx {
                source,
                output,
                actor,
                name,
            } => {
                check_actor(&actor)?;
                if actor.kind != ActorKind::Human {
                    return Err(ServiceError::new(
                        "unauthorized",
                        "a human must authorize a native workbook import",
                    ));
                }
                let key = canonical(&output)?;
                if self.stores.contains_key(&key) {
                    return Err(ServiceError::new(
                        "output_exists",
                        "native output is already open",
                    ));
                }
                let (store, manifest) = import_native_xlsx(&source, &key, actor, name, now)?;
                self.stores.insert(key, store);
                Ok(Response::ImportedXlsx(manifest))
            }
            Request::Snapshot { path, branch } => {
                let store = self.store(&path)?;
                let (_, branch) = Self::branch(store, branch.as_deref())?;
                store.write_snapshot(branch)?;
                let digest = store.document(branch)?.digest();
                Ok(Response::Snapshot { digest })
            }
        }
    }

    /// Closes every open document, writing snapshots.
    pub fn close_all(&mut self) -> Result<(), ServiceError> {
        let paths: Vec<PathBuf> = self.stores.keys().cloned().collect();
        for path in paths {
            if let Some(store) = self.stores.remove(&path) {
                store.close()?;
            }
        }
        Ok(())
    }
}

impl From<omasheets_core::ApplyError> for ServiceError {
    fn from(error: omasheets_core::ApplyError) -> Self {
        Self::new("rejected", error.to_string())
    }
}
