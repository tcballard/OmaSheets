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
    Actor, ActorKind, CellRef, CellState, CellValue, CheckResult, Command, DocumentId, Event,
    Lineage, ObjectId, SheetId,
};
use omasheets_store::{BranchDiff, LoadReport, MergeReport, Store, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Largest cell page one `cells` request returns.
pub const MAX_CELL_PAGE: usize = 10_000;
const DEFAULT_CELL_PAGE: usize = 1_000;
const MAX_ACTOR_CHARS: usize = 128;
const MAIN_BRANCH: &str = "main";

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
        if let Some(id) = ObjectId::parse(sheet).map(SheetId) {
            if document.sheets().contains(&id) {
                return Ok(id);
            }
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
                        let columns = document.columns(*sheet).map_or(0, <[_]>::len);
                        SheetSummary {
                            id: *sheet,
                            name: document.sheet_name(*sheet).unwrap_or("").to_string(),
                            rows,
                            columns,
                            cells: document.cell_count(*sheet),
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
