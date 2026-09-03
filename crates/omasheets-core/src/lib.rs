//! Stable-ID document model and deterministic semantic events for OmaSheets.
//!
//! A document is the replay of an append-only event log. Every object a
//! formula, a check or an agent can name (document, sheet, row, column,
//! table, branch, proposal, import, check, watched output) has a stable
//! identity that never changes when rows are inserted, sheets are renamed or
//! layout moves. A1 coordinates are accepted as input syntax by
//! [`Document::command`] and are resolved to stable identities before the
//! event is created; replay never re-resolves coordinates.
//!
//! Every mutation goes through [`Document::apply`], which validates the event
//! completely before touching state, so an invalid event leaves the document
//! unchanged. There is no other mutation path. Calculation is delegated to
//! `omasheets-calc`; the event core never reads a clock.

use omasheets_calc::{
    CalcError, CellId, FormulaError, ParsedFormula, StructuredColumn, StructuredContext,
    StructuredTable, Value, Workbook,
};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

pub const EVENT_SCHEMA: u16 = 1;
pub const MAX_NAME_CHARS: usize = 255;
pub const MAX_ACTOR_ID_CHARS: usize = 128;
pub const MAX_TEXT_CHARS: usize = 32_768;
pub const MAX_FORMULA_CHARS: usize = 8_192;
pub const MAX_FORMULA_REFERENCES: usize = 10_000;
pub const MAX_BATCH: usize = 10_000;

const ID_DOMAIN: &[u8] = b"omasheets-core/id/v1";
const EVENT_DOMAIN: &[u8] = b"omasheets-core/event/v1";
const STATE_DOMAIN: &[u8] = b"omasheets-core/state/v1";

// ---------------------------------------------------------------------------
// Identities
// ---------------------------------------------------------------------------

/// A 128-bit stable identity, rendered as 32 lowercase hex characters.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObjectId([u8; 16]);

impl ObjectId {
    /// Derives an identity from a parent digest, an object kind and an
    /// ordinal, so identities minted by replaying the same log are equal.
    pub fn derive(seed: &[u8], kind: &str, ordinal: u64) -> Self {
        let mut digest = Sha256::new();
        digest.update(ID_DOMAIN);
        digest.update([0]);
        digest.update(seed);
        digest.update([0]);
        digest.update(kind.as_bytes());
        digest.update([0]);
        digest.update(ordinal.to_be_bytes());
        let bytes = digest.finalize();
        let mut output = [0_u8; 16];
        output.copy_from_slice(&bytes[..16]);
        Self(output)
    }

    pub fn from_seed(seed: &str) -> Self {
        Self::derive(seed.as_bytes(), "seed", 0)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub fn parse(text: &str) -> Option<Self> {
        let bytes = decode_hex(text, 16)?;
        let mut output = [0_u8; 16];
        output.copy_from_slice(&bytes);
        Some(Self(output))
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ObjectId({self})")
    }
}

impl Serialize for ObjectId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ObjectId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| D::Error::custom("expected 32 hex characters"))
    }
}

fn decode_hex(text: &str, length: usize) -> Option<Vec<u8>> {
    if text.len() != length * 2 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    (0..length)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok())
        .collect()
}

macro_rules! typed_id {
    ($(#[$meta:meta])* $name:ident, $kind:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub ObjectId);

        impl $name {
            pub const KIND: &'static str = $kind;

            pub fn derive(seed: &[u8], ordinal: u64) -> Self {
                Self(ObjectId::derive(seed, $kind, ordinal))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}", self.0)
            }
        }
    };
}

typed_id!(DocumentId, "document");
typed_id!(SheetId, "sheet");
typed_id!(RowId, "row");
typed_id!(ColumnId, "column");
typed_id!(TableId, "table");
typed_id!(BranchId, "branch");
typed_id!(ProposalId, "proposal");
typed_id!(ImportId, "import");
// CheckId and WatchId are reserved for the checks and watched-output
// runbook; no operation creates either yet.
typed_id!(CheckId, "check");
typed_id!(WatchId, "watch");

/// Content address of an event: SHA-256 over its canonical bytes.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EventId([u8; 32]);

impl EventId {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn parse(text: &str) -> Option<Self> {
        let bytes = decode_hex(text, 32)?;
        let mut output = [0_u8; 32];
        output.copy_from_slice(&bytes);
        Some(Self(output))
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for EventId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "EventId({self})")
    }
}

impl Serialize for EventId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for EventId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| D::Error::custom("expected 64 hex characters"))
    }
}

// ---------------------------------------------------------------------------
// Actors, values and addresses
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Human,
    Import,
    Agent,
    ModelAssisted,
    System,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Actor {
    pub kind: ActorKind,
    pub id: String,
}

impl Actor {
    pub fn new(kind: ActorKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }
}

/// A literal a person, an import or an agent enters into a cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Literal {
    Blank,
    Number(f64),
    Text(String),
    Boolean(bool),
}

/// A calculated cell value as the document projects it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CellValue {
    Blank,
    Number(f64),
    Text(String),
    Boolean(bool),
    Error(String),
}

impl From<Value> for CellValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Blank => Self::Blank,
            Value::Number(number) => Self::Number(number),
            Value::Text(text) => Self::Text(text),
            Value::Boolean(flag) => Self::Boolean(flag),
            Value::Error(error) => Self::Error(calc_error_label(&error).into()),
        }
    }
}

fn calc_error_label(error: &CalcError) -> &'static str {
    match error {
        CalcError::DivisionByZero => "#DIV/0!",
        CalcError::InvalidReference => "#REF!",
        CalcError::NotAvailable => "#N/A",
        CalcError::InvalidValue => "#VALUE!",
        CalcError::InvalidNumber => "#NUM!",
        CalcError::InvalidName => "#NAME?",
        CalcError::NullIntersection => "#NULL!",
        CalcError::InvalidArguments => "#ARGS!",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnType {
    Any,
    Number,
    Text,
    Boolean,
}

/// A cell named by stable identities, never by coordinates.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CellRef {
    pub sheet: SheetId,
    pub row: RowId,
    pub column: ColumnId,
}

/// A formula as recorded in an event: the source text as entered, the sheet
/// names it used at that moment, and every cell reference it resolved, in the
/// engine's traversal order. Replay rebinds by position and never consults
/// coordinates or current sheet names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompiledFormula {
    pub source: String,
    pub sheet_bindings: Vec<(String, SheetId)>,
    #[serde(default)]
    pub table_bindings: Vec<(String, TableId)>,
    #[serde(default)]
    pub current_table: Option<TableId>,
    pub references: Vec<CellRef>,
}

/// One computed-column formula materialised for a stable table row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComputedCell {
    pub cell: CellRef,
    pub formula: CompiledFormula,
}

// ---------------------------------------------------------------------------
// Operations and events
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    CreateDocument {
        document: DocumentId,
        name: String,
    },
    AddSheet {
        sheet: SheetId,
        name: String,
    },
    RenameSheet {
        sheet: SheetId,
        name: String,
    },
    DeleteSheet {
        sheet: SheetId,
    },
    /// Inserts columns before view position `at` (`at == len` appends).
    AddColumns {
        sheet: SheetId,
        columns: Vec<ColumnId>,
        at: usize,
    },
    /// Inserts rows before view position `at`; when `table` is given the rows
    /// are also appended to that table.
    AddRows {
        sheet: SheetId,
        rows: Vec<RowId>,
        at: usize,
        table: Option<TableId>,
        #[serde(default)]
        computed: Vec<ComputedCell>,
        #[serde(default)]
        rebound: Vec<ComputedCell>,
    },
    DeleteRows {
        sheet: SheetId,
        rows: Vec<RowId>,
    },
    DeleteColumns {
        sheet: SheetId,
        columns: Vec<ColumnId>,
    },
    AddTable {
        table: TableId,
        sheet: SheetId,
        name: String,
        columns: Vec<ColumnId>,
        #[serde(default)]
        column_names: Vec<String>,
        #[serde(default)]
        header_row: Option<RowId>,
        rows: Vec<RowId>,
    },
    RenameTable {
        table: TableId,
        name: String,
    },
    SetColumnType {
        sheet: SheetId,
        column: ColumnId,
        column_type: ColumnType,
    },
    SetComputedColumn {
        table: TableId,
        column: ColumnId,
        source: String,
        formulas: Vec<ComputedCell>,
    },
    SetValue {
        cell: CellRef,
        value: Literal,
    },
    SetFormula {
        cell: CellRef,
        formula: CompiledFormula,
    },
    ClearCell {
        cell: CellRef,
    },
    /// Records the provenance of a batch of cells that follow it from an
    /// import actor.
    Import {
        import: ImportId,
        source_sha256: String,
        format: String,
    },
    /// An explicit clock tick. Nothing in the core reads wall-clock time.
    Tick {
        tick: u64,
        at: i64,
    },
    Propose {
        proposal: ProposalId,
        description: String,
    },
    AcceptProposal {
        proposal: ProposalId,
    },
    RejectProposal {
        proposal: ProposalId,
        reason: String,
    },
    /// The first event on a new branch: `branch` in the envelope names the
    /// new branch and `parent` (equal to `from`) is the fork point.
    CreateBranch {
        branch: BranchId,
        name: String,
        from: EventId,
    },
    /// Records that `source` (at `source_head`) was replayed onto this branch
    /// as the events `replayed`, after the policy gate passed.
    RecordMerge {
        source: BranchId,
        source_head: EventId,
        replayed: Vec<EventId>,
    },
    /// A deterministic check: it passes when `cell` evaluates to `TRUE`.
    AddCheck {
        check: CheckId,
        name: String,
        cell: CellRef,
        severity: Severity,
        message: String,
    },
    RemoveCheck {
        check: CheckId,
    },
    WatchOutput {
        watch: WatchId,
        name: String,
        cell: CellRef,
    },
    UnwatchOutput {
        watch: WatchId,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Error,
}

/// An object an operation reads or writes, for operation-level conflict
/// detection between branches.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Touch {
    Document,
    Sheet { sheet: SheetId },
    Row { sheet: SheetId, row: RowId },
    Column { sheet: SheetId, column: ColumnId },
    Cell { cell: CellRef },
    Table { table: TableId },
    Proposal { proposal: ProposalId },
    Check { check: CheckId },
    Watch { watch: WatchId },
    Tick,
}

impl Operation {
    /// The objects this operation changes. Two operations conflict when
    /// their touch sets intersect.
    pub fn touches(&self) -> Vec<Touch> {
        match self {
            Operation::CreateDocument { .. } | Operation::CreateBranch { .. } => {
                vec![Touch::Document]
            }
            Operation::RecordMerge { .. } => Vec::new(),
            Operation::AddSheet { sheet, .. }
            | Operation::RenameSheet { sheet, .. }
            | Operation::DeleteSheet { sheet } => vec![Touch::Sheet { sheet: *sheet }],
            Operation::AddColumns { sheet, columns, .. }
            | Operation::DeleteColumns { sheet, columns } => columns
                .iter()
                .map(|column| Touch::Column {
                    sheet: *sheet,
                    column: *column,
                })
                .collect(),
            Operation::AddRows {
                sheet,
                rows,
                table,
                computed,
                rebound,
                ..
            } => {
                let mut touches: Vec<Touch> = rows
                    .iter()
                    .map(|row| Touch::Row {
                        sheet: *sheet,
                        row: *row,
                    })
                    .collect();
                if let Some(table) = table {
                    touches.push(Touch::Table { table: *table });
                }
                touches.extend(
                    computed
                        .iter()
                        .map(|formula| Touch::Cell { cell: formula.cell }),
                );
                touches.extend(
                    rebound
                        .iter()
                        .map(|formula| Touch::Cell { cell: formula.cell }),
                );
                touches
            }
            Operation::DeleteRows { sheet, rows } => rows
                .iter()
                .map(|row| Touch::Row {
                    sheet: *sheet,
                    row: *row,
                })
                .collect(),
            Operation::AddTable { table, .. } | Operation::RenameTable { table, .. } => {
                vec![Touch::Table { table: *table }]
            }
            Operation::SetColumnType { sheet, column, .. } => vec![Touch::Column {
                sheet: *sheet,
                column: *column,
            }],
            Operation::SetComputedColumn {
                table,
                formulas,
                ..
            } => {
                let mut touches = vec![Touch::Table { table: *table }];
                touches.extend(
                    formulas
                        .iter()
                        .map(|formula| Touch::Cell { cell: formula.cell }),
                );
                touches
            }
            Operation::SetValue { cell, .. }
            | Operation::SetFormula { cell, .. }
            | Operation::ClearCell { cell } => vec![Touch::Cell { cell: *cell }],
            Operation::Import { .. } => vec![Touch::Document],
            Operation::Tick { .. } => vec![Touch::Tick],
            Operation::Propose { proposal, .. }
            | Operation::AcceptProposal { proposal }
            | Operation::RejectProposal { proposal, .. } => {
                vec![Touch::Proposal {
                    proposal: *proposal,
                }]
            }
            Operation::AddCheck { check, .. } | Operation::RemoveCheck { check } => {
                vec![Touch::Check { check: *check }]
            }
            Operation::WatchOutput { watch, .. } | Operation::UnwatchOutput { watch } => {
                vec![Touch::Watch { watch: *watch }]
            }
        }
    }
}

/// The versioned envelope. `id` is the SHA-256 of the canonical bytes of
/// every other field, so an event is self-verifying.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub schema: u16,
    pub id: EventId,
    pub parent: Option<EventId>,
    pub branch: BranchId,
    pub actor: Actor,
    pub timestamp: i64,
    pub operation: Operation,
}

#[derive(Serialize)]
struct EventBody<'a> {
    schema: u16,
    parent: Option<EventId>,
    branch: BranchId,
    actor: &'a Actor,
    timestamp: i64,
    operation: &'a Operation,
}

impl Event {
    pub fn new(
        parent: Option<EventId>,
        branch: BranchId,
        actor: Actor,
        timestamp: i64,
        operation: Operation,
    ) -> Self {
        let id = Self::compute_id(parent, branch, &actor, timestamp, &operation);
        Self {
            schema: EVENT_SCHEMA,
            id,
            parent,
            branch,
            actor,
            timestamp,
            operation,
        }
    }

    fn compute_id(
        parent: Option<EventId>,
        branch: BranchId,
        actor: &Actor,
        timestamp: i64,
        operation: &Operation,
    ) -> EventId {
        let body = EventBody {
            schema: EVENT_SCHEMA,
            parent,
            branch,
            actor,
            timestamp,
            operation,
        };
        let bytes = serde_json::to_vec(&body).expect("event bodies serialise");
        let mut digest = Sha256::new();
        digest.update(EVENT_DOMAIN);
        digest.update([0]);
        digest.update(&bytes);
        EventId(digest.finalize().into())
    }

    /// True when `id` matches the canonical bytes of the other fields.
    pub fn verify(&self) -> bool {
        self.schema == EVENT_SCHEMA
            && self.id
                == Self::compute_id(
                    self.parent,
                    self.branch,
                    &self.actor,
                    self.timestamp,
                    &self.operation,
                )
    }

    /// Canonical JSON: fixed field order, sorted maps, no whitespace.
    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).expect("events serialise")
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplyError {
    InvalidEventId,
    UnsupportedSchema(u16),
    ParentMismatch {
        expected: Option<EventId>,
        observed: Option<EventId>,
    },
    BranchMismatch,
    DocumentAlreadyCreated,
    DocumentNotCreated,
    InvalidActor,
    InvalidName,
    InvalidTimestamp,
    UnknownSheet(SheetId),
    UnknownRow(RowId),
    UnknownColumn(ColumnId),
    UnknownTable(TableId),
    UnknownProposal(ProposalId),
    DuplicateId(ObjectId),
    DuplicateName(String),
    EmptyBatch,
    BatchTooLarge,
    PositionOutOfRange,
    ReferenceOutOfView(String),
    ReferencedByFormula(CellRef),
    TypeMismatch {
        column: ColumnId,
        expected: ColumnType,
    },
    InvalidValue,
    FormulaTooLong,
    TooManyReferences,
    FormulaShapeMismatch,
    Formula(FormulaError),
    ProposalNotPending(ProposalId),
    TickNotMonotonic,
    InvalidDigest,
    UnknownBranch(BranchId),
    UnknownCheck(CheckId),
    UnknownWatch(WatchId),
    ForkPointMismatch,
    UnknownEvent(EventId),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for ApplyError {}

impl From<FormulaError> for ApplyError {
    fn from(error: FormulaError) -> Self {
        Self::Formula(error)
    }
}

// ---------------------------------------------------------------------------
// Commands: A1 input syntax resolved to stable identities
// ---------------------------------------------------------------------------

/// What a person, import or agent asks for, in input syntax. `a1` addresses
/// name a cell in a sheet's current view; they are resolved to stable
/// identities when the event is created.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    AddSheet {
        name: String,
    },
    RenameSheet {
        sheet: SheetId,
        name: String,
    },
    DeleteSheet {
        sheet: SheetId,
    },
    AddColumns {
        sheet: SheetId,
        count: usize,
        at: usize,
    },
    AddRows {
        sheet: SheetId,
        count: usize,
        at: usize,
        table: Option<TableId>,
    },
    DeleteRows {
        sheet: SheetId,
        rows: Vec<RowId>,
    },
    DeleteColumns {
        sheet: SheetId,
        columns: Vec<ColumnId>,
    },
    AddTable {
        sheet: SheetId,
        name: String,
        columns: Vec<ColumnId>,
        #[serde(default)]
        column_names: Vec<String>,
        #[serde(default)]
        header_row: Option<RowId>,
        rows: Vec<RowId>,
    },
    RenameTable {
        table: TableId,
        name: String,
    },
    SetColumnType {
        sheet: SheetId,
        column: ColumnId,
        column_type: ColumnType,
    },
    SetComputedColumn {
        table: TableId,
        column: ColumnId,
        source: String,
    },
    SetValue {
        sheet: SheetId,
        a1: String,
        value: Literal,
    },
    SetFormula {
        sheet: SheetId,
        a1: String,
        source: String,
    },
    ClearCell {
        sheet: SheetId,
        a1: String,
    },
    Import {
        source_sha256: String,
        format: String,
    },
    Tick {
        at: i64,
    },
    Propose {
        description: String,
    },
    AcceptProposal {
        proposal: ProposalId,
    },
    RejectProposal {
        proposal: ProposalId,
        reason: String,
    },
    AddCheck {
        name: String,
        sheet: SheetId,
        a1: String,
        severity: Severity,
        message: String,
    },
    RemoveCheck {
        check: CheckId,
    },
    WatchOutput {
        name: String,
        sheet: SheetId,
        a1: String,
    },
    UnwatchOutput {
        watch: WatchId,
    },
}

// ---------------------------------------------------------------------------
// Document state
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub event: EventId,
    pub actor: Actor,
    pub timestamp: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CellInput {
    Value { value: Literal },
    Formula { formula: CompiledFormula },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellState {
    pub input: CellInput,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Serialize)]
struct Sheet {
    name: String,
    /// View order; positions are projections of these identities.
    rows: Vec<RowId>,
    columns: Vec<ColumnId>,
    row_ordinals: BTreeMap<RowId, u32>,
    column_ordinals: BTreeMap<ColumnId, u32>,
    next_row_ordinal: u32,
    next_column_ordinal: u32,
    column_types: BTreeMap<ColumnId, ColumnType>,
    cells: BTreeMap<(RowId, ColumnId), CellState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub name: String,
    pub sheet: SheetId,
    pub columns: Vec<ColumnId>,
    #[serde(default)]
    pub column_names: BTreeMap<ColumnId, String>,
    #[serde(default)]
    pub header_row: Option<RowId>,
    pub rows: Vec<RowId>,
    #[serde(default)]
    pub computed_columns: BTreeMap<ColumnId, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportRecord {
    pub source_sha256: String,
    pub format: String,
    pub provenance: Provenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Accepted,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposalRecord {
    pub description: String,
    pub status: ProposalStatus,
    pub proposed: Provenance,
    pub resolved: Option<Provenance>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchRecord {
    pub name: String,
    pub parent: Option<BranchId>,
    pub base: Option<EventId>,
    pub created: Provenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckRecord {
    pub name: String,
    pub cell: CellRef,
    pub severity: Severity,
    pub message: String,
    pub added: Provenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WatchRecord {
    pub name: String,
    pub cell: CellRef,
    pub added: Provenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MergeRecord {
    pub source: BranchId,
    pub source_head: EventId,
    pub replayed: Vec<EventId>,
    pub recorded: Provenance,
}

/// Outcome of one check against the current state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    pub check: CheckId,
    pub name: String,
    pub severity: Severity,
    pub passed: bool,
    pub value: CellValue,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineageKind {
    Entered,
    Imported,
    Computed,
    Agent,
    ModelAssisted,
    System,
}

/// Where a value came from: the event and actor that set it, and, for a
/// formula, the cells it reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lineage {
    pub kind: LineageKind,
    pub event: EventId,
    pub actor: Actor,
    pub timestamp: i64,
    pub inputs: Vec<CellRef>,
}

/// The replayed state of one branch of one document.
#[derive(Clone)]
pub struct Document {
    id: DocumentId,
    name: String,
    branch: BranchId,
    head: Option<EventId>,
    event_count: u64,
    sheet_order: Vec<SheetId>,
    sheets: BTreeMap<SheetId, Sheet>,
    sheet_ordinals: BTreeMap<SheetId, u32>,
    next_sheet_ordinal: u32,
    tables: BTreeMap<TableId, Table>,
    imports: BTreeMap<ImportId, ImportRecord>,
    proposals: BTreeMap<ProposalId, ProposalRecord>,
    last_tick: Option<(u64, i64)>,
    branches: BTreeMap<BranchId, BranchRecord>,
    checks: BTreeMap<CheckId, CheckRecord>,
    watches: BTreeMap<WatchId, WatchRecord>,
    merges: Vec<MergeRecord>,
    dependents: BTreeMap<CellRef, BTreeSet<CellRef>>,
    calc: Workbook,
}

/// Canonical projection of a document used for digests and inspection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema: u16,
    pub document: DocumentId,
    pub name: String,
    pub branch: BranchId,
    pub head: Option<EventId>,
    pub event_count: u64,
    pub sheets: Vec<SheetSnapshot>,
    pub tables: BTreeMap<TableId, Table>,
    pub imports: BTreeMap<ImportId, ImportRecord>,
    pub proposals: BTreeMap<ProposalId, ProposalRecord>,
    pub last_tick: Option<(u64, i64)>,
    pub branches: BTreeMap<BranchId, BranchRecord>,
    pub checks: BTreeMap<CheckId, CheckRecord>,
    pub watches: BTreeMap<WatchId, WatchRecord>,
    pub merges: Vec<MergeRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SheetSnapshot {
    pub id: SheetId,
    pub name: String,
    pub rows: Vec<RowId>,
    pub columns: Vec<ColumnId>,
    pub column_types: BTreeMap<ColumnId, ColumnType>,
    pub cells: Vec<CellSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellSnapshot {
    pub row: RowId,
    pub column: ColumnId,
    pub state: CellState,
    pub value: CellValue,
}

impl fmt::Debug for Document {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Document")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("branch", &self.branch)
            .field("head", &self.head)
            .field("event_count", &self.event_count)
            .field("sheets", &self.sheet_order)
            .finish()
    }
}

impl Document {
    /// Creates a document by applying its first event.
    pub fn create(
        document: DocumentId,
        name: impl Into<String>,
        actor: Actor,
        timestamp: i64,
    ) -> Result<(Self, Event), ApplyError> {
        let mut state = Self::empty(document);
        let event = Event::new(
            None,
            state.branch,
            actor,
            timestamp,
            Operation::CreateDocument {
                document,
                name: name.into(),
            },
        );
        state.apply(&event)?;
        Ok((state, event))
    }

    fn empty(document: DocumentId) -> Self {
        Self {
            id: document,
            name: String::new(),
            branch: BranchId::derive(document.0.as_bytes(), 0),
            head: None,
            event_count: 0,
            sheet_order: Vec::new(),
            sheets: BTreeMap::new(),
            sheet_ordinals: BTreeMap::new(),
            next_sheet_ordinal: 0,
            tables: BTreeMap::new(),
            imports: BTreeMap::new(),
            proposals: BTreeMap::new(),
            last_tick: None,
            branches: BTreeMap::new(),
            checks: BTreeMap::new(),
            watches: BTreeMap::new(),
            merges: Vec::new(),
            dependents: BTreeMap::new(),
            calc: Workbook::default(),
        }
    }

    /// Rebuilds a document from a canonical snapshot. The caller must verify
    /// the result's [`Document::digest`] against the digest recorded with the
    /// snapshot; a mismatch means the snapshot is stale or corrupt and the
    /// log must be replayed instead.
    pub fn from_snapshot(snapshot: &Snapshot) -> Result<Self, ApplyError> {
        if snapshot.schema != EVENT_SCHEMA {
            return Err(ApplyError::UnsupportedSchema(snapshot.schema));
        }
        let mut document = Self::empty(snapshot.document);
        document.name = snapshot.name.clone();
        document.branch = snapshot.branch;
        document.head = snapshot.head;
        document.event_count = snapshot.event_count;
        let mut formulas = Vec::new();
        for sheet in &snapshot.sheets {
            let ordinal = document.next_sheet_ordinal;
            document.next_sheet_ordinal += 1;
            document.sheet_ordinals.insert(sheet.id, ordinal);
            document.sheet_order.push(sheet.id);
            document.calc.define_sheet(ordinal, sheet.name.clone());
            let row_ordinals: BTreeMap<RowId, u32> = sheet
                .rows
                .iter()
                .enumerate()
                .map(|(index, row)| (*row, index as u32))
                .collect();
            let column_ordinals: BTreeMap<ColumnId, u32> = sheet
                .columns
                .iter()
                .enumerate()
                .map(|(index, column)| (*column, index as u32))
                .collect();
            if row_ordinals.len() != sheet.rows.len()
                || column_ordinals.len() != sheet.columns.len()
            {
                return Err(ApplyError::InvalidDigest);
            }
            document.sheets.insert(
                sheet.id,
                Sheet {
                    name: sheet.name.clone(),
                    rows: sheet.rows.clone(),
                    columns: sheet.columns.clone(),
                    next_row_ordinal: sheet.rows.len() as u32,
                    next_column_ordinal: sheet.columns.len() as u32,
                    row_ordinals,
                    column_ordinals,
                    column_types: sheet.column_types.clone(),
                    cells: BTreeMap::new(),
                },
            );
            for cell in &sheet.cells {
                let reference = CellRef {
                    sheet: sheet.id,
                    row: cell.row,
                    column: cell.column,
                };
                let state = document.sheet(sheet.id)?;
                document.check_cell_exists(state, &reference)?;
                match &cell.state.input {
                    CellInput::Value { value } => {
                        let engine = document.engine_cell(reference).expect("checked");
                        match value {
                            Literal::Blank => document.calc.clear(engine),
                            Literal::Number(number) => document.calc.set_number(engine, *number),
                            Literal::Text(text) => document.calc.set_text(engine, text.clone()),
                            Literal::Boolean(flag) => document.calc.set_boolean(engine, *flag),
                        };
                    }
                    CellInput::Formula { .. } => formulas.push((reference, cell.state.clone())),
                }
                document
                    .sheets
                    .get_mut(&sheet.id)
                    .expect("inserted")
                    .cells
                    .insert((cell.row, cell.column), cell.state.clone());
            }
        }
        document.tables = snapshot.tables.clone();
        for (reference, state) in formulas {
            let CellInput::Formula { formula } = &state.input else {
                unreachable!("collected formulas only")
            };
            let bound = document.bind_formula(reference, formula)?;
            let engine = document.engine_cell(reference).expect("checked");
            document.calc.set_parsed_formula(engine, bound)?;
            document.attach_dependencies(reference, formula);
        }
        document.imports = snapshot.imports.clone();
        document.proposals = snapshot.proposals.clone();
        document.last_tick = snapshot.last_tick;
        document.branches = snapshot.branches.clone();
        document.checks = snapshot.checks.clone();
        document.watches = snapshot.watches.clone();
        document.merges = snapshot.merges.clone();
        Ok(document)
    }

    pub fn branches(&self) -> &BTreeMap<BranchId, BranchRecord> {
        &self.branches
    }

    pub fn checks(&self) -> &BTreeMap<CheckId, CheckRecord> {
        &self.checks
    }

    pub fn watches(&self) -> &BTreeMap<WatchId, WatchRecord> {
        &self.watches
    }

    pub fn merges(&self) -> &[MergeRecord] {
        &self.merges
    }

    /// Evaluates every check against the current state. A check passes only
    /// when its cell is exactly `TRUE`; errors and other values fail.
    pub fn check_results(&self) -> Vec<CheckResult> {
        self.checks
            .iter()
            .map(|(id, record)| {
                let value = self.value(record.cell);
                CheckResult {
                    check: *id,
                    name: record.name.clone(),
                    severity: record.severity,
                    passed: value == CellValue::Boolean(true),
                    value,
                    message: record.message.clone(),
                }
            })
            .collect()
    }

    /// Where a cell's value came from, or `None` for a cell nothing has set.
    pub fn lineage(&self, cell: CellRef) -> Option<Lineage> {
        let state = self.cell(cell)?;
        let (kind, inputs) = match &state.input {
            CellInput::Formula { formula } => {
                let mut inputs: Vec<CellRef> = formula.references.clone();
                inputs.sort_unstable();
                inputs.dedup();
                (LineageKind::Computed, inputs)
            }
            CellInput::Value { .. } => (
                match state.provenance.actor.kind {
                    ActorKind::Human => LineageKind::Entered,
                    ActorKind::Import => LineageKind::Imported,
                    ActorKind::Agent => LineageKind::Agent,
                    ActorKind::ModelAssisted => LineageKind::ModelAssisted,
                    ActorKind::System => LineageKind::System,
                },
                Vec::new(),
            ),
        };
        Some(Lineage {
            kind,
            event: state.provenance.event,
            actor: state.provenance.actor.clone(),
            timestamp: state.provenance.timestamp,
            inputs,
        })
    }

    /// Rebuilds a document from its log. The first event must create it.
    pub fn replay(events: &[Event]) -> Result<Self, ApplyError> {
        let first = events.first().ok_or(ApplyError::DocumentNotCreated)?;
        let Operation::CreateDocument { document, .. } = &first.operation else {
            return Err(ApplyError::DocumentNotCreated);
        };
        let mut state = Self::empty(*document);
        for event in events {
            state.apply(event)?;
        }
        Ok(state)
    }

    pub fn id(&self) -> DocumentId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn branch(&self) -> BranchId {
        self.branch
    }

    pub fn head(&self) -> Option<EventId> {
        self.head
    }

    pub fn event_count(&self) -> u64 {
        self.event_count
    }

    pub fn sheets(&self) -> &[SheetId] {
        &self.sheet_order
    }

    pub fn sheet_name(&self, sheet: SheetId) -> Option<&str> {
        self.sheets.get(&sheet).map(|sheet| sheet.name.as_str())
    }

    /// Number of cells with an input on `sheet`; zero for an unknown sheet.
    pub fn cell_count(&self, sheet: SheetId) -> usize {
        self.sheets.get(&sheet).map_or(0, |sheet| sheet.cells.len())
    }

    pub fn rows(&self, sheet: SheetId) -> Option<&[RowId]> {
        self.sheets.get(&sheet).map(|sheet| sheet.rows.as_slice())
    }

    pub fn columns(&self, sheet: SheetId) -> Option<&[ColumnId]> {
        self.sheets
            .get(&sheet)
            .map(|sheet| sheet.columns.as_slice())
    }

    pub fn table(&self, table: TableId) -> Option<&Table> {
        self.tables.get(&table)
    }

    pub fn proposal(&self, proposal: ProposalId) -> Option<&ProposalRecord> {
        self.proposals.get(&proposal)
    }

    pub fn cell(&self, cell: CellRef) -> Option<&CellState> {
        self.sheets
            .get(&cell.sheet)
            .and_then(|sheet| sheet.cells.get(&(cell.row, cell.column)))
    }

    /// The calculated value of a cell, `Blank` when it holds nothing.
    pub fn value(&self, cell: CellRef) -> CellValue {
        match self.engine_cell(cell) {
            Some(engine) => self.calc.value(engine).into(),
            None => CellValue::Blank,
        }
    }

    /// Resolves an A1 address in a sheet's current view to stable identities.
    pub fn resolve_a1(&self, sheet: SheetId, a1: &str) -> Result<CellRef, ApplyError> {
        let state = self.sheet(sheet)?;
        let (row, column) =
            parse_a1(a1).ok_or_else(|| ApplyError::ReferenceOutOfView(a1.into()))?;
        let row = *state
            .rows
            .get(row)
            .ok_or_else(|| ApplyError::ReferenceOutOfView(a1.into()))?;
        let column = *state
            .columns
            .get(column)
            .ok_or_else(|| ApplyError::ReferenceOutOfView(a1.into()))?;
        Ok(CellRef { sheet, row, column })
    }

    /// Renders the current view address of a stable cell.
    pub fn project_a1(&self, cell: CellRef) -> Option<String> {
        let sheet = self.sheets.get(&cell.sheet)?;
        let row = sheet.rows.iter().position(|row| *row == cell.row)?;
        let column = sheet
            .columns
            .iter()
            .position(|column| *column == cell.column)?;
        Some(format!("{}{}", column_letters(column), row + 1))
    }

    /// Canonical projection of the whole state.
    pub fn snapshot(&self) -> Snapshot {
        let sheets = self
            .sheet_order
            .iter()
            .map(|id| {
                let sheet = &self.sheets[id];
                SheetSnapshot {
                    id: *id,
                    name: sheet.name.clone(),
                    rows: sheet.rows.clone(),
                    columns: sheet.columns.clone(),
                    column_types: sheet.column_types.clone(),
                    cells: sheet
                        .cells
                        .iter()
                        .map(|((row, column), state)| CellSnapshot {
                            row: *row,
                            column: *column,
                            state: state.clone(),
                            value: self.value(CellRef {
                                sheet: *id,
                                row: *row,
                                column: *column,
                            }),
                        })
                        .collect(),
                }
            })
            .collect();
        Snapshot {
            schema: EVENT_SCHEMA,
            document: self.id,
            name: self.name.clone(),
            branch: self.branch,
            head: self.head,
            event_count: self.event_count,
            sheets,
            tables: self.tables.clone(),
            imports: self.imports.clone(),
            proposals: self.proposals.clone(),
            last_tick: self.last_tick,
            branches: self.branches.clone(),
            checks: self.checks.clone(),
            watches: self.watches.clone(),
            merges: self.merges.clone(),
        }
    }

    /// SHA-256 over the canonical snapshot bytes, rendered as hex.
    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(&self.snapshot()).expect("snapshots serialise");
        let mut digest = Sha256::new();
        digest.update(STATE_DOMAIN);
        digest.update([0]);
        digest.update(&bytes);
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    // -- command path -----------------------------------------------------

    /// Resolves a command against the current state, mints any new
    /// identities from the current head, applies the resulting event and
    /// returns it. This is the only way to mutate a document besides
    /// [`Document::apply`], and both attribute the change to an actor.
    pub fn command(
        &mut self,
        actor: Actor,
        timestamp: i64,
        command: Command,
    ) -> Result<Event, ApplyError> {
        let operation = self.resolve(command)?;
        let event = Event::new(self.head, self.branch, actor, timestamp, operation);
        self.apply(&event)?;
        Ok(event)
    }

    fn seed(&self) -> Vec<u8> {
        match self.head {
            Some(head) => head.as_bytes().to_vec(),
            None => self.id.0.as_bytes().to_vec(),
        }
    }

    fn resolve(&self, command: Command) -> Result<Operation, ApplyError> {
        let seed = self.seed();
        Ok(match command {
            Command::AddSheet { name } => Operation::AddSheet {
                sheet: SheetId::derive(&seed, 0),
                name,
            },
            Command::RenameSheet { sheet, name } => Operation::RenameSheet { sheet, name },
            Command::DeleteSheet { sheet } => Operation::DeleteSheet { sheet },
            Command::AddColumns { sheet, count, at } => {
                check_batch(count)?;
                Operation::AddColumns {
                    sheet,
                    columns: (0..count as u64)
                        .map(|ordinal| ColumnId::derive(&seed, ordinal))
                        .collect(),
                    at,
                }
            }
            Command::AddRows {
                sheet,
                count,
                at,
                table,
            } => {
                check_batch(count)?;
                let rows: Vec<RowId> = (0..count as u64)
                    .map(|ordinal| RowId::derive(&seed, ordinal))
                    .collect();
                self.resolve_add_rows(sheet, rows, at, table)?
            }
            Command::DeleteRows { sheet, rows } => Operation::DeleteRows { sheet, rows },
            Command::DeleteColumns { sheet, columns } => {
                Operation::DeleteColumns { sheet, columns }
            }
            Command::AddTable {
                sheet,
                name,
                columns,
                column_names,
                header_row,
                rows,
            } => Operation::AddTable {
                table: TableId::derive(&seed, 0),
                sheet,
                name,
                columns,
                column_names,
                header_row,
                rows,
            },
            Command::RenameTable { table, name } => Operation::RenameTable { table, name },
            Command::SetColumnType {
                sheet,
                column,
                column_type,
            } => Operation::SetColumnType {
                sheet,
                column,
                column_type,
            },
            Command::SetComputedColumn {
                table,
                column,
                source,
            } => Operation::SetComputedColumn {
                table,
                column,
                formulas: self.compile_computed_column(table, column, &source)?,
                source,
            },
            Command::SetValue { sheet, a1, value } => Operation::SetValue {
                cell: self.resolve_a1(sheet, &a1)?,
                value,
            },
            Command::SetFormula { sheet, a1, source } => {
                let cell = self.resolve_a1(sheet, &a1)?;
                let containing: Vec<TableId> = self
                    .tables
                    .iter()
                    .filter(|(_, table)| table.sheet == sheet && table.rows.contains(&cell.row))
                    .map(|(id, _)| *id)
                    .collect();
                let current_table = if source.contains('[') && containing.len() == 1 {
                    Some(containing[0])
                } else {
                    None
                };
                Operation::SetFormula {
                    cell,
                    formula: self.compile_formula_at(cell, current_table, &source)?,
                }
            }
            Command::ClearCell { sheet, a1 } => Operation::ClearCell {
                cell: self.resolve_a1(sheet, &a1)?,
            },
            Command::Import {
                source_sha256,
                format,
            } => Operation::Import {
                import: ImportId::derive(&seed, 0),
                source_sha256,
                format,
            },
            Command::Tick { at } => Operation::Tick {
                tick: self.last_tick.map_or(1, |(tick, _)| tick + 1),
                at,
            },
            Command::Propose { description } => Operation::Propose {
                proposal: ProposalId::derive(&seed, 0),
                description,
            },
            Command::AcceptProposal { proposal } => Operation::AcceptProposal { proposal },
            Command::RejectProposal { proposal, reason } => {
                Operation::RejectProposal { proposal, reason }
            }
            Command::AddCheck {
                name,
                sheet,
                a1,
                severity,
                message,
            } => Operation::AddCheck {
                check: CheckId::derive(&seed, 0),
                name,
                cell: self.resolve_a1(sheet, &a1)?,
                severity,
                message,
            },
            Command::RemoveCheck { check } => Operation::RemoveCheck { check },
            Command::WatchOutput { name, sheet, a1 } => Operation::WatchOutput {
                watch: WatchId::derive(&seed, 0),
                name,
                cell: self.resolve_a1(sheet, &a1)?,
            },
            Command::UnwatchOutput { watch } => Operation::UnwatchOutput { watch },
        })
    }

    /// Builds the first event of a new branch forked at the current head.
    /// The branch identity derives from the head so replay mints it again.
    pub fn fork(&self, name: impl Into<String>, actor: Actor, timestamp: i64) -> Event {
        let branch = BranchId::derive(&self.seed(), 1);
        Event::new(
            self.head,
            branch,
            actor,
            timestamp,
            Operation::CreateBranch {
                branch,
                name: name.into(),
                from: self.head.expect("a created document has a head"),
            },
        )
    }

    /// Parses A1 source against the current view and records every reference
    /// as stable identities, in the engine's traversal order.
    fn resolve_add_rows(
        &self,
        sheet: SheetId,
        rows: Vec<RowId>,
        at: usize,
        table: Option<TableId>,
    ) -> Result<Operation, ApplyError> {
        let state = self.sheet(sheet)?;
        if at > state.rows.len() {
            return Err(ApplyError::PositionOutOfRange);
        }
        self.check_fresh_batch(rows.iter().map(|id| id.0))?;
        if let Some(table_id) = table {
            let record = self
                .tables
                .get(&table_id)
                .ok_or(ApplyError::UnknownTable(table_id))?;
            if record.sheet != sheet {
                return Err(ApplyError::UnknownTable(table_id));
            }
        }

        let mut staged = self.clone();
        let state = staged.sheets.get_mut(&sheet).expect("checked");
        for (offset, row) in rows.iter().enumerate() {
            state.rows.insert(at + offset, *row);
            state.row_ordinals.insert(*row, state.next_row_ordinal);
            state.next_row_ordinal += 1;
        }
        let mut computed = Vec::new();
        let mut rebound = Vec::new();
        if let Some(table_id) = table {
            staged
                .tables
                .get_mut(&table_id)
                .expect("checked")
                .rows
                .extend(rows.iter().copied());
            let definitions = staged.tables[&table_id].computed_columns.clone();
            for (formula_sheet, sheet_state) in &staged.sheets {
                for ((row, column), state) in &sheet_state.cells {
                    let CellInput::Formula { formula } = &state.input else {
                        continue;
                    };
                    if formula
                        .table_bindings
                        .iter()
                        .any(|(_, bound)| *bound == table_id)
                    {
                        let cell = CellRef {
                            sheet: *formula_sheet,
                            row: *row,
                            column: *column,
                        };
                        rebound.push(ComputedCell {
                            cell,
                            formula: staged.recompile_formula(cell, formula)?,
                        });
                    }
                }
            }
            for row in &rows {
                for (column, source) in &definitions {
                    let cell = CellRef {
                        sheet,
                        row: *row,
                        column: *column,
                    };
                    computed.push(ComputedCell {
                        cell,
                        formula: staged.compile_formula_at(cell, Some(table_id), source)?,
                    });
                }
            }
            if computed.len() > MAX_BATCH || rebound.len() > MAX_BATCH {
                return Err(ApplyError::BatchTooLarge);
            }
        }
        Ok(Operation::AddRows {
            sheet,
            rows,
            at,
            table,
            computed,
            rebound,
        })
    }

    fn compile_computed_column(
        &self,
        table: TableId,
        column: ColumnId,
        source: &str,
    ) -> Result<Vec<ComputedCell>, ApplyError> {
        let record = self
            .tables
            .get(&table)
            .ok_or(ApplyError::UnknownTable(table))?;
        if !record.columns.contains(&column) {
            return Err(ApplyError::UnknownColumn(column));
        }
        record
            .rows
            .iter()
            .map(|row| {
                let cell = CellRef {
                    sheet: record.sheet,
                    row: *row,
                    column,
                };
                Ok(ComputedCell {
                    cell,
                    formula: self.compile_formula_at(cell, Some(table), source)?,
                })
            })
            .collect()
    }

    pub fn compile_formula(
        &self,
        sheet: SheetId,
        source: &str,
    ) -> Result<CompiledFormula, ApplyError> {
        self.compile_formula_context(sheet, None, None, source)
    }

    fn compile_formula_at(
        &self,
        cell: CellRef,
        current_table: Option<TableId>,
        source: &str,
    ) -> Result<CompiledFormula, ApplyError> {
        self.compile_formula_context(cell.sheet, Some(cell.row), current_table, source)
    }

    fn compile_formula_context(
        &self,
        sheet: SheetId,
        current_row: Option<RowId>,
        current_table: Option<TableId>,
        source: &str,
    ) -> Result<CompiledFormula, ApplyError> {
        if source.chars().count() > MAX_FORMULA_CHARS {
            return Err(ApplyError::FormulaTooLong);
        }
        let origin = *self
            .sheet_ordinals
            .get(&sheet)
            .ok_or(ApplyError::UnknownSheet(sheet))?;
        let names: HashMap<String, u32> = self
            .sheets
            .iter()
            .map(|(id, state)| (state.name.clone(), self.sheet_ordinals[id]))
            .collect();
        let by_ordinal: BTreeMap<u32, SheetId> = self
            .sheet_ordinals
            .iter()
            .map(|(id, ordinal)| (*ordinal, *id))
            .collect();
        let structured = self.structured_tables(None, false)?;
        let current_table_name = current_table
            .map(|id| {
                self.tables
                    .get(&id)
                    .map(|table| table.name.as_str())
                    .ok_or(ApplyError::UnknownTable(id))
            })
            .transpose()?;
        let current_row = current_row
            .map(|row| {
                self.sheets[&sheet]
                    .rows
                    .iter()
                    .position(|candidate| *candidate == row)
                    .map(|position| position as u32)
                    .ok_or(ApplyError::UnknownRow(row))
            })
            .transpose()?;
        let parsed = ParsedFormula::parse_with_structured_references(
            source,
            origin,
            &names,
            StructuredContext {
                tables: &structured,
                current_table: current_table_name,
                current_row,
            },
        )?;
        let view_references = parsed.references();
        if view_references.len() > MAX_FORMULA_REFERENCES {
            return Err(ApplyError::TooManyReferences);
        }
        let mut references = Vec::with_capacity(view_references.len());
        let mut bound = BTreeSet::new();
        for view in view_references {
            let target = by_ordinal[&view.sheet];
            let state = &self.sheets[&target];
            let address = format!("{}{}", column_letters(view.column as usize), view.row + 1);
            let row = *state
                .rows
                .get(view.row as usize)
                .ok_or_else(|| ApplyError::ReferenceOutOfView(address.clone()))?;
            let column = *state
                .columns
                .get(view.column as usize)
                .ok_or(ApplyError::ReferenceOutOfView(address))?;
            if target != sheet {
                bound.insert(target);
            }
            references.push(CellRef {
                sheet: target,
                row,
                column,
            });
        }
        let sheet_bindings = bound
            .into_iter()
            .map(|id| (self.sheets[&id].name.clone(), id))
            .collect();
        let mut table_bindings = parsed
            .structured_tables()
            .iter()
            .map(|name| {
                self.tables
                    .iter()
                    .find(|(_, table)| table.name.eq_ignore_ascii_case(name))
                    .map(|(id, table)| (table.name.clone(), *id))
                    .ok_or_else(|| ApplyError::Formula(FormulaError::UnknownTable(name.clone())))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(current) = current_table
            && !table_bindings.iter().any(|(_, id)| *id == current)
        {
            table_bindings.push((self.tables[&current].name.clone(), current));
            table_bindings.sort_by(|left, right| left.0.to_lowercase().cmp(&right.0.to_lowercase()));
        }
        Ok(CompiledFormula {
            source: source.to_string(),
            sheet_bindings,
            table_bindings,
            current_table,
            references,
        })
    }

    fn structured_tables(
        &self,
        bindings: Option<&[(String, TableId)]>,
        engine_ordinals: bool,
    ) -> Result<Vec<StructuredTable>, ApplyError> {
        let selected: Vec<(String, TableId)> = bindings.map_or_else(
            || {
                self.tables
                    .iter()
                    .map(|(id, table)| (table.name.clone(), *id))
                    .collect()
            },
            |bindings| bindings.to_vec(),
        );
        selected
            .into_iter()
            .map(|(name, id)| {
                let table = self.tables.get(&id).ok_or(ApplyError::UnknownTable(id))?;
                let sheet = self.sheet(table.sheet)?;
                let row_ordinal = |row: &RowId| {
                    if engine_ordinals {
                        sheet.row_ordinals.get(row).copied()
                    } else {
                        sheet
                            .rows
                            .iter()
                            .position(|candidate| candidate == row)
                            .map(|position| position as u32)
                    }
                };
                let column_ordinal = |column: &ColumnId| {
                    if engine_ordinals {
                        sheet.column_ordinals.get(column).copied()
                    } else {
                        sheet
                            .columns
                            .iter()
                            .position(|candidate| candidate == column)
                            .map(|position| position as u32)
                    }
                };
                Ok(StructuredTable {
                    name,
                    sheet: self.sheet_ordinals[&table.sheet],
                    header_row: table.header_row.and_then(|row| row_ordinal(&row)),
                    rows: table.rows.iter().filter_map(row_ordinal).collect(),
                    columns: table
                        .columns
                        .iter()
                        .filter_map(|column| {
                            Some(StructuredColumn {
                                name: table.column_names.get(column)?.clone(),
                                column: column_ordinal(column)?,
                            })
                        })
                        .collect(),
                })
            })
            .collect()
    }

    /// Re-resolves only the cell membership of a structured formula after a
    /// table shape change. Recorded sheet/table aliases and the source remain
    /// untouched, so a later rename cannot retarget the formula.
    fn recompile_formula(
        &self,
        cell: CellRef,
        formula: &CompiledFormula,
    ) -> Result<CompiledFormula, ApplyError> {
        let mut names = HashMap::new();
        for (name, sheet) in &formula.sheet_bindings {
            names.insert(name.clone(), self.sheet_ordinals[sheet]);
        }
        let structured = self.structured_tables(Some(&formula.table_bindings), false)?;
        let current_table_name = formula
            .current_table
            .map(|id| {
                formula
                    .table_bindings
                    .iter()
                    .find(|(_, bound)| *bound == id)
                    .map(|(name, _)| name.as_str())
                    .ok_or(ApplyError::UnknownTable(id))
            })
            .transpose()?;
        let sheet = self.sheet(cell.sheet)?;
        let current_row = sheet
            .rows
            .iter()
            .position(|row| *row == cell.row)
            .map(|position| position as u32);
        let parsed = ParsedFormula::parse_with_structured_references(
            &formula.source,
            self.sheet_ordinals[&cell.sheet],
            &names,
            StructuredContext {
                tables: &structured,
                current_table: current_table_name,
                current_row,
            },
        )?;
        let view_references = parsed.references();
        if view_references.len() > MAX_FORMULA_REFERENCES {
            return Err(ApplyError::TooManyReferences);
        }
        let by_ordinal: BTreeMap<u32, SheetId> = self
            .sheet_ordinals
            .iter()
            .map(|(id, ordinal)| (*ordinal, *id))
            .collect();
        let references = view_references
            .into_iter()
            .map(|view| {
                let target = by_ordinal[&view.sheet];
                let state = &self.sheets[&target];
                let row = *state
                    .rows
                    .get(view.row as usize)
                    .ok_or_else(|| ApplyError::ReferenceOutOfView((view.row + 1).to_string()))?;
                let column = *state
                    .columns
                    .get(view.column as usize)
                    .ok_or_else(|| ApplyError::ReferenceOutOfView((view.column + 1).to_string()))?;
                Ok(CellRef {
                    sheet: target,
                    row,
                    column,
                })
            })
            .collect::<Result<Vec<_>, ApplyError>>()?;
        let mut rebound = formula.clone();
        rebound.references = references;
        Ok(rebound)
    }

    // -- apply path -------------------------------------------------------

    /// Validates `event` completely, then applies it. On `Err` the document
    /// is unchanged.
    pub fn apply(&mut self, event: &Event) -> Result<(), ApplyError> {
        if event.schema != EVENT_SCHEMA {
            return Err(ApplyError::UnsupportedSchema(event.schema));
        }
        if !event.verify() {
            return Err(ApplyError::InvalidEventId);
        }
        if event.parent != self.head {
            return Err(ApplyError::ParentMismatch {
                expected: self.head,
                observed: event.parent,
            });
        }
        let forking = matches!(event.operation, Operation::CreateBranch { .. });
        if event.branch != self.branch && !forking {
            return Err(ApplyError::BranchMismatch);
        }
        if event.actor.id.is_empty() || event.actor.id.chars().count() > MAX_ACTOR_ID_CHARS {
            return Err(ApplyError::InvalidActor);
        }
        let created = self.event_count > 0;
        match (&event.operation, created) {
            (Operation::CreateDocument { .. }, true) => {
                return Err(ApplyError::DocumentAlreadyCreated);
            }
            (Operation::CreateDocument { .. }, false) => {}
            (_, false) => return Err(ApplyError::DocumentNotCreated),
            (_, true) => {}
        }
        let provenance = Provenance {
            event: event.id,
            actor: event.actor.clone(),
            timestamp: event.timestamp,
        };
        self.apply_operation(&event.operation, provenance)?;
        self.head = Some(event.id);
        self.event_count += 1;
        Ok(())
    }

    fn apply_operation(
        &mut self,
        operation: &Operation,
        provenance: Provenance,
    ) -> Result<(), ApplyError> {
        match operation {
            Operation::CreateDocument { document, name } => {
                if *document != self.id {
                    return Err(ApplyError::DuplicateId(document.0));
                }
                check_name(name)?;
                self.name = name.clone();
                self.branches.insert(
                    self.branch,
                    BranchRecord {
                        name: "main".into(),
                        parent: None,
                        base: None,
                        created: provenance,
                    },
                );
            }
            Operation::CreateBranch { branch, name, from } => {
                check_name(name)?;
                if self.head != Some(*from) {
                    return Err(ApplyError::ForkPointMismatch);
                }
                if *branch == self.branch || self.branches.contains_key(branch) {
                    return Err(ApplyError::DuplicateId(branch.0));
                }
                if self.branches.values().any(|record| record.name == *name) {
                    return Err(ApplyError::DuplicateName(name.clone()));
                }
                self.branches.insert(
                    *branch,
                    BranchRecord {
                        name: name.clone(),
                        parent: Some(self.branch),
                        base: Some(*from),
                        created: provenance,
                    },
                );
                self.branch = *branch;
            }
            Operation::RecordMerge {
                source,
                source_head,
                replayed,
            } => {
                // The source branch's own CreateBranch event lives on the
                // source chain, so the target state need not know the branch.
                if replayed.len() > MAX_BATCH {
                    return Err(ApplyError::BatchTooLarge);
                }
                self.merges.push(MergeRecord {
                    source: *source,
                    source_head: *source_head,
                    replayed: replayed.clone(),
                    recorded: provenance,
                });
            }
            Operation::AddCheck {
                check,
                name,
                cell,
                severity,
                message,
            } => {
                check_name(name)?;
                if message.chars().count() > MAX_TEXT_CHARS {
                    return Err(ApplyError::InvalidValue);
                }
                self.check_fresh(check.0)?;
                let state = self.sheet(cell.sheet)?;
                self.check_cell_exists(state, cell)?;
                if self.checks.values().any(|record| record.name == *name) {
                    return Err(ApplyError::DuplicateName(name.clone()));
                }
                self.checks.insert(
                    *check,
                    CheckRecord {
                        name: name.clone(),
                        cell: *cell,
                        severity: *severity,
                        message: message.clone(),
                        added: provenance,
                    },
                );
            }
            Operation::RemoveCheck { check } => {
                if self.checks.remove(check).is_none() {
                    return Err(ApplyError::UnknownCheck(*check));
                }
            }
            Operation::WatchOutput { watch, name, cell } => {
                check_name(name)?;
                self.check_fresh(watch.0)?;
                let state = self.sheet(cell.sheet)?;
                self.check_cell_exists(state, cell)?;
                if self.watches.values().any(|record| record.name == *name) {
                    return Err(ApplyError::DuplicateName(name.clone()));
                }
                self.watches.insert(
                    *watch,
                    WatchRecord {
                        name: name.clone(),
                        cell: *cell,
                        added: provenance,
                    },
                );
            }
            Operation::UnwatchOutput { watch } => {
                if self.watches.remove(watch).is_none() {
                    return Err(ApplyError::UnknownWatch(*watch));
                }
            }
            Operation::AddSheet { sheet, name } => {
                check_name(name)?;
                self.check_fresh(sheet.0)?;
                if self.sheets.values().any(|state| state.name == *name) {
                    return Err(ApplyError::DuplicateName(name.clone()));
                }
                let ordinal = self.next_sheet_ordinal;
                self.next_sheet_ordinal += 1;
                self.sheet_ordinals.insert(*sheet, ordinal);
                self.sheet_order.push(*sheet);
                self.calc.define_sheet(ordinal, name.clone());
                self.sheets.insert(
                    *sheet,
                    Sheet {
                        name: name.clone(),
                        rows: Vec::new(),
                        columns: Vec::new(),
                        row_ordinals: BTreeMap::new(),
                        column_ordinals: BTreeMap::new(),
                        next_row_ordinal: 0,
                        next_column_ordinal: 0,
                        column_types: BTreeMap::new(),
                        cells: BTreeMap::new(),
                    },
                );
            }
            Operation::RenameSheet { sheet, name } => {
                check_name(name)?;
                self.sheet(*sheet)?;
                if self
                    .sheets
                    .iter()
                    .any(|(id, state)| id != sheet && state.name == *name)
                {
                    return Err(ApplyError::DuplicateName(name.clone()));
                }
                self.sheets.get_mut(sheet).expect("checked").name = name.clone();
            }
            Operation::DeleteSheet { sheet } => {
                let state = self.sheet(*sheet)?;
                for (row, column) in state.cells.keys() {
                    self.check_unreferenced_outside(
                        CellRef {
                            sheet: *sheet,
                            row: *row,
                            column: *column,
                        },
                        *sheet,
                    )?;
                }
                let cells: Vec<(RowId, ColumnId)> = state.cells.keys().copied().collect();
                for (row, column) in cells {
                    self.clear_cell(CellRef {
                        sheet: *sheet,
                        row,
                        column,
                    });
                }
                self.tables.retain(|_, table| table.sheet != *sheet);
                self.sheets.remove(sheet);
                self.sheet_order.retain(|id| id != sheet);
            }
            Operation::AddColumns { sheet, columns, at } => {
                check_batch(columns.len())?;
                let state = self.sheet(*sheet)?;
                if *at > state.columns.len() {
                    return Err(ApplyError::PositionOutOfRange);
                }
                self.check_fresh_batch(columns.iter().map(|id| id.0))?;
                let state = self.sheets.get_mut(sheet).expect("checked");
                for (offset, column) in columns.iter().enumerate() {
                    state.columns.insert(at + offset, *column);
                    state
                        .column_ordinals
                        .insert(*column, state.next_column_ordinal);
                    state.next_column_ordinal += 1;
                }
            }
            Operation::AddRows {
                sheet,
                rows,
                at,
                table,
                computed,
                rebound,
            } => {
                let before = self.clone();
                let result = (|| {
                    check_batch(rows.len())?;
                    let state = self.sheet(*sheet)?;
                    if *at > state.rows.len() {
                        return Err(ApplyError::PositionOutOfRange);
                    }
                    if let Some(table) = table {
                        let record = self
                            .tables
                            .get(table)
                            .ok_or(ApplyError::UnknownTable(*table))?;
                        if record.sheet != *sheet {
                            return Err(ApplyError::UnknownTable(*table));
                        }
                    }
                    self.check_fresh_batch(rows.iter().map(|id| id.0))?;
                    let state = self.sheets.get_mut(sheet).expect("checked");
                    for (offset, row) in rows.iter().enumerate() {
                        state.rows.insert(at + offset, *row);
                        state.row_ordinals.insert(*row, state.next_row_ordinal);
                        state.next_row_ordinal += 1;
                    }
                    if let Some(table) = table {
                        self.tables
                            .get_mut(table)
                            .expect("checked")
                            .rows
                            .extend(rows.iter().copied());
                    }
                    if table.is_none() && (!computed.is_empty() || !rebound.is_empty()) {
                        return Err(ApplyError::FormulaShapeMismatch);
                    }
                    if computed.len() > MAX_BATCH || rebound.len() > MAX_BATCH {
                        return Err(ApplyError::BatchTooLarge);
                    }
                    if let Some(table_id) = table {
                        let mut expected_rebound = Vec::new();
                        for (formula_sheet, sheet_state) in &self.sheets {
                            for ((row, column), state) in &sheet_state.cells {
                                let CellInput::Formula { formula } = &state.input else {
                                    continue;
                                };
                                if formula
                                    .table_bindings
                                    .iter()
                                    .any(|(_, bound)| bound == table_id)
                                {
                                    let cell = CellRef {
                                        sheet: *formula_sheet,
                                        row: *row,
                                        column: *column,
                                    };
                                    expected_rebound.push(ComputedCell {
                                        cell,
                                        formula: self.recompile_formula(cell, formula)?,
                                    });
                                }
                            }
                        }
                        if &expected_rebound != rebound {
                            return Err(ApplyError::FormulaShapeMismatch);
                        }
                        for item in rebound {
                            self.set_formula_state(item.cell, &item.formula, provenance.clone())?;
                        }

                        let record = self.tables.get(table_id).expect("checked").clone();
                        let expected_cells: Vec<CellRef> = rows
                            .iter()
                            .flat_map(|row| {
                                record.computed_columns.keys().map(move |column| CellRef {
                                    sheet: *sheet,
                                    row: *row,
                                    column: *column,
                                })
                            })
                            .collect();
                        if computed.iter().map(|item| item.cell).collect::<Vec<_>>()
                            != expected_cells
                        {
                            return Err(ApplyError::FormulaShapeMismatch);
                        }
                        for item in computed {
                            let source = record
                                .computed_columns
                                .get(&item.cell.column)
                                .ok_or(ApplyError::FormulaShapeMismatch)?;
                            let expected =
                                self.compile_formula_at(item.cell, Some(*table_id), source)?;
                            if expected != item.formula {
                                return Err(ApplyError::FormulaShapeMismatch);
                            }
                            self.set_formula_state(item.cell, &item.formula, provenance.clone())?;
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    *self = before;
                    return Err(error);
                }
            }
            Operation::DeleteRows { sheet, rows } => {
                check_batch(rows.len())?;
                let state = self.sheet(*sheet)?;
                for row in rows {
                    if !state.row_ordinals.contains_key(row) {
                        return Err(ApplyError::UnknownRow(*row));
                    }
                }
                let doomed: BTreeSet<RowId> = rows.iter().copied().collect();
                let cells: Vec<CellRef> = state
                    .cells
                    .keys()
                    .filter(|(row, _)| doomed.contains(row))
                    .map(|(row, column)| CellRef {
                        sheet: *sheet,
                        row: *row,
                        column: *column,
                    })
                    .collect();
                for cell in &cells {
                    self.check_unreferenced_by(cell, |dependent| doomed.contains(&dependent.row))?;
                }
                for cell in cells {
                    self.clear_cell(cell);
                }
                let state = self.sheets.get_mut(sheet).expect("checked");
                state.rows.retain(|row| !doomed.contains(row));
                for row in &doomed {
                    state.row_ordinals.remove(row);
                }
                for table in self.tables.values_mut() {
                    table.rows.retain(|row| !doomed.contains(row));
                    if table.header_row.is_some_and(|row| doomed.contains(&row)) {
                        table.header_row = None;
                    }
                }
            }
            Operation::DeleteColumns { sheet, columns } => {
                check_batch(columns.len())?;
                let state = self.sheet(*sheet)?;
                for column in columns {
                    if !state.column_ordinals.contains_key(column) {
                        return Err(ApplyError::UnknownColumn(*column));
                    }
                }
                let doomed: BTreeSet<ColumnId> = columns.iter().copied().collect();
                let cells: Vec<CellRef> = state
                    .cells
                    .keys()
                    .filter(|(_, column)| doomed.contains(column))
                    .map(|(row, column)| CellRef {
                        sheet: *sheet,
                        row: *row,
                        column: *column,
                    })
                    .collect();
                for cell in &cells {
                    self.check_unreferenced_by(cell, |dependent| {
                        doomed.contains(&dependent.column)
                    })?;
                }
                for cell in cells {
                    self.clear_cell(cell);
                }
                let state = self.sheets.get_mut(sheet).expect("checked");
                state.columns.retain(|column| !doomed.contains(column));
                for column in &doomed {
                    state.column_ordinals.remove(column);
                    state.column_types.remove(column);
                }
                for table in self.tables.values_mut() {
                    table.columns.retain(|column| !doomed.contains(column));
                    table
                        .column_names
                        .retain(|column, _| !doomed.contains(column));
                    table
                        .computed_columns
                        .retain(|column, _| !doomed.contains(column));
                }
            }
            Operation::AddTable {
                table,
                sheet,
                name,
                columns,
                column_names,
                header_row,
                rows,
            } => {
                check_name(name)?;
                self.check_fresh(table.0)?;
                let state = self.sheet(*sheet)?;
                check_batch(columns.len())?;
                if rows.len() > MAX_BATCH {
                    return Err(ApplyError::BatchTooLarge);
                }
                if columns.iter().copied().collect::<BTreeSet<_>>().len() != columns.len()
                    || rows.iter().copied().collect::<BTreeSet<_>>().len() != rows.len()
                {
                    return Err(ApplyError::InvalidValue);
                }
                for column in columns {
                    if !state.column_ordinals.contains_key(column) {
                        return Err(ApplyError::UnknownColumn(*column));
                    }
                }
                for row in rows {
                    if !state.row_ordinals.contains_key(row) {
                        return Err(ApplyError::UnknownRow(*row));
                    }
                }
                if let Some(header) = header_row {
                    if !state.row_ordinals.contains_key(header) {
                        return Err(ApplyError::UnknownRow(*header));
                    }
                    if rows.contains(header) {
                        return Err(ApplyError::InvalidValue);
                    }
                }
                if !column_names.is_empty() && column_names.len() != columns.len() {
                    return Err(ApplyError::InvalidValue);
                }
                let mut seen_names = BTreeSet::new();
                for column_name in column_names {
                    check_name(column_name)?;
                    if !seen_names.insert(column_name.to_lowercase()) {
                        return Err(ApplyError::DuplicateName(column_name.clone()));
                    }
                }
                if self
                    .tables
                    .values()
                    .any(|record| record.name.eq_ignore_ascii_case(name))
                {
                    return Err(ApplyError::DuplicateName(name.clone()));
                }
                self.tables.insert(
                    *table,
                    Table {
                        name: name.clone(),
                        sheet: *sheet,
                        columns: columns.clone(),
                        column_names: columns
                            .iter()
                            .copied()
                            .zip(column_names.iter().cloned())
                            .collect(),
                        header_row: *header_row,
                        rows: rows.clone(),
                        computed_columns: BTreeMap::new(),
                    },
                );
            }
            Operation::RenameTable { table, name } => {
                check_name(name)?;
                if !self.tables.contains_key(table) {
                    return Err(ApplyError::UnknownTable(*table));
                }
                if self
                    .tables
                    .iter()
                    .any(|(id, record)| id != table && record.name.eq_ignore_ascii_case(name))
                {
                    return Err(ApplyError::DuplicateName(name.clone()));
                }
                self.tables.get_mut(table).expect("checked").name = name.clone();
            }
            Operation::SetColumnType {
                sheet,
                column,
                column_type,
            } => {
                let state = self.sheet(*sheet)?;
                if !state.column_ordinals.contains_key(column) {
                    return Err(ApplyError::UnknownColumn(*column));
                }
                for ((_, cell_column), cell) in &state.cells {
                    if cell_column == column
                        && let CellInput::Value { value } = &cell.input
                        && !literal_matches(value, *column_type)
                    {
                        return Err(ApplyError::TypeMismatch {
                            column: *column,
                            expected: *column_type,
                        });
                    }
                }
                self.sheets
                    .get_mut(sheet)
                    .expect("checked")
                    .column_types
                    .insert(*column, *column_type);
            }
            Operation::SetComputedColumn {
                table,
                column,
                source,
                formulas,
            } => {
                let before = self.clone();
                let result = (|| {
                    if formulas.len() > MAX_BATCH {
                        return Err(ApplyError::BatchTooLarge);
                    }
                    let record = self
                        .tables
                        .get(table)
                        .ok_or(ApplyError::UnknownTable(*table))?
                        .clone();
                    if !record.columns.contains(column) {
                        return Err(ApplyError::UnknownColumn(*column));
                    }
                    let expected_cells: Vec<CellRef> = record
                        .rows
                        .iter()
                        .map(|row| CellRef {
                            sheet: record.sheet,
                            row: *row,
                            column: *column,
                        })
                        .collect();
                    if formulas.iter().map(|item| item.cell).collect::<Vec<_>>() != expected_cells {
                        return Err(ApplyError::FormulaShapeMismatch);
                    }
                    for item in formulas {
                        let expected = self.compile_formula_at(item.cell, Some(*table), source)?;
                        if expected != item.formula {
                            return Err(ApplyError::FormulaShapeMismatch);
                        }
                        self.set_formula_state(item.cell, &item.formula, provenance.clone())?;
                    }
                    self.tables
                        .get_mut(table)
                        .expect("checked")
                        .computed_columns
                        .insert(*column, source.clone());
                    Ok(())
                })();
                if let Err(error) = result {
                    *self = before;
                    return Err(error);
                }
            }
            Operation::SetValue { cell, value } => {
                check_literal(value)?;
                let state = self.sheet(cell.sheet)?;
                self.check_cell_exists(state, cell)?;
                let expected = state
                    .column_types
                    .get(&cell.column)
                    .copied()
                    .unwrap_or(ColumnType::Any);
                if !literal_matches(value, expected) {
                    return Err(ApplyError::TypeMismatch {
                        column: cell.column,
                        expected,
                    });
                }
                let engine = self.engine_cell(*cell).expect("checked");
                self.detach_dependencies(*cell);
                match value {
                    Literal::Blank => self.calc.clear(engine),
                    Literal::Number(number) => self.calc.set_number(engine, *number),
                    Literal::Text(text) => self.calc.set_text(engine, text.clone()),
                    Literal::Boolean(flag) => self.calc.set_boolean(engine, *flag),
                };
                self.sheets
                    .get_mut(&cell.sheet)
                    .expect("checked")
                    .cells
                    .insert(
                        (cell.row, cell.column),
                        CellState {
                            input: CellInput::Value {
                                value: value.clone(),
                            },
                            provenance,
                        },
                    );
            }
            Operation::SetFormula { cell, formula } => {
                self.set_formula_state(*cell, formula, provenance)?;
            }
            Operation::ClearCell { cell } => {
                let state = self.sheet(cell.sheet)?;
                self.check_cell_exists(state, cell)?;
                self.clear_cell(*cell);
            }
            Operation::Import {
                import,
                source_sha256,
                format,
            } => {
                self.check_fresh(import.0)?;
                if decode_hex(source_sha256, 32).is_none() || format.is_empty() {
                    return Err(ApplyError::InvalidValue);
                }
                check_name(format)?;
                self.imports.insert(
                    *import,
                    ImportRecord {
                        source_sha256: source_sha256.clone(),
                        format: format.clone(),
                        provenance,
                    },
                );
            }
            Operation::Tick { tick, at } => {
                let expected = self.last_tick.map_or(1, |(last, _)| last + 1);
                if *tick != expected || self.last_tick.is_some_and(|(_, last)| *at < last) {
                    return Err(ApplyError::TickNotMonotonic);
                }
                self.last_tick = Some((*tick, *at));
            }
            Operation::Propose {
                proposal,
                description,
            } => {
                self.check_fresh(proposal.0)?;
                if description.chars().count() > MAX_TEXT_CHARS {
                    return Err(ApplyError::InvalidValue);
                }
                self.proposals.insert(
                    *proposal,
                    ProposalRecord {
                        description: description.clone(),
                        status: ProposalStatus::Pending,
                        proposed: provenance,
                        resolved: None,
                        reason: None,
                    },
                );
            }
            Operation::AcceptProposal { proposal } => {
                let record = self.pending_proposal(*proposal)?;
                record.status = ProposalStatus::Accepted;
                record.resolved = Some(provenance);
            }
            Operation::RejectProposal { proposal, reason } => {
                if reason.chars().count() > MAX_TEXT_CHARS {
                    return Err(ApplyError::InvalidValue);
                }
                let record = self.pending_proposal(*proposal)?;
                record.status = ProposalStatus::Rejected;
                record.resolved = Some(provenance);
                record.reason = Some(reason.clone());
            }
        }
        Ok(())
    }

    // -- helpers ------------------------------------------------------------

    fn sheet(&self, sheet: SheetId) -> Result<&Sheet, ApplyError> {
        self.sheets
            .get(&sheet)
            .ok_or(ApplyError::UnknownSheet(sheet))
    }

    fn check_cell_exists(&self, state: &Sheet, cell: &CellRef) -> Result<(), ApplyError> {
        if !state.row_ordinals.contains_key(&cell.row) {
            return Err(ApplyError::UnknownRow(cell.row));
        }
        if !state.column_ordinals.contains_key(&cell.column) {
            return Err(ApplyError::UnknownColumn(cell.column));
        }
        Ok(())
    }

    fn known_ids(&self) -> impl Iterator<Item = ObjectId> + '_ {
        let sheets = self.sheets.iter().flat_map(|(id, sheet)| {
            std::iter::once(id.0)
                .chain(sheet.row_ordinals.keys().map(|row| row.0))
                .chain(sheet.column_ordinals.keys().map(|column| column.0))
        });
        std::iter::once(self.id.0)
            .chain(self.branches.keys().map(|id| id.0))
            .chain(sheets)
            .chain(self.tables.keys().map(|id| id.0))
            .chain(self.imports.keys().map(|id| id.0))
            .chain(self.proposals.keys().map(|id| id.0))
            .chain(self.checks.keys().map(|id| id.0))
            .chain(self.watches.keys().map(|id| id.0))
    }

    fn check_fresh(&self, id: ObjectId) -> Result<(), ApplyError> {
        if self.known_ids().any(|known| known == id) {
            return Err(ApplyError::DuplicateId(id));
        }
        Ok(())
    }

    fn check_fresh_batch(&self, ids: impl Iterator<Item = ObjectId>) -> Result<(), ApplyError> {
        let mut seen = BTreeSet::new();
        for id in ids {
            if !seen.insert(id) {
                return Err(ApplyError::DuplicateId(id));
            }
            self.check_fresh(id)?;
        }
        Ok(())
    }

    fn pending_proposal(
        &mut self,
        proposal: ProposalId,
    ) -> Result<&mut ProposalRecord, ApplyError> {
        let record = self
            .proposals
            .get_mut(&proposal)
            .ok_or(ApplyError::UnknownProposal(proposal))?;
        if record.status != ProposalStatus::Pending {
            return Err(ApplyError::ProposalNotPending(proposal));
        }
        Ok(record)
    }

    fn engine_cell(&self, cell: CellRef) -> Option<CellId> {
        let sheet = self.sheets.get(&cell.sheet)?;
        Some(CellId::new(
            self.sheet_ordinals[&cell.sheet],
            *sheet.row_ordinals.get(&cell.row)?,
            *sheet.column_ordinals.get(&cell.column)?,
        ))
    }

    /// Re-parses the recorded source purely for structure and rebinds every
    /// reference, by position, to the recorded stable identities.
    fn bind_formula(
        &self,
        cell: CellRef,
        formula: &CompiledFormula,
    ) -> Result<ParsedFormula, ApplyError> {
        let origin_ordinal = self.sheet_ordinals[&cell.sheet];
        let mut names: HashMap<String, u32> = HashMap::new();
        for (name, sheet) in &formula.sheet_bindings {
            let ordinal = *self
                .sheet_ordinals
                .get(sheet)
                .ok_or(ApplyError::UnknownSheet(*sheet))?;
            names.insert(name.clone(), ordinal);
        }
        let structured = self.structured_tables(Some(&formula.table_bindings), true)?;
        let current_table_name = formula
            .current_table
            .map(|id| {
                formula
                    .table_bindings
                    .iter()
                    .find(|(_, bound)| *bound == id)
                    .map(|(name, _)| name.as_str())
                    .ok_or(ApplyError::UnknownTable(id))
            })
            .transpose()?;
        let current_row = self
            .sheets
            .get(&cell.sheet)
            .and_then(|sheet| sheet.row_ordinals.get(&cell.row))
            .copied();
        let parsed = ParsedFormula::parse_with_structured_references(
            &formula.source,
            origin_ordinal,
            &names,
            StructuredContext {
                tables: &structured,
                current_table: current_table_name,
                current_row,
            },
        )?;
        if parsed.references().len() != formula.references.len() {
            return Err(ApplyError::FormulaShapeMismatch);
        }
        let mut engine_cells = Vec::with_capacity(formula.references.len());
        for reference in &formula.references {
            let state = self.sheet(reference.sheet)?;
            self.check_cell_exists(state, reference)?;
            engine_cells.push(self.engine_cell(*reference).expect("checked"));
        }
        let mut cursor = engine_cells.into_iter();
        Ok(parsed.map_references(|_| cursor.next().expect("length checked")))
    }

    fn attach_dependencies(&mut self, cell: CellRef, formula: &CompiledFormula) {
        for reference in &formula.references {
            self.dependents.entry(*reference).or_default().insert(cell);
        }
    }

    fn detach_dependencies(&mut self, cell: CellRef) {
        let Some(CellState {
            input: CellInput::Formula { formula },
            ..
        }) = self.cell(cell).cloned()
        else {
            return;
        };
        for reference in &formula.references {
            if let Some(dependents) = self.dependents.get_mut(reference) {
                dependents.remove(&cell);
                if dependents.is_empty() {
                    self.dependents.remove(reference);
                }
            }
        }
    }

    fn check_unreferenced_by(
        &self,
        cell: &CellRef,
        exempt: impl Fn(&CellRef) -> bool,
    ) -> Result<(), ApplyError> {
        if let Some(dependents) = self.dependents.get(cell)
            && let Some(dependent) = dependents.iter().find(|dependent| !exempt(dependent))
        {
            return Err(ApplyError::ReferencedByFormula(*dependent));
        }
        Ok(())
    }

    fn check_unreferenced_outside(&self, cell: CellRef, sheet: SheetId) -> Result<(), ApplyError> {
        self.check_unreferenced_by(&cell, |dependent| dependent.sheet == sheet)
    }

    fn clear_cell(&mut self, cell: CellRef) {
        self.detach_dependencies(cell);
        if let Some(engine) = self.engine_cell(cell) {
            self.calc.clear(engine);
        }
        if let Some(sheet) = self.sheets.get_mut(&cell.sheet) {
            sheet.cells.remove(&(cell.row, cell.column));
        }
    }

    fn set_formula_state(
        &mut self,
        cell: CellRef,
        formula: &CompiledFormula,
        provenance: Provenance,
    ) -> Result<(), ApplyError> {
        let state = self.sheet(cell.sheet)?;
        self.check_cell_exists(state, &cell)?;
        if formula.source.chars().count() > MAX_FORMULA_CHARS {
            return Err(ApplyError::FormulaTooLong);
        }
        if formula.references.len() > MAX_FORMULA_REFERENCES {
            return Err(ApplyError::TooManyReferences);
        }
        let bound = self.bind_formula(cell, formula)?;
        let engine = self.engine_cell(cell).expect("checked");
        let previous = self.cell(cell).cloned();
        self.detach_dependencies(cell);
        if let Err(error) = self.calc.set_parsed_formula(engine, bound) {
            if let Some(CellState {
                input: CellInput::Formula { formula: earlier },
                ..
            }) = &previous
            {
                self.attach_dependencies(cell, earlier);
            }
            return Err(ApplyError::Formula(error));
        }
        self.attach_dependencies(cell, formula);
        self.sheets
            .get_mut(&cell.sheet)
            .expect("checked")
            .cells
            .insert(
                (cell.row, cell.column),
                CellState {
                    input: CellInput::Formula {
                        formula: formula.clone(),
                    },
                    provenance,
                },
            );
        Ok(())
    }
}

fn check_name(name: &str) -> Result<(), ApplyError> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed != name
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(ApplyError::InvalidName);
    }
    Ok(())
}

fn check_batch(count: usize) -> Result<(), ApplyError> {
    if count == 0 {
        return Err(ApplyError::EmptyBatch);
    }
    if count > MAX_BATCH {
        return Err(ApplyError::BatchTooLarge);
    }
    Ok(())
}

fn check_literal(value: &Literal) -> Result<(), ApplyError> {
    match value {
        Literal::Number(number) if !number.is_finite() => Err(ApplyError::InvalidValue),
        Literal::Text(text) if text.chars().count() > MAX_TEXT_CHARS => {
            Err(ApplyError::InvalidValue)
        }
        _ => Ok(()),
    }
}

fn literal_matches(value: &Literal, column_type: ColumnType) -> bool {
    matches!(
        (column_type, value),
        (ColumnType::Any, _)
            | (_, Literal::Blank)
            | (ColumnType::Number, Literal::Number(_))
            | (ColumnType::Text, Literal::Text(_))
            | (ColumnType::Boolean, Literal::Boolean(_))
    )
}

/// Parses a single A1 cell address into zero-based view positions.
pub fn parse_a1(address: &str) -> Option<(usize, usize)> {
    let text = address.trim().replace('$', "").to_ascii_uppercase();
    let split = text.find(|character: char| character.is_ascii_digit())?;
    let (letters, digits) = text.split_at(split);
    if letters.is_empty()
        || digits.is_empty()
        || !letters.bytes().all(|byte| byte.is_ascii_uppercase())
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let mut column = 0_usize;
    for byte in letters.bytes() {
        column = column
            .checked_mul(26)?
            .checked_add(usize::from(byte - b'A') + 1)?;
    }
    let row = digits.parse::<usize>().ok().filter(|row| *row > 0)?;
    Some((row - 1, column - 1))
}

pub fn column_letters(mut column: usize) -> String {
    let mut letters = Vec::new();
    loop {
        letters.push(b'A' + (column % 26) as u8);
        if column < 26 {
            break;
        }
        column = column / 26 - 1;
    }
    letters.reverse();
    String::from_utf8(letters).expect("ascii")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn human() -> Actor {
        Actor::new(ActorKind::Human, "tom")
    }

    fn agent() -> Actor {
        Actor::new(ActorKind::Agent, "planner-1")
    }

    struct Fixture {
        document: Document,
        events: Vec<Event>,
        sheet: SheetId,
        clock: i64,
    }

    impl Fixture {
        fn new() -> Self {
            let (document, created) = Document::create(
                DocumentId(ObjectId::from_seed("fixture")),
                "Budget",
                human(),
                1_000,
            )
            .unwrap();
            let mut fixture = Self {
                document,
                events: vec![created],
                sheet: SheetId(ObjectId::from_seed("unset")),
                clock: 1_000,
            };
            let sheet = fixture.run(
                human(),
                Command::AddSheet {
                    name: "Summary".into(),
                },
            );
            let Operation::AddSheet { sheet, .. } = sheet.operation else {
                unreachable!()
            };
            fixture.sheet = sheet;
            fixture.run(
                human(),
                Command::AddColumns {
                    sheet,
                    count: 3,
                    at: 0,
                },
            );
            fixture.run(
                human(),
                Command::AddRows {
                    sheet,
                    count: 3,
                    at: 0,
                    table: None,
                },
            );
            fixture
        }

        fn run(&mut self, actor: Actor, command: Command) -> Event {
            self.clock += 1;
            let event = self.document.command(actor, self.clock, command).unwrap();
            self.events.push(event.clone());
            event
        }

        fn fail(&mut self, actor: Actor, command: Command) -> ApplyError {
            self.clock += 1;
            let before = self.document.digest();
            let error = self
                .document
                .command(actor, self.clock, command)
                .unwrap_err();
            assert_eq!(
                self.document.digest(),
                before,
                "failed command mutated state"
            );
            error
        }

        fn set(&mut self, a1: &str, value: f64) {
            let sheet = self.sheet;
            self.run(
                human(),
                Command::SetValue {
                    sheet,
                    a1: a1.into(),
                    value: Literal::Number(value),
                },
            );
        }

        fn formula(&mut self, a1: &str, source: &str) -> Event {
            let sheet = self.sheet;
            self.run(
                human(),
                Command::SetFormula {
                    sheet,
                    a1: a1.into(),
                    source: source.into(),
                },
            )
        }

        fn value(&self, a1: &str) -> CellValue {
            let cell = self.document.resolve_a1(self.sheet, a1).unwrap();
            self.document.value(cell)
        }
    }

    #[test]
    fn replay_reproduces_the_same_canonical_digest_and_event_ids() {
        let mut fixture = Fixture::new();
        fixture.set("A1", 2.0);
        fixture.set("A2", 3.0);
        fixture.formula("A3", "=SUM(A1:A2)*2");
        fixture.run(
            agent(),
            Command::Propose {
                description: "double the total".into(),
            },
        );

        let replayed = Document::replay(&fixture.events).unwrap();
        assert_eq!(replayed.digest(), fixture.document.digest());
        assert_eq!(replayed.head(), fixture.document.head());
        assert_eq!(replayed.event_count(), fixture.events.len() as u64);

        let json: Vec<String> = fixture.events.iter().map(Event::canonical_json).collect();
        let decoded: Vec<Event> = json
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(decoded, fixture.events);
        assert!(decoded.iter().all(Event::verify));
        assert_eq!(
            Document::replay(&decoded).unwrap().digest(),
            replayed.digest()
        );
        for pair in fixture.events.windows(2) {
            assert_eq!(pair[1].parent, Some(pair[0].id));
        }
        assert_eq!(fixture.value("A3"), CellValue::Number(10.0));
    }

    #[test]
    fn row_insertion_and_sheet_rename_preserve_formula_targets() {
        let mut fixture = Fixture::new();
        fixture.set("A1", 2.0);
        fixture.set("A2", 3.0);
        let event = fixture.formula("A3", "=A1+A2");
        let Operation::SetFormula { cell, formula } = &event.operation else {
            unreachable!()
        };
        let targets = formula.references.clone();
        let total = *cell;
        assert_eq!(fixture.value("A3"), CellValue::Number(5.0));

        let sheet = fixture.sheet;
        fixture.run(
            human(),
            Command::AddRows {
                sheet,
                count: 2,
                at: 0,
                table: None,
            },
        );
        assert_eq!(fixture.document.project_a1(total).as_deref(), Some("A5"));
        assert_eq!(
            fixture.document.project_a1(targets[0]).as_deref(),
            Some("A3")
        );
        assert_eq!(fixture.value("A5"), CellValue::Number(5.0));
        assert_eq!(fixture.value("A1"), CellValue::Blank);
        fixture.set("A1", 100.0);
        assert_eq!(fixture.value("A5"), CellValue::Number(5.0));
        fixture.set("A3", 40.0);
        assert_eq!(fixture.value("A5"), CellValue::Number(43.0));
        let stored = fixture.document.cell(total).unwrap();
        let CellInput::Formula { formula } = &stored.input else {
            unreachable!()
        };
        assert_eq!(formula.references, targets);
        assert_eq!(formula.source, "=A1+A2");

        let inputs = fixture.run(
            human(),
            Command::AddSheet {
                name: "Inputs".into(),
            },
        );
        let Operation::AddSheet { sheet: inputs, .. } = inputs.operation else {
            unreachable!()
        };
        fixture.run(
            human(),
            Command::AddColumns {
                sheet: inputs,
                count: 1,
                at: 0,
            },
        );
        fixture.run(
            human(),
            Command::AddRows {
                sheet: inputs,
                count: 1,
                at: 0,
                table: None,
            },
        );
        fixture.run(
            human(),
            Command::SetValue {
                sheet: inputs,
                a1: "A1".into(),
                value: Literal::Number(7.0),
            },
        );
        fixture.formula("B1", "=Inputs!A1*2");
        assert_eq!(fixture.value("B1"), CellValue::Number(14.0));
        fixture.run(
            human(),
            Command::RenameSheet {
                sheet: inputs,
                name: "Assumptions".into(),
            },
        );
        assert_eq!(fixture.value("B1"), CellValue::Number(14.0));
        let replayed = Document::replay(&fixture.events).unwrap();
        assert_eq!(replayed.digest(), fixture.document.digest());
        let cell = replayed.resolve_a1(sheet, "B1").unwrap();
        assert_eq!(replayed.value(cell), CellValue::Number(14.0));
    }

    #[test]
    fn invalid_events_fail_transactionally() {
        let mut fixture = Fixture::new();
        let sheet = fixture.sheet;
        fixture.set("A1", 1.0);
        fixture.formula("A2", "=A1+1");
        let a1 = fixture.document.resolve_a1(sheet, "A1").unwrap();
        let a2 = fixture.document.resolve_a1(sheet, "A2").unwrap();

        assert!(matches!(
            fixture.fail(
                human(),
                Command::SetFormula {
                    sheet,
                    a1: "A1".into(),
                    source: "=A2*2".into(),
                },
            ),
            ApplyError::Formula(FormulaError::Cycle(_))
        ));
        assert_eq!(fixture.value("A2"), CellValue::Number(2.0));
        assert_eq!(
            fixture.fail(
                human(),
                Command::DeleteRows {
                    sheet,
                    rows: vec![a1.row],
                },
            ),
            ApplyError::ReferencedByFormula(a2)
        );
        assert_eq!(
            fixture.fail(
                human(),
                Command::SetFormula {
                    sheet,
                    a1: "B1".into(),
                    source: "=A9".into(),
                },
            ),
            ApplyError::ReferenceOutOfView("A9".into())
        );
        assert_eq!(
            fixture.fail(
                human(),
                Command::SetFormula {
                    sheet,
                    a1: "B1".into(),
                    source: "=TODAY()".into(),
                },
            ),
            ApplyError::Formula(FormulaError::UnsupportedFunction("TODAY".into()))
        );
        fixture.run(
            human(),
            Command::SetColumnType {
                sheet,
                column: a1.column,
                column_type: ColumnType::Number,
            },
        );
        assert_eq!(
            fixture.fail(
                human(),
                Command::SetValue {
                    sheet,
                    a1: "A3".into(),
                    value: Literal::Text("oops".into()),
                },
            ),
            ApplyError::TypeMismatch {
                column: a1.column,
                expected: ColumnType::Number,
            }
        );
        assert_eq!(
            fixture.fail(
                human(),
                Command::SetValue {
                    sheet,
                    a1: "A3".into(),
                    value: Literal::Number(f64::NAN),
                },
            ),
            ApplyError::InvalidValue
        );
        assert_eq!(
            fixture.fail(
                human(),
                Command::AddRows {
                    sheet,
                    count: 1,
                    at: 99,
                    table: None,
                },
            ),
            ApplyError::PositionOutOfRange
        );
        assert_eq!(
            fixture.fail(
                human(),
                Command::AddSheet {
                    name: "Summary".into(),
                },
            ),
            ApplyError::DuplicateName("Summary".into())
        );
        assert_eq!(
            fixture.fail(Actor::new(ActorKind::Agent, ""), Command::Tick { at: 5 }),
            ApplyError::InvalidActor
        );

        // Deleting the dependent row first makes the deletion legal.
        fixture.run(
            human(),
            Command::DeleteRows {
                sheet,
                rows: vec![a2.row],
            },
        );
        fixture.run(
            human(),
            Command::DeleteRows {
                sheet,
                rows: vec![a1.row],
            },
        );
        assert_eq!(fixture.document.rows(sheet).unwrap().len(), 1);

        // Tampered, out-of-order and foreign-branch events are rejected.
        let mut tampered = fixture.events[2].clone();
        tampered.timestamp += 1;
        assert!(!tampered.verify());
        let mut replay = fixture.events.clone();
        replay[2] = tampered;
        assert_eq!(
            Document::replay(&replay).unwrap_err(),
            ApplyError::InvalidEventId
        );
        let mut swapped = fixture.events.clone();
        swapped.swap(2, 3);
        assert!(matches!(
            Document::replay(&swapped).unwrap_err(),
            ApplyError::ParentMismatch { .. }
        ));
        let foreign = Event::new(
            fixture.document.head(),
            BranchId(ObjectId::from_seed("other")),
            human(),
            9,
            tick_operation(),
        );
        assert_eq!(
            fixture.document.apply(&foreign).unwrap_err(),
            ApplyError::BranchMismatch
        );
    }

    fn tick_operation() -> Operation {
        Operation::Tick { tick: 1, at: 9 }
    }

    #[test]
    fn actors_ticks_imports_and_proposals_are_attributed() {
        let mut fixture = Fixture::new();
        let sheet = fixture.sheet;
        fixture.run(
            Actor::new(ActorKind::Import, "xlsx"),
            Command::Import {
                source_sha256: "a".repeat(64),
                format: "xlsx".into(),
            },
        );
        fixture.run(
            Actor::new(ActorKind::Import, "xlsx"),
            Command::SetValue {
                sheet,
                a1: "A1".into(),
                value: Literal::Number(1.0),
            },
        );
        fixture.run(
            Actor::new(ActorKind::ModelAssisted, "assistant"),
            Command::SetValue {
                sheet,
                a1: "A2".into(),
                value: Literal::Text("note".into()),
            },
        );
        let a1 = fixture.document.resolve_a1(sheet, "A1").unwrap();
        let a2 = fixture.document.resolve_a1(sheet, "A2").unwrap();
        assert_eq!(
            fixture.document.cell(a1).unwrap().provenance.actor.kind,
            ActorKind::Import
        );
        assert_eq!(
            fixture.document.cell(a2).unwrap().provenance.actor.kind,
            ActorKind::ModelAssisted
        );

        fixture.run(human(), Command::Tick { at: 5_000 });
        fixture.run(human(), Command::Tick { at: 6_000 });
        assert_eq!(
            fixture.fail(human(), Command::Tick { at: 5_500 }),
            ApplyError::TickNotMonotonic
        );
        assert_eq!(fixture.document.snapshot().last_tick, Some((2, 6_000)));

        let proposed = fixture.run(
            agent(),
            Command::Propose {
                description: "fill Q3".into(),
            },
        );
        let Operation::Propose { proposal, .. } = proposed.operation else {
            unreachable!()
        };
        assert_eq!(
            fixture.document.proposal(proposal).unwrap().status,
            ProposalStatus::Pending
        );
        fixture.run(
            human(),
            Command::RejectProposal {
                proposal,
                reason: "not yet".into(),
            },
        );
        let record = fixture.document.proposal(proposal).unwrap();
        assert_eq!(record.status, ProposalStatus::Rejected);
        assert_eq!(record.proposed.actor, agent());
        assert_eq!(record.resolved.as_ref().unwrap().actor, human());
        assert_eq!(
            fixture.fail(human(), Command::AcceptProposal { proposal }),
            ApplyError::ProposalNotPending(proposal)
        );

        let kinds: BTreeSet<ActorKind> = fixture
            .events
            .iter()
            .map(|event| event.actor.kind)
            .collect();
        assert_eq!(
            kinds,
            BTreeSet::from([
                ActorKind::Human,
                ActorKind::Import,
                ActorKind::Agent,
                ActorKind::ModelAssisted
            ])
        );
        assert_eq!(
            Document::replay(&fixture.events).unwrap().digest(),
            fixture.document.digest()
        );
    }

    #[test]
    fn tables_follow_rows_and_columns_by_identity() {
        let mut fixture = Fixture::new();
        let sheet = fixture.sheet;
        let columns = fixture.document.columns(sheet).unwrap().to_vec();
        let rows = fixture.document.rows(sheet).unwrap().to_vec();
        let event = fixture.run(
            human(),
            Command::AddTable {
                sheet,
                name: "Lines".into(),
                columns: columns[..2].to_vec(),
                column_names: vec!["Quantity".into(), "Price".into()],
                header_row: Some(rows[0]),
                rows: rows[1..].to_vec(),
            },
        );
        let Operation::AddTable { table, .. } = event.operation else {
            unreachable!()
        };
        fixture.run(
            human(),
            Command::AddRows {
                sheet,
                count: 2,
                at: 0,
                table: Some(table),
            },
        );
        assert_eq!(fixture.document.table(table).unwrap().rows.len(), 4);
        fixture.run(
            human(),
            Command::DeleteColumns {
                sheet,
                columns: vec![columns[0]],
            },
        );
        assert_eq!(
            fixture.document.table(table).unwrap().columns,
            vec![columns[1]]
        );
        assert_eq!(fixture.document.columns(sheet).unwrap().len(), 2);
        fixture.run(
            human(),
            Command::RenameTable {
                table,
                name: "Order lines".into(),
            },
        );
        assert_eq!(
            fixture.fail(
                human(),
                Command::AddTable {
                    sheet,
                    name: "Order lines".into(),
                    columns: vec![columns[1]],
                    column_names: vec!["Price".into()],
                    header_row: None,
                    rows: vec![],
                },
            ),
            ApplyError::DuplicateName("Order lines".into())
        );
        assert_eq!(
            Document::replay(&fixture.events).unwrap().digest(),
            fixture.document.digest()
        );
    }

    #[test]
    fn structured_references_and_computed_columns_follow_table_rows() {
        let mut fixture = Fixture::new();
        let sheet = fixture.sheet;
        let columns = fixture.document.columns(sheet).unwrap().to_vec();
        let rows = fixture.document.rows(sheet).unwrap().to_vec();
        fixture.run(
            human(),
            Command::AddColumns {
                sheet,
                count: 1,
                at: columns.len(),
            },
        );
        fixture.set("A2", 2.0);
        fixture.set("B2", 4.0);
        fixture.set("A3", 3.0);
        fixture.set("B3", 5.0);
        let event = fixture.run(
            human(),
            Command::AddTable {
                sheet,
                name: "Lines".into(),
                columns: columns.clone(),
                column_names: vec!["Quantity".into(), "Price".into(), "Total".into()],
                header_row: Some(rows[0]),
                rows: rows[1..].to_vec(),
            },
        );
        let Operation::AddTable { table, .. } = event.operation else {
            unreachable!()
        };
        fixture.run(
            human(),
            Command::SetComputedColumn {
                table,
                column: columns[2],
                source: "=[@Quantity]*[@Price]".into(),
            },
        );
        assert_eq!(fixture.value("C2"), CellValue::Number(8.0));
        assert_eq!(fixture.value("C3"), CellValue::Number(15.0));
        fixture.formula("D1", "=SUM(Lines[Total])");
        assert_eq!(fixture.value("D1"), CellValue::Number(23.0));

        let added = fixture.run(
            human(),
            Command::AddRows {
                sheet,
                count: 1,
                at: 3,
                table: Some(table),
            },
        );
        let Operation::AddRows {
            computed, rebound, ..
        } = &added.operation
        else {
            unreachable!()
        };
        assert_eq!(computed.len(), 1);
        assert_eq!(rebound.len(), 3);
        fixture.set("A4", 6.0);
        fixture.set("B4", 7.0);
        assert_eq!(fixture.value("C4"), CellValue::Number(42.0));
        assert_eq!(fixture.value("D1"), CellValue::Number(65.0));

        fixture.run(
            human(),
            Command::RenameTable {
                table,
                name: "Order lines".into(),
            },
        );
        assert_eq!(fixture.value("D1"), CellValue::Number(65.0));
        let replayed = Document::replay(&fixture.events).unwrap();
        assert_eq!(replayed.digest(), fixture.document.digest());
        assert_eq!(
            replayed.value(replayed.resolve_a1(sheet, "D1").unwrap()),
            CellValue::Number(65.0)
        );
    }

    #[test]
    fn branches_checks_watches_and_snapshots_round_trip() {
        let mut fixture = Fixture::new();
        let sheet = fixture.sheet;
        fixture.set("A1", 5.0);
        fixture.formula("A2", "=A1*2");
        fixture.formula("A3", "=A2>=10");
        let check = fixture.run(
            human(),
            Command::AddCheck {
                name: "doubled".into(),
                sheet,
                a1: "A3".into(),
                severity: Severity::Error,
                message: "A2 must be at least 10".into(),
            },
        );
        let Operation::AddCheck { check, .. } = check.operation else {
            unreachable!()
        };
        let watch = fixture.run(
            human(),
            Command::WatchOutput {
                name: "total".into(),
                sheet,
                a1: "A2".into(),
            },
        );
        let Operation::WatchOutput { watch, .. } = watch.operation else {
            unreachable!()
        };
        assert!(fixture.document.check_results()[0].passed);
        fixture.set("A1", 1.0);
        let results = fixture.document.check_results();
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        assert_eq!(results[0].check, check);
        assert_eq!(results[0].severity, Severity::Error);
        let a2 = fixture.document.resolve_a1(sheet, "A2").unwrap();
        let lineage = fixture.document.lineage(a2).unwrap();
        assert_eq!(lineage.kind, LineageKind::Computed);
        assert_eq!(
            lineage.inputs,
            vec![fixture.document.resolve_a1(sheet, "A1").unwrap()]
        );
        let a1 = fixture.document.resolve_a1(sheet, "A1").unwrap();
        assert_eq!(
            fixture.document.lineage(a1).unwrap().kind,
            LineageKind::Entered
        );
        assert_eq!(fixture.document.watches()[&watch].cell, a2);

        // Snapshot reconstruction reproduces the digest exactly.
        let snapshot = fixture.document.snapshot();
        let rebuilt = Document::from_snapshot(&snapshot).unwrap();
        assert_eq!(rebuilt.digest(), fixture.document.digest());
        assert_eq!(rebuilt.value(a2), CellValue::Number(2.0));
        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(
            Document::from_snapshot(&decoded).unwrap().digest(),
            rebuilt.digest()
        );
        let mut tampered = snapshot.clone();
        tampered.name = "Other".into();
        assert_ne!(
            Document::from_snapshot(&tampered).unwrap().digest(),
            rebuilt.digest()
        );

        // Forking switches the document to the new branch; later events on
        // the old branch are rejected, and the fork replays deterministically.
        let fork = fixture.document.fork("agent-work", agent(), 9_000);
        let main_branch = fixture.document.branch();
        fixture.document.apply(&fork).unwrap();
        fixture.events.push(fork.clone());
        assert_ne!(fixture.document.branch(), main_branch);
        assert_eq!(fixture.document.branches().len(), 2);
        let stale = Event::new(
            fixture.document.head(),
            main_branch,
            human(),
            9_001,
            Operation::Tick { tick: 1, at: 1 },
        );
        assert_eq!(
            fixture.document.apply(&stale).unwrap_err(),
            ApplyError::BranchMismatch
        );
        fixture.set("A1", 7.0);
        let replayed = Document::replay(&fixture.events).unwrap();
        assert_eq!(replayed.digest(), fixture.document.digest());
        assert_eq!(replayed.branch(), fixture.document.branch());
        let duplicate = Document::replay(&fixture.events[..fixture.events.len() - 2])
            .unwrap()
            .fork("agent-work", agent(), 9_000);
        assert_eq!(duplicate.id, fork.id);
        assert_eq!(
            fixture.fail(
                human(),
                Command::RemoveCheck {
                    check: CheckId(ObjectId::from_seed("nope")),
                },
            ),
            ApplyError::UnknownCheck(CheckId(ObjectId::from_seed("nope")))
        );
        fixture.run(human(), Command::RemoveCheck { check });
        assert!(fixture.document.check_results().is_empty());
        assert_eq!(
            Operation::SetValue {
                cell: a1,
                value: Literal::Blank
            }
            .touches(),
            vec![Touch::Cell { cell: a1 }]
        );
    }

    #[test]
    fn identities_are_deterministic_and_text_safe() {
        let a = ObjectId::derive(b"seed", "row", 1);
        let b = ObjectId::derive(b"seed", "row", 1);
        let c = ObjectId::derive(b"seed", "row", 2);
        let d = ObjectId::derive(b"seed", "column", 1);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert_eq!(a.to_string().len(), 32);
        assert_eq!(ObjectId::parse(&a.to_string()), Some(a));
        assert_eq!(ObjectId::parse("zz"), None);
        assert_eq!(parse_a1("$B$3"), Some((2, 1)));
        assert_eq!(parse_a1("AA10"), Some((9, 26)));
        assert_eq!(parse_a1("A0"), None);
        assert_eq!(parse_a1("1A"), None);
        assert_eq!(column_letters(0), "A");
        assert_eq!(column_letters(25), "Z");
        assert_eq!(column_letters(26), "AA");
        assert_eq!(column_letters(701), "ZZ");
        assert_eq!(column_letters(702), "AAA");
    }
}
