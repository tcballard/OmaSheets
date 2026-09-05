//! Single-file SQLite store for OmaSheets documents.
//!
//! One `.omasheets` file holds the append-only event log, which is the only
//! source of truth, plus schema metadata, branch heads and bounded
//! materialised snapshots that are caches: a snapshot is used only when the
//! document rebuilt from it reproduces the digest recorded beside it, and
//! otherwise the log is replayed. The file runs in WAL mode with full
//! synchronous writes while open, so an event that `append` returned is on
//! disk before the caller sees it.
//!
//! Every mutation goes through the event core (`omasheets-core`): the store
//! never edits state rows, it only appends events and refreshes caches.
//! Merging a branch is gated: the actor must be human, the source branch's
//! error-severity checks must pass, and operation-level conflicts against the
//! target must be empty. Agents can propose on their own branch; they cannot
//! merge or publish.

use omasheets_core::{
    Actor, ActorKind, ApplyError, BranchId, CellRef, CellValue, CheckResult, Command, Document,
    Event, EventId, Lineage, Operation, Severity, Snapshot, Touch, WatchId,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: i64 = 1;
pub const DEFAULT_SNAPSHOT_INTERVAL: u64 = 100;
pub const MAX_MERGE_EVENTS: usize = 10_000;

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Apply(ApplyError),
    NotADocumentStore(PathBuf),
    UnsupportedSchema(i64),
    UnknownBranch(String),
    DuplicateBranch(String),
    UnknownEvent(EventId),
    CorruptLog(String),
    Unauthorized(String),
    ChecksFailed(Vec<CheckResult>),
    Conflicts(Vec<Touch>),
    NothingToMerge,
    TooManyEvents(usize),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "sqlite: {error}"),
            Self::Json(error) => write!(formatter, "json: {error}"),
            Self::Apply(error) => write!(formatter, "event rejected: {error}"),
            Self::NotADocumentStore(path) => {
                write!(formatter, "{} is not an OmaSheets store", path.display())
            }
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported store schema {version}")
            }
            Self::UnknownBranch(name) => write!(formatter, "unknown branch {name}"),
            Self::DuplicateBranch(name) => write!(formatter, "branch {name} already exists"),
            Self::UnknownEvent(id) => write!(formatter, "unknown event {id}"),
            Self::CorruptLog(detail) => write!(formatter, "corrupt event log: {detail}"),
            Self::Unauthorized(detail) => write!(formatter, "not authorized: {detail}"),
            Self::ChecksFailed(results) => {
                write!(
                    formatter,
                    "{} error-severity check(s) failed",
                    results.len()
                )
            }
            Self::Conflicts(touches) => {
                write!(formatter, "{} conflicting object(s)", touches.len())
            }
            Self::NothingToMerge => write!(formatter, "the source branch has no new events"),
            Self::TooManyEvents(count) => write!(formatter, "{count} events exceed the bound"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<ApplyError> for StoreError {
    fn from(error: ApplyError) -> Self {
        Self::Apply(error)
    }
}

/// How a branch was loaded, for evidence and tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadPath {
    FullReplay,
    SnapshotPlusTail,
    SnapshotRejected,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoadReport {
    pub branch: String,
    pub path: LoadPath,
    pub events_replayed: u64,
    pub event_count: u64,
    pub head: Option<EventId>,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WatchedChange {
    pub watch: WatchId,
    pub name: String,
    pub cell: CellRef,
    pub before: CellValue,
    pub after: CellValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperationSummary {
    pub event: EventId,
    pub actor: Actor,
    pub timestamp: i64,
    pub operation: String,
    pub touches: Vec<Touch>,
}

/// A semantic comparison of two branches since their common base.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchDiff {
    pub source: String,
    pub target: String,
    pub base: EventId,
    pub source_operations: Vec<OperationSummary>,
    pub target_operations: Vec<OperationSummary>,
    pub watched_changes: Vec<WatchedChange>,
    pub conflicts: Vec<Touch>,
    pub source_checks: Vec<CheckResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MergeReport {
    pub source: String,
    pub target: String,
    pub replayed: Vec<EventId>,
    pub record: EventId,
    pub digest: String,
}

struct StoredEvent {
    seq: i64,
    event: Event,
}

pub struct Store {
    connection: Connection,
    path: PathBuf,
    snapshot_interval: u64,
    documents: BTreeMap<BranchId, Document>,
    load_reports: BTreeMap<BranchId, LoadReport>,
}

impl fmt::Debug for Store {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Store")
            .field("path", &self.path)
            .field(
                "cached_branches",
                &self.documents.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Store {
    /// Creates a new store file holding a freshly created document.
    pub fn create(
        path: impl AsRef<Path>,
        document: omasheets_core::DocumentId,
        name: &str,
        actor: Actor,
        timestamp: i64,
    ) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        if path.exists() {
            return Err(StoreError::CorruptLog(format!(
                "refusing to replace existing file {}",
                path.display()
            )));
        }
        let connection = Connection::open(&path)?;
        configure(&connection)?;
        install_schema(&connection)?;
        let (state, event) = Document::create(document, name, actor, timestamp)?;
        let mut store = Self {
            connection,
            path,
            snapshot_interval: DEFAULT_SNAPSHOT_INTERVAL,
            documents: BTreeMap::new(),
            load_reports: BTreeMap::new(),
        };
        let transaction = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_event(&transaction, &event)?;
        transaction.execute(
            "INSERT INTO branches (id, name, parent, base_event, head) VALUES (?1, ?2, NULL, NULL, ?3)",
            params![state.branch().to_string(), "main", event.id.to_string()],
        )?;
        transaction.commit()?;
        store.documents.insert(state.branch(), state);
        Ok(store)
    }

    /// Opens an existing store, migrating older schemas forward.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        if !path.is_file() {
            return Err(StoreError::NotADocumentStore(path));
        }
        let connection = Connection::open(&path)?;
        configure(&connection)?;
        migrate(&connection, &path)?;
        Ok(Self {
            connection,
            path,
            snapshot_interval: DEFAULT_SNAPSHOT_INTERVAL,
            documents: BTreeMap::new(),
            load_reports: BTreeMap::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn set_snapshot_interval(&mut self, interval: u64) {
        self.snapshot_interval = interval.max(1);
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        read_schema_version(&self.connection)
    }

    pub fn branch_names(&self) -> Result<Vec<String>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT name FROM branches ORDER BY rowid")?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(names)
    }

    pub fn branch_id(&self, name: &str) -> Result<BranchId, StoreError> {
        let id: Option<String> = self
            .connection
            .query_row(
                "SELECT id FROM branches WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        id.and_then(|text| omasheets_core::ObjectId::parse(&text))
            .map(BranchId)
            .ok_or_else(|| StoreError::UnknownBranch(name.into()))
    }

    fn branch_name(&self, branch: BranchId) -> Result<String, StoreError> {
        self.connection
            .query_row(
                "SELECT name FROM branches WHERE id = ?1",
                params![branch.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::UnknownBranch(branch.to_string()))
    }

    /// The document at a branch's head, loaded from a verified snapshot plus
    /// the tail of the log when possible, otherwise by full replay.
    pub fn document(&mut self, branch: BranchId) -> Result<&Document, StoreError> {
        if !self.documents.contains_key(&branch) {
            let (document, report) = self.load(branch)?;
            self.documents.insert(branch, document);
            self.load_reports.insert(branch, report);
        }
        Ok(&self.documents[&branch])
    }

    pub fn load_report(&self, branch: BranchId) -> Option<&LoadReport> {
        self.load_reports.get(&branch)
    }

    /// Discards cached documents so the next access reloads from disk.
    pub fn evict(&mut self) {
        self.documents.clear();
        self.load_reports.clear();
    }

    fn chain(&self, branch: BranchId) -> Result<Vec<StoredEvent>, StoreError> {
        self.chain_after(branch, 0)
    }

    fn chain_after(&self, branch: BranchId, after: i64) -> Result<Vec<StoredEvent>, StoreError> {
        // Walk ancestry: each branch contributes its own events up to the next
        // child's fork point, then the child's events.
        let mut lineage = Vec::new();
        let mut cursor = Some(branch);
        while let Some(current) = cursor {
            let row: Option<(Option<String>, Option<String>)> = self
                .connection
                .query_row(
                    "SELECT parent, base_event FROM branches WHERE id = ?1",
                    params![current.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((parent, base)) = row else {
                return Err(StoreError::UnknownBranch(current.to_string()));
            };
            lineage.push((current, base));
            cursor = parent
                .map(|text| {
                    omasheets_core::ObjectId::parse(&text)
                        .map(BranchId)
                        .ok_or_else(|| StoreError::CorruptLog("bad branch id".into()))
                })
                .transpose()?;
            if lineage.len() > 1_024 {
                return Err(StoreError::CorruptLog("branch ancestry too deep".into()));
            }
        }
        lineage.reverse();
        let mut events = Vec::new();
        // Query only the requested tail, bounded by each child's fork point.
        for (index, (current, _)) in lineage.iter().enumerate() {
            let next_base = lineage.get(index + 1).and_then(|(_, base)| base.as_ref());
            let through: i64 = match next_base {
                Some(base) => self.connection.query_row(
                    "SELECT seq FROM events WHERE id = ?1 AND branch = ?2",
                    params![base, current.to_string()],
                    |row| row.get(0),
                )?,
                None => i64::MAX,
            };
            let mut statement = self.connection.prepare(
                "SELECT seq, canonical FROM events WHERE branch = ?1 AND seq > ?2 AND seq <= ?3 ORDER BY seq",
            )?;
            let rows = statement
                .query_map(params![current.to_string(), after, through], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?;
            for row in rows {
                let (seq, canonical) = row?;
                events.push(StoredEvent {
                    seq,
                    event: serde_json::from_str(&canonical)?,
                });
            }
        }
        Ok(events)
    }

    fn load(&self, branch: BranchId) -> Result<(Document, LoadReport), StoreError> {
        let name = self.branch_name(branch)?;
        let snapshot: Option<(i64, String, String)> = self
            .connection
            .query_row(
                "SELECT seq, digest, payload FROM snapshots WHERE branch = ?1 ORDER BY seq DESC LIMIT 1",
                params![branch.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let mut path = LoadPath::FullReplay;
        if let Some((seq, digest, payload)) = snapshot {
            // Include the checkpoint event itself to verify membership in this
            // branch's ancestry, without parsing the history preceding it.
            let tail = self.chain_after(branch, seq.saturating_sub(1))?;
            let restored = serde_json::from_str::<Snapshot>(&payload)
                .ok()
                .and_then(|snapshot| Document::from_snapshot(&snapshot).ok())
                .filter(|document| document.digest() == digest)
                .filter(|document| {
                    document.branch() == branch
                        && tail.first().is_some_and(|stored| {
                            stored.seq == seq
                                && stored.event.verify()
                                && Some(stored.event.id) == document.head()
                        })
                });
            match restored {
                Some(mut document) => {
                    let mut replayed = 0;
                    for stored in tail.iter().skip(1) {
                        document.apply(&stored.event)?;
                        replayed += 1;
                    }
                    let report = LoadReport {
                        branch: name,
                        path: LoadPath::SnapshotPlusTail,
                        events_replayed: replayed,
                        event_count: document.event_count(),
                        head: document.head(),
                        digest: document.digest(),
                    };
                    return Ok((document, report));
                }
                None => path = LoadPath::SnapshotRejected,
            }
        }
        let events: Vec<Event> = self
            .chain(branch)?
            .into_iter()
            .map(|stored| stored.event)
            .collect();
        if events.is_empty() {
            return Err(StoreError::CorruptLog("branch has no events".into()));
        }
        let document = Document::replay(&events)?;
        let report = LoadReport {
            branch: name,
            path,
            events_replayed: events.len() as u64,
            event_count: document.event_count(),
            head: document.head(),
            digest: document.digest(),
        };
        Ok((document, report))
    }

    /// Resolves and appends one command on `branch`. The event is durable
    /// when this returns; on any error nothing is written.
    pub fn append(
        &mut self,
        branch: BranchId,
        actor: Actor,
        timestamp: i64,
        command: Command,
    ) -> Result<Event, StoreError> {
        self.document(branch)?;
        let document = self.documents.get_mut(&branch).expect("loaded");
        let event = document.command(actor, timestamp, command)?;
        if let Err(error) = self.persist(branch, std::slice::from_ref(&event)) {
            // The in-memory document already advanced; drop it so the next
            // access reloads exactly what is on disk.
            self.documents.remove(&branch);
            self.load_reports.remove(&branch);
            return Err(error);
        }
        // A checkpoint is only a cache. Its failure must not turn a durable
        // append into an apparent failed save that a client might retry.
        let _ = self.maybe_snapshot(branch);
        Ok(event)
    }

    /// Resolves and appends a command sequence in one database transaction.
    /// Resolution happens against a cloned document, so a rejected command or
    /// failed transaction leaves both the durable and cached state unchanged.
    pub fn append_batch(
        &mut self,
        branch: BranchId,
        actor: Actor,
        timestamp: i64,
        commands: Vec<Command>,
    ) -> Result<Vec<Event>, StoreError> {
        if commands.len() == 1 {
            return self
                .append(
                    branch,
                    actor,
                    timestamp,
                    commands.into_iter().next().expect("one command"),
                )
                .map(|event| vec![event]);
        }
        self.document(branch)?;
        let mut staged = self.documents[&branch].clone();
        let defer = commands.iter().all(|command| {
            matches!(
                command,
                Command::SetValue { .. } | Command::SetFormula { .. } | Command::ClearCell { .. }
            )
        });
        if defer {
            staged.begin_bulk();
        }
        let mut events = Vec::with_capacity(commands.len());
        for command in commands {
            events.push(staged.command(actor.clone(), timestamp, command)?);
        }
        if defer {
            staged.end_bulk();
        }
        if events.is_empty() {
            return Ok(events);
        }
        self.persist(branch, &events)?;
        self.documents.insert(branch, staged);
        self.load_reports.remove(&branch);
        let _ = self.maybe_snapshot(branch);
        Ok(events)
    }

    /// Validate and atomically persist already-resolved events on one branch.
    /// Import planning can retain its signed events instead of resolving the
    /// same commands a second time. Validation still replays every event.
    pub fn append_events(
        &mut self,
        branch: BranchId,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }
        let mut staged = self.document(branch)?.clone();
        let mut deferred = false;
        for event in &events {
            let edit = matches!(
                event.operation,
                Operation::SetValue { .. }
                    | Operation::SetFormula { .. }
                    | Operation::ClearCell { .. }
            );
            if deferred && !edit {
                staged.end_bulk();
            }
            if !deferred && edit {
                staged.begin_bulk();
            }
            deferred = edit;
            if event.branch != branch {
                return Err(StoreError::Apply(ApplyError::BranchMismatch));
            }
            staged.apply(event)?;
            if staged.branch() != branch {
                return Err(StoreError::Apply(ApplyError::BranchMismatch));
            }
        }
        if deferred {
            staged.end_bulk();
        }
        self.persist(branch, &events)?;
        self.documents.insert(branch, staged);
        self.load_reports.remove(&branch);
        let _ = self.maybe_snapshot(branch);
        Ok(())
    }

    fn persist(&mut self, branch: BranchId, events: &[Event]) -> Result<(), StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for event in events {
            insert_event(&transaction, event)?;
        }
        if let Some(last) = events.last() {
            let changed = transaction.execute(
                "UPDATE branches SET head = ?1 WHERE id = ?2",
                params![last.id.to_string(), branch.to_string()],
            )?;
            if changed != 1 {
                return Err(StoreError::UnknownBranch(branch.to_string()));
            }
        }
        transaction.commit()?;
        Ok(())
    }

    fn maybe_snapshot(&mut self, branch: BranchId) -> Result<(), StoreError> {
        let count = self.documents[&branch].event_count();
        let checkpoint_count: u64 = self.connection.query_row(
            "SELECT COALESCE(MAX(event_count), 0) FROM snapshots WHERE branch = ?1",
            params![branch.to_string()],
            |row| row.get(0),
        )?;
        if count.saturating_sub(checkpoint_count) >= self.snapshot_interval {
            self.write_snapshot(branch)?;
        }
        Ok(())
    }

    /// Materialises the branch head as a cache entry.
    pub fn write_snapshot(&mut self, branch: BranchId) -> Result<(), StoreError> {
        self.document(branch)?;
        let document = &self.documents[&branch];
        let Some(head) = document.head() else {
            return Ok(());
        };
        let seq: i64 = self.connection.query_row(
            "SELECT seq FROM events WHERE id = ?1",
            params![head.to_string()],
            |row| row.get(0),
        )?;
        let payload = serde_json::to_string(&document.snapshot())?;
        self.connection.execute(
            "INSERT OR REPLACE INTO snapshots (branch, seq, head, event_count, digest, payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                branch.to_string(),
                seq,
                head.to_string(),
                document.event_count() as i64,
                document.digest(),
                payload
            ],
        )?;
        // Snapshots are disposable caches. Keep one per branch instead of
        // retaining a full document copy at every checkpoint indefinitely.
        self.connection.execute(
            "DELETE FROM snapshots WHERE branch = ?1 AND seq < ?2",
            params![branch.to_string(), seq],
        )?;
        Ok(())
    }

    /// Writes snapshots for every cached branch and checkpoints the WAL.
    pub fn close(mut self) -> Result<(), StoreError> {
        let branches: Vec<BranchId> = self.documents.keys().copied().collect();
        for branch in branches {
            self.write_snapshot(branch)?;
        }
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    /// Forks `from` at its head into a new named branch.
    pub fn create_branch(
        &mut self,
        from: BranchId,
        name: &str,
        actor: Actor,
        timestamp: i64,
    ) -> Result<BranchId, StoreError> {
        if self.branch_id(name).is_ok() {
            return Err(StoreError::DuplicateBranch(name.into()));
        }
        self.document(from)?;
        let mut document = self.documents[&from].clone();
        let fork = document.fork(name, actor, timestamp);
        document.apply(&fork)?;
        let branch = document.branch();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_event(&transaction, &fork)?;
        transaction.execute(
            "INSERT INTO branches (id, name, parent, base_event, head) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                branch.to_string(),
                name,
                from.to_string(),
                fork.parent.expect("forks have a parent").to_string(),
                fork.id.to_string()
            ],
        )?;
        transaction.commit()?;
        self.documents.insert(branch, document);
        Ok(branch)
    }

    pub fn check(&mut self, branch: BranchId) -> Result<Vec<CheckResult>, StoreError> {
        Ok(self.document(branch)?.check_results())
    }

    pub fn lineage(
        &mut self,
        branch: BranchId,
        cell: CellRef,
    ) -> Result<Option<Lineage>, StoreError> {
        Ok(self.document(branch)?.lineage(cell))
    }

    fn base_event(&self, source: BranchId) -> Result<EventId, StoreError> {
        let base: Option<String> = self.connection.query_row(
            "SELECT base_event FROM branches WHERE id = ?1",
            params![source.to_string()],
            |row| row.get(0),
        )?;
        base.and_then(|text| EventId::parse(&text))
            .ok_or_else(|| StoreError::UnknownBranch(source.to_string()))
    }

    fn events_since(&self, branch: BranchId, base: EventId) -> Result<Vec<Event>, StoreError> {
        let chain = self.chain(branch)?;
        let position = chain
            .iter()
            .position(|stored| stored.event.id == base)
            .ok_or(StoreError::UnknownEvent(base))?;
        let events: Vec<Event> = chain
            .into_iter()
            .skip(position + 1)
            .map(|stored| stored.event)
            .collect();
        if events.len() > MAX_MERGE_EVENTS {
            return Err(StoreError::TooManyEvents(events.len()));
        }
        Ok(events)
    }

    /// The event on each side after which changes are still unmerged: the
    /// fork point, or, once `source` has been merged into `target`, the last
    /// merged source head and the merge record that carried it.
    fn merge_bases(
        &mut self,
        source: BranchId,
        target: BranchId,
    ) -> Result<(EventId, EventId), StoreError> {
        let fork = self.base_event(source)?;
        let last = self
            .document(target)?
            .merges()
            .iter()
            .rev()
            .find(|record| record.source == source)
            .map(|record| (record.source_head, record.recorded.event));
        Ok(last.unwrap_or((fork, fork)))
    }

    /// Compares `source` with `target` since their last common point:
    /// operations on each side, watched-output value changes, and the
    /// objects both sides touched.
    pub fn diff(&mut self, source: BranchId, target: BranchId) -> Result<BranchDiff, StoreError> {
        let (source_base, target_base) = self.merge_bases(source, target)?;
        let base = source_base;
        let source_events = self.events_since(source, source_base)?;
        let target_events = self.events_since(target, target_base)?;
        let summarise = |events: &[Event]| -> Vec<OperationSummary> {
            events
                .iter()
                .filter(|event| !matches!(event.operation, Operation::CreateBranch { .. }))
                .map(|event| OperationSummary {
                    event: event.id,
                    actor: event.actor.clone(),
                    timestamp: event.timestamp,
                    operation: operation_label(&event.operation),
                    touches: event.operation.touches(),
                })
                .collect()
        };
        let conflicts = conflicts_between(&source_events, &target_events);
        self.document(source)?;
        self.document(target)?;
        let source_document = &self.documents[&source];
        let target_document = &self.documents[&target];
        let mut watched_changes = Vec::new();
        for (watch, record) in target_document.watches() {
            let before = target_document.value(record.cell);
            let after = source_document.value(record.cell);
            if before != after {
                watched_changes.push(WatchedChange {
                    watch: *watch,
                    name: record.name.clone(),
                    cell: record.cell,
                    before,
                    after,
                });
            }
        }
        Ok(BranchDiff {
            source: self.branch_name(source)?,
            target: self.branch_name(target)?,
            base,
            source_operations: summarise(&source_events),
            target_operations: summarise(&target_events),
            watched_changes,
            conflicts,
            source_checks: source_document.check_results(),
        })
    }

    /// Replays the source branch's events since its fork point onto the
    /// target, as new events attributed to their original actors, then
    /// records the merge. Gated: `approver` must be human, every
    /// error-severity check on the source must pass, and there must be no
    /// operation-level conflicts. All or nothing.
    pub fn merge(
        &mut self,
        source: BranchId,
        target: BranchId,
        approver: Actor,
        timestamp: i64,
    ) -> Result<MergeReport, StoreError> {
        if approver.kind != ActorKind::Human {
            return Err(StoreError::Unauthorized(format!(
                "merge requires a human approver, not {:?}",
                approver.kind
            )));
        }
        let diff = self.diff(source, target)?;
        let failed: Vec<CheckResult> = diff
            .source_checks
            .iter()
            .filter(|result| result.severity == Severity::Error && !result.passed)
            .cloned()
            .collect();
        if !failed.is_empty() {
            return Err(StoreError::ChecksFailed(failed));
        }
        if !diff.conflicts.is_empty() {
            return Err(StoreError::Conflicts(diff.conflicts));
        }
        let base = diff.base;
        let source_events: Vec<Event> = self
            .events_since(source, base)?
            .into_iter()
            .filter(|event| !matches!(event.operation, Operation::CreateBranch { .. }))
            .collect();
        if source_events.is_empty() {
            return Err(StoreError::NothingToMerge);
        }
        let source_head = self.documents[&source]
            .head()
            .ok_or(StoreError::NothingToMerge)?;
        self.document(target)?;
        let mut candidate = self.documents[&target].clone();
        let mut replayed = Vec::with_capacity(source_events.len());
        for original in &source_events {
            let event = Event::new(
                candidate.head(),
                candidate.branch(),
                original.actor.clone(),
                original.timestamp,
                original.operation.clone(),
            );
            candidate.apply(&event)?;
            replayed.push(event);
        }
        let record = Event::new(
            candidate.head(),
            candidate.branch(),
            approver,
            timestamp,
            Operation::RecordMerge {
                source,
                source_head,
                replayed: replayed.iter().map(|event| event.id).collect(),
            },
        );
        candidate.apply(&record)?;
        let mut all = replayed;
        all.push(record.clone());
        self.persist(target, &all)?;
        let digest = candidate.digest();
        self.documents.insert(target, candidate);
        self.write_snapshot(target)?;
        Ok(MergeReport {
            source: self.branch_name(source)?,
            target: self.branch_name(target)?,
            replayed: all[..all.len() - 1].iter().map(|event| event.id).collect(),
            record: record.id,
            digest,
        })
    }
}

fn conflicts_between(source: &[Event], target: &[Event]) -> Vec<Touch> {
    let touched = |events: &[Event]| -> BTreeSet<Touch> {
        events
            .iter()
            .flat_map(|event| event.operation.touches())
            .filter(|touch| !matches!(touch, Touch::Document))
            .collect()
    };
    touched(source)
        .intersection(&touched(target))
        .copied()
        .collect()
}

pub fn operation_label(operation: &Operation) -> String {
    let json = serde_json::to_value(operation).expect("operations serialise");
    json.get("op")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown")
        .to_string()
}

fn configure(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;\n         PRAGMA synchronous = FULL;\n         PRAGMA foreign_keys = ON;",
    )?;
    Ok(())
}

fn install_schema(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);\n         CREATE TABLE events (\n             seq INTEGER PRIMARY KEY,\n             id TEXT NOT NULL UNIQUE,\n             parent TEXT,\n             branch TEXT NOT NULL,\n             actor_kind TEXT NOT NULL,\n             actor_id TEXT NOT NULL,\n             timestamp INTEGER NOT NULL,\n             canonical TEXT NOT NULL\n         );\n         CREATE INDEX events_branch_seq ON events (branch, seq);\n         CREATE TABLE branches (\n             id TEXT PRIMARY KEY,\n             name TEXT NOT NULL UNIQUE,\n             parent TEXT,\n             base_event TEXT,\n             head TEXT\n         );\n         CREATE TABLE snapshots (\n             branch TEXT NOT NULL,\n             seq INTEGER NOT NULL,\n             head TEXT NOT NULL,\n             event_count INTEGER NOT NULL,\n             digest TEXT NOT NULL,\n             payload TEXT NOT NULL,\n             PRIMARY KEY (branch, seq)\n         );",
    )?;
    connection.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', ?1), ('format', 'omasheets-store')",
        params![SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

fn read_schema_version(connection: &Connection) -> Result<i64, StoreError> {
    let value: Option<String> = connection
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    value
        .and_then(|text| text.parse().ok())
        .ok_or(StoreError::UnsupportedSchema(-1))
}

/// Brings an older file forward. Version 0 is the pre-release layout that
/// lacked the `(branch, seq)` index and the format marker.
fn migrate(connection: &Connection, path: &Path) -> Result<(), StoreError> {
    let has_meta: bool = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |row| row.get::<_, i64>(0).map(|count| count > 0),
    )?;
    if !has_meta {
        return Err(StoreError::NotADocumentStore(path.to_path_buf()));
    }
    let mut version = read_schema_version(connection).unwrap_or(0);
    while version < SCHEMA_VERSION {
        match version {
            0 => {
                connection.execute_batch(
                    "CREATE INDEX IF NOT EXISTS events_branch_seq ON events (branch, seq);\n                     INSERT OR REPLACE INTO meta (key, value) VALUES ('format', 'omasheets-store');\n                     INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', '1');",
                )?;
            }
            other => return Err(StoreError::UnsupportedSchema(other)),
        }
        version += 1;
    }
    if version > SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema(version));
    }
    Ok(())
}

fn insert_event(connection: &Connection, event: &Event) -> Result<(), StoreError> {
    if !event.verify() {
        return Err(StoreError::CorruptLog(
            "event id does not match its bytes".into(),
        ));
    }
    connection.execute(
        "INSERT INTO events (id, parent, branch, actor_kind, actor_id, timestamp, canonical) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            event.id.to_string(),
            event.parent.map(|parent| parent.to_string()),
            event.branch.to_string(),
            serde_json::to_value(event.actor.kind)?
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            event.actor.id,
            event.timestamp,
            event.canonical_json()
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use omasheets_core::{DocumentId, Literal, ObjectId, SheetId};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn human() -> Actor {
        Actor::new(ActorKind::Human, "tom")
    }

    fn agent() -> Actor {
        Actor::new(ActorKind::Agent, "planner")
    }

    fn temp_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "omasheets-store-{label}-{}-{nonce}.omasheets",
            std::process::id()
        ))
    }

    fn cleanup(path: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    struct Setup {
        store: Store,
        main: BranchId,
        sheet: SheetId,
        clock: i64,
    }

    impl Setup {
        fn new(path: &Path) -> Self {
            let mut store = Store::create(
                path,
                DocumentId(ObjectId::from_seed("store-test")),
                "Budget",
                human(),
                1_000,
            )
            .unwrap();
            let main = store.branch_id("main").unwrap();
            let event = store
                .append(
                    main,
                    human(),
                    1_001,
                    Command::AddSheet {
                        name: "Summary".into(),
                    },
                )
                .unwrap();
            let Operation::AddSheet { sheet, .. } = event.operation else {
                unreachable!()
            };
            let mut setup = Self {
                store,
                main,
                sheet,
                clock: 1_001,
            };
            setup.run(
                main,
                human(),
                Command::AddColumns {
                    sheet,
                    count: 2,
                    at: 0,
                },
            );
            setup.run(
                main,
                human(),
                Command::AddRows {
                    sheet,
                    count: 4,
                    at: 0,
                    table: None,
                },
            );
            setup
        }

        fn run(&mut self, branch: BranchId, actor: Actor, command: Command) -> Event {
            self.clock += 1;
            self.store
                .append(branch, actor, self.clock, command)
                .unwrap()
        }

        fn set(&mut self, branch: BranchId, a1: &str, value: f64) -> Event {
            let sheet = self.sheet;
            self.run(
                branch,
                human(),
                Command::SetValue {
                    sheet,
                    a1: a1.into(),
                    value: Literal::Number(value),
                },
            )
        }

        fn value(&mut self, branch: BranchId, a1: &str) -> CellValue {
            let sheet = self.sheet;
            let document = self.store.document(branch).unwrap();
            let cell = document.resolve_a1(sheet, a1).unwrap();
            document.value(cell)
        }
    }

    #[test]
    fn single_batch_persistence_failure_reloads_and_checkpoint_failure_keeps_saved_event() {
        let path = temp_path("single-batch-failure");
        let mut setup = Setup::new(&path);
        let main = setup.main;
        let before = setup.store.document(main).unwrap().digest();
        let command = Command::SetValue {
            sheet: setup.sheet,
            a1: "A1".into(),
            value: Literal::Number(42.0),
        };
        setup.store.connection.execute_batch("CREATE TRIGGER refuse_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
        assert!(
            setup
                .store
                .append_batch(main, human(), 2_000, vec![command.clone()])
                .is_err()
        );
        assert_eq!(setup.store.document(main).unwrap().digest(), before);
        setup.store.connection.execute_batch("DROP TRIGGER refuse_event; CREATE TRIGGER refuse_snapshot BEFORE INSERT ON snapshots BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
        setup.store.set_snapshot_interval(1);
        let events = setup
            .store
            .append_batch(main, human(), 2_001, vec![command])
            .unwrap();
        setup.store.evict();
        assert_eq!(
            setup.store.document(main).unwrap().head(),
            Some(events[0].id)
        );
        assert_eq!(setup.value(main, "A1"), CellValue::Number(42.0));
        cleanup(&path);
    }

    #[test]
    fn deferred_edit_batch_and_snapshot_restore_preserve_overlapping_formula_results() {
        let path = temp_path("deferred-calculation");
        let mut setup = Setup::new(&path);
        let main = setup.main;
        let sheet = setup.sheet;
        setup.run(
            main,
            human(),
            Command::AddColumns {
                sheet,
                count: 1,
                at: 2,
            },
        );
        setup.run(
            main,
            human(),
            Command::SetFormula {
                sheet,
                a1: "C1".into(),
                source: "=SUM(A1:B2)".into(),
            },
        );
        setup.run(
            main,
            human(),
            Command::SetFormula {
                sheet,
                a1: "C2".into(),
                source: "=C1*2".into(),
            },
        );
        let mut commands: Vec<_> = ["A1", "B1", "A2", "B2"]
            .into_iter()
            .enumerate()
            .map(|(i, a1)| Command::SetValue {
                sheet,
                a1: a1.into(),
                value: Literal::Number((i + 1) as f64),
            })
            .collect();
        commands.push(Command::ClearCell {
            sheet,
            a1: "B2".into(),
        });
        setup
            .store
            .append_batch(main, human(), 3_000, commands)
            .unwrap();
        assert_eq!(setup.value(main, "C2"), CellValue::Number(12.0));
        let expected = setup.store.document(main).unwrap().digest();
        setup.store.write_snapshot(main).unwrap();
        setup.store.evict();
        assert_eq!(setup.store.document(main).unwrap().digest(), expected);
        assert_eq!(setup.value(main, "C2"), CellValue::Number(12.0));
        setup
            .store
            .connection
            .execute("DELETE FROM snapshots", [])
            .unwrap();
        setup.store.evict();
        assert_eq!(setup.store.document(main).unwrap().digest(), expected);
        cleanup(&path);
    }

    #[test]
    fn resolved_event_import_validates_every_event_before_persisting() {
        let path = temp_path("resolved-events");
        let mut setup = Setup::new(&path);
        let main = setup.main;
        let before = setup.store.document(main).unwrap().digest();
        let mut planning = setup.store.document(main).unwrap().clone();
        let events = vec![
            planning
                .command(
                    human(),
                    3_000,
                    Command::SetValue {
                        sheet: setup.sheet,
                        a1: "A1".into(),
                        value: Literal::Number(4.0),
                    },
                )
                .unwrap(),
            planning
                .command(
                    human(),
                    3_001,
                    Command::SetFormula {
                        sheet: setup.sheet,
                        a1: "B1".into(),
                        source: "=A1*2".into(),
                    },
                )
                .unwrap(),
        ];
        let mut corrupt = events.clone();
        corrupt[1].timestamp += 1;
        assert!(matches!(
            setup.store.append_events(main, corrupt),
            Err(StoreError::Apply(ApplyError::InvalidEventId))
        ));
        assert_eq!(setup.store.document(main).unwrap().digest(), before);
        setup.store.evict();
        assert_eq!(setup.store.document(main).unwrap().digest(), before);
        setup.store.append_events(main, events).unwrap();
        assert_eq!(
            setup.store.document(main).unwrap().digest(),
            planning.digest()
        );
        assert_eq!(setup.value(main, "B1"), CellValue::Number(8.0));
        setup.store.evict();
        assert_eq!(
            setup.store.document(main).unwrap().digest(),
            planning.digest()
        );
        cleanup(&path);
    }

    #[test]
    fn batch_append_is_atomic_and_replays_identically() {
        let path = temp_path("batch");
        let (main, expected) = {
            let mut setup = Setup::new(&path);
            let before = setup.store.document(setup.main).unwrap().digest();
            let rejected = setup.store.append_batch(
                setup.main,
                human(),
                2_000,
                vec![
                    Command::SetValue {
                        sheet: setup.sheet,
                        a1: "A1".into(),
                        value: Literal::Number(1.0),
                    },
                    Command::SetValue {
                        sheet: setup.sheet,
                        a1: "A99".into(),
                        value: Literal::Number(99.0),
                    },
                ],
            );
            assert!(matches!(rejected, Err(StoreError::Apply(_))));
            assert_eq!(setup.store.document(setup.main).unwrap().digest(), before);

            let events = setup
                .store
                .append_batch(
                    setup.main,
                    human(),
                    2_001,
                    vec![
                        Command::SetValue {
                            sheet: setup.sheet,
                            a1: "A1".into(),
                            value: Literal::Number(1.0),
                        },
                        Command::SetValue {
                            sheet: setup.sheet,
                            a1: "B1".into(),
                            value: Literal::Number(2.0),
                        },
                    ],
                )
                .unwrap();
            assert_eq!(events.len(), 2);
            let expected = setup.store.document(setup.main).unwrap().digest();
            (setup.main, expected)
        };
        let mut reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.document(main).unwrap().digest(), expected);
        drop(reopened);
        cleanup(&path);
    }

    #[test]
    fn committed_events_survive_dropping_the_store_without_close() {
        let path = temp_path("durable");
        let (main, digest, count) = {
            let mut setup = Setup::new(&path);
            let main = setup.main;
            setup.set(main, "A1", 2.0);
            let sheet = setup.sheet;
            setup.run(
                main,
                human(),
                Command::SetFormula {
                    sheet,
                    a1: "A2".into(),
                    source: "=A1*3".into(),
                },
            );
            let document = setup.store.document(main).unwrap();
            (main, document.digest(), document.event_count())
            // No close(): the WAL holds the commits.
        };
        let mut reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.schema_version().unwrap(), SCHEMA_VERSION);
        let document = reopened.document(main).unwrap();
        assert_eq!(document.digest(), digest);
        assert_eq!(document.event_count(), count);
        assert_eq!(
            reopened.load_report(main).unwrap().path,
            LoadPath::FullReplay
        );
        cleanup(&path);
    }

    #[test]
    fn rejected_commands_write_nothing() {
        let path = temp_path("rejected");
        let mut setup = Setup::new(&path);
        let main = setup.main;
        let sheet = setup.sheet;
        let before = setup.store.document(main).unwrap().digest();
        let error = setup
            .store
            .append(
                main,
                human(),
                5_000,
                Command::SetFormula {
                    sheet,
                    a1: "A1".into(),
                    source: "=A1+1".into(),
                },
            )
            .unwrap_err();
        assert!(matches!(error, StoreError::Apply(ApplyError::Formula(_))));
        let rows: i64 = setup
            .store
            .connection
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 4);
        assert_eq!(setup.store.document(main).unwrap().digest(), before);
        cleanup(&path);
    }

    #[test]
    fn batches_cross_checkpoint_threshold_and_reopen_parses_only_the_tail() {
        let path = temp_path("batch-checkpoint");
        let mut setup = Setup::new(&path);
        let main = setup.main;
        setup.store.set_snapshot_interval(5);
        let commands = (0..3)
            .map(|value| Command::SetValue {
                sheet: setup.sheet,
                a1: "A1".into(),
                value: Literal::Number(f64::from(value)),
            })
            .collect();
        setup
            .store
            .append_batch(main, human(), 2_000, commands)
            .unwrap();
        // Four setup events plus three edits crossed 5 without landing on it.
        let count: i64 = setup
            .store
            .connection
            .query_row(
                "SELECT event_count FROM snapshots WHERE branch = ?1",
                params![main.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 7);
        setup.set(main, "A2", 3.0);
        let expected = setup.store.document(main).unwrap().digest();
        // A valid snapshot permits skipping earlier canonical JSON entirely.
        setup
            .store
            .connection
            .execute(
                "UPDATE events SET canonical = 'unparseable' WHERE seq = 1",
                [],
            )
            .unwrap();
        setup.store.evict();
        assert_eq!(setup.store.document(main).unwrap().digest(), expected);
        assert_eq!(setup.store.load_report(main).unwrap().events_replayed, 1);
        setup.store.write_snapshot(main).unwrap();
        let count: i64 = setup
            .store
            .connection
            .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        setup
            .store
            .connection
            .execute("DELETE FROM snapshots", [])
            .unwrap();
        setup.store.evict();
        assert!(setup.store.document(main).is_err());
        cleanup(&path);
    }

    #[test]
    fn snapshot_plus_tail_equals_full_replay_and_corrupt_snapshots_fall_back() {
        let path = temp_path("snapshot");
        let mut setup = Setup::new(&path);
        let main = setup.main;
        setup.store.set_snapshot_interval(3);
        for value in 1..=7 {
            setup.set(main, "A1", f64::from(value));
        }
        let expected = setup.store.document(main).unwrap().digest();
        setup.store.close().unwrap();

        let mut reopened = Store::open(&path).unwrap();
        let document = reopened.document(main).unwrap();
        assert_eq!(document.digest(), expected);
        let report = reopened.load_report(main).unwrap();
        assert_eq!(report.path, LoadPath::SnapshotPlusTail);
        assert_eq!(report.events_replayed, 0);

        // Corrupt the newest snapshot payload: the store must notice and replay.
        reopened
            .connection
            .execute(
                "UPDATE snapshots SET payload = replace(payload, '\"Budget\"', '\"Broken\"')",
                [],
            )
            .unwrap();
        reopened.evict();
        let document = reopened.document(main).unwrap();
        assert_eq!(document.digest(), expected);
        assert_eq!(
            reopened.load_report(main).unwrap().path,
            LoadPath::SnapshotRejected
        );

        // Append after the snapshot: tail replay covers the new events.
        reopened
            .connection
            .execute("DELETE FROM snapshots", [])
            .unwrap();
        reopened.evict();
        reopened.set_snapshot_interval(1_000);
        let sheet = setup.sheet;
        reopened
            .append(
                main,
                human(),
                9_000,
                Command::SetValue {
                    sheet,
                    a1: "B1".into(),
                    value: Literal::Text("tail".into()),
                },
            )
            .unwrap();
        reopened.write_snapshot(main).unwrap();
        reopened
            .append(
                main,
                human(),
                9_001,
                Command::SetValue {
                    sheet,
                    a1: "B2".into(),
                    value: Literal::Text("after".into()),
                },
            )
            .unwrap();
        let full = reopened.document(main).unwrap().digest();
        reopened.evict();
        let document = reopened.document(main).unwrap();
        assert_eq!(document.digest(), full);
        let report = reopened.load_report(main).unwrap();
        assert_eq!(report.path, LoadPath::SnapshotPlusTail);
        assert_eq!(report.events_replayed, 1);
        cleanup(&path);
    }

    #[test]
    fn version_zero_files_migrate_forward_and_foreign_files_are_refused() {
        let path = temp_path("migrate");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);\n                     CREATE TABLE events (seq INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, parent TEXT, branch TEXT NOT NULL, actor_kind TEXT NOT NULL, actor_id TEXT NOT NULL, timestamp INTEGER NOT NULL, canonical TEXT NOT NULL);\n                     CREATE TABLE branches (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, parent TEXT, base_event TEXT, head TEXT);\n                     CREATE TABLE snapshots (branch TEXT NOT NULL, seq INTEGER NOT NULL, head TEXT NOT NULL, event_count INTEGER NOT NULL, digest TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY (branch, seq));",
                )
                .unwrap();
            let (document, event) =
                Document::create(DocumentId(ObjectId::from_seed("v0")), "Legacy", human(), 1)
                    .unwrap();
            insert_event(&connection, &event).unwrap();
            connection
                .execute(
                    "INSERT INTO branches (id, name, parent, base_event, head) VALUES (?1, 'main', NULL, NULL, ?2)",
                    params![document.branch().to_string(), event.id.to_string()],
                )
                .unwrap();
        }
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), 1);
        let main = store.branch_id("main").unwrap();
        assert_eq!(store.document(main).unwrap().name(), "Legacy");
        let indexed: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'events_branch_seq'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1);
        cleanup(&path);

        let foreign = temp_path("foreign");
        Connection::open(&foreign)
            .unwrap()
            .execute_batch("CREATE TABLE other (x);")
            .unwrap();
        assert!(matches!(
            Store::open(&foreign).unwrap_err(),
            StoreError::NotADocumentStore(_)
        ));
        cleanup(&foreign);

        let missing = temp_path("missing");
        assert!(matches!(
            Store::open(&missing).unwrap_err(),
            StoreError::NotADocumentStore(_)
        ));
    }

    #[test]
    fn branches_diff_semantically_and_merges_are_gated() {
        let path = temp_path("branches");
        let mut setup = Setup::new(&path);
        let main = setup.main;
        let sheet = setup.sheet;
        setup.set(main, "A1", 10.0);
        setup.run(
            main,
            human(),
            Command::SetFormula {
                sheet,
                a1: "A2".into(),
                source: "=A1*2".into(),
            },
        );
        setup.run(
            main,
            human(),
            Command::SetFormula {
                sheet,
                a1: "A3".into(),
                source: "=A2<=100".into(),
            },
        );
        setup.run(
            main,
            human(),
            Command::WatchOutput {
                name: "double".into(),
                sheet,
                a1: "A2".into(),
            },
        );
        setup.run(
            main,
            human(),
            Command::AddCheck {
                name: "bounded".into(),
                sheet,
                a1: "A3".into(),
                severity: Severity::Error,
                message: "A2 must stay at or below 100".into(),
            },
        );

        let work = setup
            .store
            .create_branch(main, "agent-work", agent(), 2_000)
            .unwrap();
        assert_eq!(
            setup.store.branch_names().unwrap(),
            vec!["main", "agent-work"]
        );
        assert!(matches!(
            setup
                .store
                .create_branch(main, "agent-work", agent(), 2_001),
            Err(StoreError::DuplicateBranch(_))
        ));

        // Agent proposes on its branch; main is untouched.
        setup.run(
            work,
            agent(),
            Command::SetValue {
                sheet,
                a1: "A1".into(),
                value: Literal::Number(80.0),
            },
        );
        assert_eq!(setup.value(work, "A2"), CellValue::Number(160.0));
        assert_eq!(setup.value(main, "A2"), CellValue::Number(20.0));

        // Failing check blocks the merge.
        let diff = setup.store.diff(work, main).unwrap();
        assert_eq!(diff.source_operations.len(), 1);
        assert_eq!(diff.source_operations[0].operation, "set_value");
        assert!(diff.target_operations.is_empty());
        assert_eq!(diff.watched_changes.len(), 1);
        assert_eq!(diff.watched_changes[0].name, "double");
        assert_eq!(diff.watched_changes[0].before, CellValue::Number(20.0));
        assert_eq!(diff.watched_changes[0].after, CellValue::Number(160.0));
        assert!(diff.conflicts.is_empty());
        assert!(matches!(
            setup.store.merge(work, main, human(), 3_000),
            Err(StoreError::ChecksFailed(_))
        ));

        // Fix on the branch; an agent still cannot merge.
        setup.run(
            work,
            agent(),
            Command::SetValue {
                sheet,
                a1: "A1".into(),
                value: Literal::Number(40.0),
            },
        );
        assert!(matches!(
            setup.store.merge(work, main, agent(), 3_001),
            Err(StoreError::Unauthorized(_))
        ));

        // A conflicting edit on main blocks the merge until resolved.
        setup.set(main, "A1", 11.0);
        let diff = setup.store.diff(work, main).unwrap();
        assert_eq!(diff.conflicts.len(), 1);
        assert!(matches!(diff.conflicts[0], Touch::Cell { .. }));
        assert!(matches!(
            setup.store.merge(work, main, human(), 3_002),
            Err(StoreError::Conflicts(_))
        ));

        // Resolve by taking the branch's value on main explicitly, then a
        // non-conflicting branch edit merges with human approval.
        let a1 = setup
            .store
            .document(main)
            .unwrap()
            .resolve_a1(sheet, "A1")
            .unwrap();
        let _ = a1;
        let side = setup
            .store
            .create_branch(main, "side", human(), 4_000)
            .unwrap();
        setup.run(
            side,
            agent(),
            Command::SetValue {
                sheet,
                a1: "B1".into(),
                value: Literal::Text("note".into()),
            },
        );
        let report = setup.store.merge(side, main, human(), 4_500).unwrap();
        assert_eq!(report.replayed.len(), 1);
        assert_eq!(setup.value(main, "B1"), CellValue::Text("note".into()));
        let document = setup.store.document(main).unwrap();
        assert_eq!(document.merges().len(), 1);
        assert_eq!(document.merges()[0].source, side);
        let b1 = document.resolve_a1(sheet, "B1").unwrap();
        assert_eq!(
            document.lineage(b1).unwrap().kind,
            omasheets_core::LineageKind::Agent
        );
        assert!(matches!(
            setup.store.merge(side, main, human(), 4_600),
            Err(StoreError::NothingToMerge)
        ));

        // Everything above survives close and reopen by replay and by snapshot.
        let expected = setup.store.document(main).unwrap().digest();
        setup.store.close().unwrap();
        let mut reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.document(main).unwrap().digest(), expected);
        let work_digest = reopened.document(work).unwrap().digest();
        reopened.evict();
        reopened
            .connection
            .execute("DELETE FROM snapshots", [])
            .unwrap();
        assert_eq!(reopened.document(main).unwrap().digest(), expected);
        assert_eq!(reopened.document(work).unwrap().digest(), work_digest);
        cleanup(&path);
    }
}
