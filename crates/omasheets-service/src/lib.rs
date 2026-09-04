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

use omasheets_core::{
    Actor, ActorKind, CellInput, CellRef, CellState, CellValue, CheckResult, ColumnId, ColumnType,
    Command, DocumentId, Event, InferredColumnType, Lineage, ObjectId, SheetId,
};
use omasheets_store::{BranchDiff, LoadReport, MergeReport, Store, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Largest cell page one `cells` request returns.
pub const MAX_CELL_PAGE: usize = 10_000;
/// Largest rectangular grid page one `grid_page` request may inspect.
pub const MAX_GRID_PAGE_CELLS: usize = 10_000;
/// Largest native CSV projection, checked before the output file is created.
pub const MAX_CSV_EXPORT_CELLS: usize = 10_000_000;
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
    let (temporary_path, mut file) = temporary.ok_or_else(|| {
        ServiceError::new("export_io", "could not allocate a temporary CSV file")
    })?;

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
    if text.contains(|character| matches!(character, ',' | '"' | '\r' | '\n')) {
        writer.write_all(b"\"")?;
        writer.write_all(text.replace('"', "\"\"").as_bytes())?;
        writer.write_all(b"\"")
    } else {
        writer.write_all(text.as_bytes())
    }
}

fn export_io_error(error: io::Error) -> ServiceError {
    ServiceError::new("export_io", error.to_string())
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
                let columns = requested_columns
                    .min(all_columns.len().saturating_sub(column_start));
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
                    potential_formula_injection_cells: stats
                        .potential_formula_injection_cells,
                    limitations: vec![
                        "formula_source_omitted".into(),
                        "styles_tables_checks_lineage_omitted".into(),
                        "potential_formula_injection_cells_are_not_rewritten".into(),
                    ],
                }))
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
