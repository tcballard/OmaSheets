//! Owned M0 calculation experiments for OmaSheets.
//!
//! This crate deliberately starts with a narrow Excel-compatible surface. It
//! proves the dependency and incremental-recalculation semantics independently
//! of the candidate import engine; it is not yet the installed product engine.
//!
//! Dates are Excel 1900-system serial numbers; see [`serial_date`] for the
//! boundary rules and the deliberately unsupported cases.

pub mod serial_date;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;

/// Defined names may refer to other names; deeper chains are rejected.
const MAX_NAME_DEPTH: usize = 8;

const MAX_RANGE_CELLS: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CellId {
    pub sheet: u32,
    pub row: u32,
    pub column: u32,
}

impl CellId {
    pub const fn new(sheet: u32, row: u32, column: u32) -> Self {
        Self { sheet, row, column }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Blank,
    Number(f64),
    Boolean(bool),
    Text(String),
    Error(CalcError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalcError {
    /// Excel `#DIV/0!`.
    DivisionByZero,
    /// Excel `#REF!`.
    InvalidReference,
    /// Excel `#N/A`: a lookup found nothing, or `NA()`.
    NotAvailable,
    /// Excel `#VALUE!`.
    InvalidValue,
    /// Excel `#NUM!`: a numeric argument outside the function's domain.
    InvalidNumber,
    /// Excel `#NAME?`, only ever imported from a source workbook or written
    /// as a literal; unknown names fail to compile instead.
    InvalidName,
    /// Excel `#NULL!`, only ever imported or written as a literal.
    NullIntersection,
    /// Wrong argument count or shape for the function.
    InvalidArguments,
}

impl CalcError {
    /// Excel's spelling of the error.
    pub fn label(&self) -> &'static str {
        match self {
            Self::DivisionByZero => "#DIV/0!",
            Self::InvalidReference => "#REF!",
            Self::NotAvailable => "#N/A",
            Self::InvalidValue => "#VALUE!",
            Self::InvalidNumber => "#NUM!",
            Self::InvalidName => "#NAME?",
            Self::NullIntersection => "#NULL!",
            Self::InvalidArguments => "#ARGS!",
        }
    }

    fn parse_literal(text: &str) -> Option<Self> {
        Some(match text {
            "#DIV/0!" => Self::DivisionByZero,
            "#REF!" => Self::InvalidReference,
            "#N/A" => Self::NotAvailable,
            "#VALUE!" => Self::InvalidValue,
            "#NUM!" => Self::InvalidNumber,
            "#NAME?" => Self::InvalidName,
            "#NULL!" => Self::NullIntersection,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FormulaError {
    Empty,
    UnexpectedToken(usize),
    UnsupportedFunction(String),
    InvalidReference(String),
    UnknownSheet(String),
    /// A reference into another workbook (`[1]Sheet!A1`); never evaluated.
    ExternalReference(String),
    /// A bare identifier that is neither a cell reference nor a defined name.
    UnknownName(String),
    /// A defined name whose definition the engine cannot compile.
    UnsupportedName(String),
    RangeTooLarge,
    Cycle(Vec<CellId>),
}

impl fmt::Display for FormulaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "formula is empty"),
            Self::UnexpectedToken(offset) => {
                write!(formatter, "unexpected formula token at byte {offset}")
            }
            Self::UnsupportedFunction(name) => write!(formatter, "unsupported function {name}"),
            Self::InvalidReference(reference) => write!(formatter, "invalid reference {reference}"),
            Self::UnknownSheet(sheet) => write!(formatter, "unknown sheet {sheet}"),
            Self::ExternalReference(reference) => {
                write!(formatter, "external workbook reference {reference}")
            }
            Self::UnknownName(name) => write!(formatter, "unknown name {name}"),
            Self::UnsupportedName(name) => write!(formatter, "unsupported defined name {name}"),
            Self::RangeTooLarge => write!(formatter, "formula range exceeds the M0 safety bound"),
            Self::Cycle(path) => write!(formatter, "formula introduces a cycle: {path:?}"),
        }
    }
}

impl std::error::Error for FormulaError {}

#[derive(Clone, Debug, PartialEq)]
enum Expr<R = CellId> {
    Number(f64),
    Boolean(bool),
    Text(String),
    /// An error literal such as `#REF!`.
    Error(CalcError),
    /// An omitted argument, as in `IF(x,,y)`; evaluates to blank.
    Empty,
    Reference(R),
    UnaryMinus(Box<Expr<R>>),
    Percent(Box<Expr<R>>),
    Binary(BinaryOp, Box<Expr<R>>, Box<Expr<R>>),
    /// A rectangle of cells anchored at its top-left cell, or, after a
    /// rebinding that no longer forms a rectangle, an explicit member list in
    /// row-major order. Only parsed formulas (`R = CellId`) carry this
    /// variant; compiling turns it into a shared [`Expr::RangeNode`].
    Range {
        anchor: R,
        members: Option<Vec<R>>,
        rows: usize,
        columns: usize,
    },
    /// A compiled range: one shared node in the workbook graph whose
    /// dependencies are the member cells. Every formula over the same cells
    /// points at the same node, so a range costs its members once, not once
    /// per formula.
    RangeNode {
        node: usize,
        rows: usize,
        columns: usize,
    },
    Function(Function, Vec<Expr<R>>),
}

/// Cells of a rectangle in row-major order.
fn rectangle_cells(anchor: CellId, rows: usize, columns: usize) -> impl Iterator<Item = CellId> {
    (0..rows).flat_map(move |row| {
        (0..columns).map(move |column| {
            CellId::new(
                anchor.sheet,
                anchor.row + row as u32,
                anchor.column + column as u32,
            )
        })
    })
}

/// Identity of a shared range node: the rectangle it covers, or its explicit
/// member list when a rebinding broke the rectangle.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum RangeKey {
    Rectangle {
        anchor: CellId,
        rows: usize,
        columns: usize,
    },
    Members {
        members: Vec<CellId>,
        rows: usize,
        columns: usize,
    },
}

fn range_key(
    anchor: CellId,
    members: Option<Vec<CellId>>,
    rows: usize,
    columns: usize,
) -> RangeKey {
    match members {
        None => RangeKey::Rectangle {
            anchor,
            rows,
            columns,
        },
        Some(members) => RangeKey::Members {
            members,
            rows,
            columns,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
    Concat,
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Function {
    Sum,
    Average,
    Min,
    Max,
    Count,
    CountA,
    Product,
    Abs,
    Round,
    RoundUp,
    RoundDown,
    Int,
    Mod,
    Power,
    Sqrt,
    If,
    And,
    Or,
    Not,
    IfError,
    Sign,
    Ceiling,
    Floor,
    Trunc,
    Exp,
    Ln,
    Log,
    Log10,
    Pi,
    Len,
    Left,
    Right,
    Mid,
    Trim,
    Upper,
    Lower,
    Concat,
    Value,
    Exact,
    CountIf,
    SumIf,
    CountIfs,
    SumIfs,
    AverageIf,
    AverageIfs,
    Index,
    Match,
    VLookup,
    XLookup,
    Date,
    Year,
    Month,
    Day,
    EDate,
    EoMonth,
    Weekday,
    YearFrac,
    Days360,
    NetworkDays,
    WorkDay,
    Lookup,
    Pmt,
    Npv,
    Xnpv,
    Xirr,
    NormDist,
    AverageA,
    Correl,
    IsBlank,
    IsNumber,
    IsText,
    IsLogical,
    IsError,
    N,
    T,
    SumProduct,
    Median,
    Choose,
    SubTotal,
    StDev,
    StDevP,
    Var,
    VarP,
    Na,
    IsNa,
    HLookup,
    Find,
    Rept,
    Row,
    Column,
}

/// A parsed formula whose cell references can be enumerated and rebound
/// before it is installed, so callers that own stable object identities can
/// bind A1 input syntax once and replay against engine cells later.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedFormula {
    expression: Expr<CellId>,
}

impl ParsedFormula {
    /// Parses `source` as entered in `sheet`, resolving sheet names through
    /// `sheet_names` (lower-cased name to sheet index), without touching any
    /// workbook.
    pub fn parse(
        source: &str,
        sheet: u32,
        sheet_names: &HashMap<String, u32>,
    ) -> Result<Self, FormulaError> {
        let lowered: HashMap<String, u32> = sheet_names
            .iter()
            .map(|(name, index)| (name.to_lowercase(), *index))
            .collect();
        // Stand-alone parsing has no workbook, so no defined names resolve;
        // a name reference fails with `UnknownName`, as it would in a
        // workbook without definitions.
        let defined_names = HashMap::new();
        Parser::new(source, sheet, &lowered, &defined_names)
            .parse()
            .map(|expression| Self { expression })
    }

    /// Every cell reference in deterministic traversal order, including each
    /// cell of an expanded range. The same order is used by
    /// [`ParsedFormula::map_references`].
    pub fn references(&self) -> Vec<CellId> {
        let mut output = Vec::new();
        visit_references(&self.expression, &mut |cell| output.push(cell));
        output
    }

    /// Rebinds every reference in the order [`ParsedFormula::references`]
    /// reports them.
    pub fn map_references(mut self, mut map: impl FnMut(CellId) -> CellId) -> Self {
        rebind_references(&mut self.expression, &mut map);
        self
    }
}

fn visit_references(expression: &Expr<CellId>, visit: &mut impl FnMut(CellId)) {
    match expression {
        Expr::Reference(cell) => visit(*cell),
        Expr::UnaryMinus(inner) | Expr::Percent(inner) => visit_references(inner, visit),
        Expr::Binary(_, left, right) => {
            visit_references(left, visit);
            visit_references(right, visit);
        }
        Expr::Range {
            anchor,
            members,
            rows,
            columns,
        } => match members {
            Some(members) => members.iter().for_each(|cell| visit(*cell)),
            None => rectangle_cells(*anchor, *rows, *columns).for_each(visit),
        },
        Expr::Function(_, items) => {
            for item in items {
                visit_references(item, visit);
            }
        }
        Expr::RangeNode { .. }
        | Expr::Number(_)
        | Expr::Boolean(_)
        | Expr::Text(_)
        | Expr::Error(_)
        | Expr::Empty => {}
    }
}

fn rebind_references(expression: &mut Expr<CellId>, map: &mut impl FnMut(CellId) -> CellId) {
    match expression {
        Expr::Reference(cell) => *cell = map(*cell),
        Expr::UnaryMinus(inner) | Expr::Percent(inner) => rebind_references(inner, map),
        Expr::Binary(_, left, right) => {
            rebind_references(left, map);
            rebind_references(right, map);
        }
        Expr::Range {
            anchor,
            members,
            rows,
            columns,
        } => {
            let mapped: Vec<CellId> = match members {
                Some(members) => members.iter().map(|cell| map(*cell)).collect(),
                None => rectangle_cells(*anchor, *rows, *columns)
                    .map(&mut *map)
                    .collect(),
            };
            let first = mapped[0];
            let rectangular = mapped.iter().enumerate().all(|(index, cell)| {
                *cell
                    == CellId::new(
                        first.sheet,
                        first.row + (index / *columns) as u32,
                        first.column + (index % *columns) as u32,
                    )
            });
            *anchor = first;
            *members = if rectangular { None } else { Some(mapped) };
        }
        Expr::Function(_, items) => {
            for item in items {
                rebind_references(item, map);
            }
        }
        Expr::RangeNode { .. }
        | Expr::Number(_)
        | Expr::Boolean(_)
        | Expr::Text(_)
        | Expr::Error(_)
        | Expr::Empty => {}
    }
}

#[derive(Clone, Debug)]
enum Input {
    Literal(Value),
    Formula(Expr<usize>),
    /// A shared range node: its `dependents` are the formulas that read it
    /// and it carries no value of its own. A rectangle keeps no per-member
    /// edges at all; membership is decided by position through the sheet
    /// index, so a range costs nothing per cell it covers. An explicit member
    /// list (a rebound range that no longer forms a rectangle) keeps its
    /// members as `dependencies` in row-major order.
    Range {
        shape: RangeShape,
    },
}

#[derive(Clone, Copy, Debug)]
enum RangeShape {
    Rectangle {
        anchor: CellId,
        rows: usize,
        columns: usize,
    },
    Members,
}

/// Rows per bucket of the rectangle index: a changed cell only checks the
/// rectangles that touch its band.
const RANGE_BAND_ROWS: u32 = 256;

#[derive(Clone, Debug)]
struct Cell {
    id: CellId,
    input: Input,
    dependencies: Vec<usize>,
    dependents: Vec<usize>,
    value: Value,
}

/// Sizes of the calculation graph, for memory diagnostics. Counts are exact;
/// `expression_nodes` walks every compiled formula.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GraphStatistics {
    pub cells: usize,
    pub formula_cells: usize,
    pub range_nodes: usize,
    pub dependency_edges: usize,
    pub dependent_edges: usize,
    pub dependency_capacity: usize,
    pub dependent_capacity: usize,
    pub largest_dependents: usize,
    pub expression_nodes: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecalcReport {
    /// Changed cell plus downstream formula cells evaluated in this pass.
    pub evaluated: Vec<CellId>,
}

pub struct Workbook {
    indices: HashMap<CellId, usize>,
    /// Cells of each sheet keyed by position, so a rectangle can be scanned
    /// for the cells that exist inside it without materialising blanks.
    sheet_cells: HashMap<u32, BTreeMap<(u32, u32), usize>>,
    /// Rectangle range nodes bucketed by sheet and row band, so a changed
    /// cell finds the ranges that cover it without per-member edges.
    range_bands: HashMap<(u32, u32), Vec<usize>>,
    cells: Vec<Cell>,
    dirty_marks: Vec<u64>,
    pending: Vec<usize>,
    sheet_names: HashMap<String, u32>,
    defined_names: HashMap<String, String>,
    /// Shared range nodes by identity, so every formula over the same cells
    /// shares one node and one set of member edges.
    ranges: HashMap<RangeKey, usize>,
    generation: u64,
    /// While a bulk load is open, commits record their cells here instead of
    /// recalculating, and `end_bulk` evaluates everything once.
    bulk: Option<Vec<usize>>,
    /// The cell whose formula is being evaluated, for implicit intersection
    /// and `ROW()`/`COLUMN()`. Evaluation is sequential, so a plain cell is
    /// enough.
    evaluating: std::cell::Cell<CellId>,
}

impl Default for Workbook {
    fn default() -> Self {
        Self {
            indices: HashMap::new(),
            sheet_cells: HashMap::new(),
            range_bands: HashMap::new(),
            cells: Vec::new(),
            dirty_marks: Vec::new(),
            pending: Vec::new(),
            sheet_names: HashMap::new(),
            defined_names: HashMap::new(),
            ranges: HashMap::new(),
            generation: 1,
            bulk: None,
            evaluating: std::cell::Cell::new(CellId::new(0, 0, 0)),
        }
    }
}

impl Workbook {
    pub fn define_sheet(&mut self, index: u32, name: impl Into<String>) {
        self.sheet_names.insert(name.into().to_lowercase(), index);
    }

    /// Registers a workbook-level defined name. The definition is compiled
    /// lazily, where the name is used, with the same parser as formulas; a
    /// definition that does not compile makes every use an explicit
    /// `UnsupportedName` failure. The first definition of a name wins.
    pub fn define_name(&mut self, name: impl Into<String>, definition: impl Into<String>) {
        self.defined_names
            .entry(name.into().to_lowercase())
            .or_insert_with(|| definition.into());
    }

    pub fn set_error(&mut self, cell: CellId, error: CalcError) -> RecalcReport {
        self.commit(cell, Input::Literal(Value::Error(error)), Vec::new())
    }

    pub fn value(&self, cell: CellId) -> Value {
        self.indices
            .get(&cell)
            .map(|index| self.cells[*index].value.clone())
            .unwrap_or(Value::Blank)
    }

    pub fn set_number(&mut self, cell: CellId, value: f64) -> RecalcReport {
        self.commit(cell, Input::Literal(Value::Number(value)), Vec::new())
    }

    pub fn set_boolean(&mut self, cell: CellId, value: bool) -> RecalcReport {
        self.commit(cell, Input::Literal(Value::Boolean(value)), Vec::new())
    }

    pub fn set_text(&mut self, cell: CellId, value: impl Into<String>) -> RecalcReport {
        self.commit(cell, Input::Literal(Value::Text(value.into())), Vec::new())
    }

    pub fn clear(&mut self, cell: CellId) -> RecalcReport {
        self.commit(cell, Input::Literal(Value::Blank), Vec::new())
    }

    pub fn set_formula(
        &mut self,
        cell: CellId,
        formula: &str,
    ) -> Result<RecalcReport, FormulaError> {
        let expression =
            Parser::new(formula, cell.sheet, &self.sheet_names, &self.defined_names).parse()?;
        self.set_parsed_formula(cell, ParsedFormula { expression })
    }

    /// Installs an already parsed (and possibly rebound) formula with the same
    /// transactional cycle rejection as [`Workbook::set_formula`].
    pub fn set_parsed_formula(
        &mut self,
        cell: CellId,
        formula: ParsedFormula,
    ) -> Result<RecalcReport, FormulaError> {
        let parsed = formula.expression;
        let mut cells = BTreeSet::new();
        let mut range_keys = Vec::new();
        collect_dependencies(&parsed, &mut cells, &mut range_keys);
        let mut range_nodes: HashMap<RangeKey, usize> = HashMap::new();
        for key in range_keys {
            let node = self.ensure_range(&key);
            range_nodes.insert(key, node);
        }
        if let Some(path) = self.prospective_cycle(cell, &cells, range_nodes.values().copied()) {
            return Err(FormulaError::Cycle(path));
        }
        let mut dependencies: Vec<usize> = cells
            .into_iter()
            .map(|dependency| self.ensure_cell(dependency))
            .collect();
        dependencies.extend(range_nodes.values().copied());
        dependencies.sort_unstable();
        dependencies.dedup();
        let expression = compile_expression(parsed, &self.indices, &range_nodes);
        Ok(self.commit(cell, Input::Formula(expression), dependencies))
    }

    /// Returns the shared node for `key`, creating it on first use. A node is
    /// a graph vertex like a cell: formulas that read the range depend on it,
    /// and a change inside the rectangle reaches it through the band index.
    fn ensure_range(&mut self, key: &RangeKey) -> usize {
        if let Some(node) = self.ranges.get(key) {
            return *node;
        }
        let node = self.cells.len();
        let cell = match key {
            RangeKey::Rectangle {
                anchor,
                rows,
                columns,
            } => {
                let last_row = anchor.row + (*rows as u32 - 1);
                for band in anchor.row / RANGE_BAND_ROWS..=last_row / RANGE_BAND_ROWS {
                    self.range_bands
                        .entry((anchor.sheet, band))
                        .or_default()
                        .push(node);
                }
                Cell {
                    id: *anchor,
                    input: Input::Range {
                        shape: RangeShape::Rectangle {
                            anchor: *anchor,
                            rows: *rows,
                            columns: *columns,
                        },
                    },
                    dependencies: Vec::new(),
                    dependents: Vec::new(),
                    value: Value::Blank,
                }
            }
            RangeKey::Members { members, .. } => {
                let indices: Vec<usize> = members
                    .iter()
                    .map(|member| self.ensure_cell(*member))
                    .collect();
                for member in &indices {
                    self.cells[*member].dependents.push(node);
                }
                Cell {
                    id: members[0],
                    input: Input::Range {
                        shape: RangeShape::Members,
                    },
                    dependencies: indices,
                    dependents: Vec::new(),
                    value: Value::Blank,
                }
            }
        };
        self.cells.push(cell);
        self.dirty_marks.push(0);
        self.pending.push(0);
        self.ranges.insert(key.clone(), node);
        node
    }

    fn range_shape(&self, node: usize) -> RangeShape {
        match self.cells[node].input {
            Input::Range { shape } => shape,
            _ => unreachable!("range nodes hold a range shape"),
        }
    }

    fn rectangle_covers(&self, node: usize, cell: CellId) -> bool {
        match self.range_shape(node) {
            RangeShape::Rectangle {
                anchor,
                rows,
                columns,
            } => {
                cell.sheet == anchor.sheet
                    && cell.row >= anchor.row
                    && cell.row < anchor.row + rows as u32
                    && cell.column >= anchor.column
                    && cell.column < anchor.column + columns as u32
            }
            RangeShape::Members => false,
        }
    }

    /// Rectangle nodes covering `cell`, through the band index.
    fn covering_nodes(&self, cell: CellId) -> impl Iterator<Item = usize> + '_ {
        self.range_bands
            .get(&(cell.sheet, cell.row / RANGE_BAND_ROWS))
            .into_iter()
            .flatten()
            .copied()
            .filter(move |node| self.rectangle_covers(*node, cell))
    }

    /// Graph successors of `index`: explicit dependents plus, for a cell,
    /// every rectangle that covers it.
    fn for_each_successor(&self, index: usize, mut visit: impl FnMut(usize)) {
        for dependent in &self.cells[index].dependents {
            visit(*dependent);
        }
        if !matches!(self.cells[index].input, Input::Range { .. }) {
            for node in self.covering_nodes(self.cells[index].id) {
                visit(node);
            }
        }
    }

    pub fn statistics(&self) -> GraphStatistics {
        let mut statistics = GraphStatistics::default();
        for cell in &self.cells {
            match &cell.input {
                Input::Range { .. } => statistics.range_nodes += 1,
                Input::Formula(expression) => {
                    statistics.cells += 1;
                    statistics.formula_cells += 1;
                    statistics.expression_nodes += count_nodes(expression);
                }
                Input::Literal(_) => statistics.cells += 1,
            }
            statistics.dependency_edges += cell.dependencies.len();
            statistics.dependent_edges += cell.dependents.len();
            statistics.dependency_capacity += cell.dependencies.capacity();
            statistics.dependent_capacity += cell.dependents.capacity();
            statistics.largest_dependents =
                statistics.largest_dependents.max(cell.dependents.len());
        }
        statistics
    }

    fn range_len(&self, node: usize) -> usize {
        match self.range_shape(node) {
            RangeShape::Rectangle { rows, columns, .. } => rows * columns,
            RangeShape::Members => self.cells[node].dependencies.len(),
        }
    }

    /// The cell at a row-major position inside the range, if it exists.
    fn range_cell(&self, node: usize, index: usize) -> Option<usize> {
        match self.range_shape(node) {
            RangeShape::Rectangle {
                anchor, columns, ..
            } => self
                .indices
                .get(&CellId::new(
                    anchor.sheet,
                    anchor.row + (index / columns) as u32,
                    anchor.column + (index % columns) as u32,
                ))
                .copied(),
            RangeShape::Members => Some(self.cells[node].dependencies[index]),
        }
    }

    fn range_value(&self, node: usize, index: usize) -> Value {
        self.range_cell(node, index)
            .map_or(Value::Blank, |cell| self.cells[cell].value.clone())
    }

    /// Every position of the range in row-major order with the cell that
    /// exists there, found by scanning the sheet index rather than by
    /// probing each position.
    /// Visits every existing cell inside a rectangle as (row-major position,
    /// cell index), scanning the sheet index rather than probing positions.
    fn for_each_rectangle_cell(
        &self,
        anchor: CellId,
        rows: usize,
        columns: usize,
        mut visit: impl FnMut(usize, usize),
    ) {
        let Some(sheet) = self.sheet_cells.get(&anchor.sheet) else {
            return;
        };
        let last_row = anchor.row + (rows as u32 - 1);
        let last_column = anchor.column + (columns as u32 - 1);
        let position = |row: u32, column: u32| {
            (row - anchor.row) as usize * columns + (column - anchor.column) as usize
        };
        if columns <= 8 && rows <= 4096 {
            for row in anchor.row..=last_row {
                for ((_, column), index) in sheet.range((row, anchor.column)..=(row, last_column)) {
                    visit(position(row, *column), *index);
                }
            }
        } else {
            for ((row, column), index) in sheet.range((anchor.row, 0)..=(last_row, u32::MAX)) {
                if *column >= anchor.column && *column <= last_column {
                    visit(position(*row, *column), *index);
                }
            }
        }
    }

    /// Every position of the range in row-major order with the cell that
    /// exists there.
    fn range_cells(&self, node: usize) -> Vec<Option<usize>> {
        match self.range_shape(node) {
            RangeShape::Rectangle {
                anchor,
                rows,
                columns,
            } => {
                let mut output = vec![None; rows * columns];
                self.for_each_rectangle_cell(anchor, rows, columns, |position, index| {
                    output[position] = Some(index);
                });
                output
            }
            RangeShape::Members => self.cells[node]
                .dependencies
                .iter()
                .map(|member| Some(*member))
                .collect(),
        }
    }

    /// Every value of the range in row-major order, blank where no cell
    /// exists, in one allocation.
    fn range_values(&self, node: usize) -> Vec<Value> {
        match self.range_shape(node) {
            RangeShape::Rectangle {
                anchor,
                rows,
                columns,
            } => {
                let mut output = vec![Value::Blank; rows * columns];
                self.for_each_rectangle_cell(anchor, rows, columns, |position, index| {
                    output[position] = self.cells[index].value.clone();
                });
                output
            }
            RangeShape::Members => self.cells[node]
                .dependencies
                .iter()
                .map(|member| self.cells[*member].value.clone())
                .collect(),
        }
    }

    fn ensure_cell(&mut self, cell: CellId) -> usize {
        if let Some(index) = self.indices.get(&cell) {
            return *index;
        }
        let index = self.cells.len();
        self.indices.insert(cell, index);
        self.sheet_cells
            .entry(cell.sheet)
            .or_default()
            .insert((cell.row, cell.column), index);
        self.cells.push(Cell {
            id: cell,
            input: Input::Literal(Value::Blank),
            dependencies: Vec::new(),
            dependents: Vec::new(),
            value: Value::Blank,
        });
        self.dirty_marks.push(0);
        self.pending.push(0);
        index
    }

    fn commit(&mut self, cell: CellId, input: Input, dependencies: Vec<usize>) -> RecalcReport {
        let changed = self.ensure_cell(cell);
        let previous = std::mem::take(&mut self.cells[changed].dependencies);
        for dependency in previous {
            self.cells[dependency]
                .dependents
                .retain(|dependent| *dependent != changed);
        }
        for dependency in &dependencies {
            self.cells[*dependency].dependents.push(changed);
        }
        let entry = &mut self.cells[changed];
        entry.input = input;
        entry.dependencies = dependencies;
        entry.value = Value::Blank;
        if let Some(pending) = &mut self.bulk {
            pending.push(changed);
            return RecalcReport::default();
        }
        self.recalculate(&[changed])
    }

    /// Starts a bulk load: every edit until [`Workbook::end_bulk`] updates
    /// the graph without evaluating, so an import of many cells costs one
    /// recalculation instead of one per cell. Values read during a bulk load
    /// are blank; cycles are still rejected per formula.
    pub fn begin_bulk(&mut self) {
        if self.bulk.is_none() {
            self.bulk = Some(Vec::new());
        }
    }

    /// Ends a bulk load and evaluates every cell it touched, with their
    /// dependents, in one topological pass.
    pub fn end_bulk(&mut self) -> RecalcReport {
        match self.bulk.take() {
            Some(seeds) => self.recalculate(&seeds),
            None => RecalcReport::default(),
        }
    }

    fn prospective_cycle(
        &self,
        changed: CellId,
        dependencies: &BTreeSet<CellId>,
        range_nodes: impl Iterator<Item = usize>,
    ) -> Option<Vec<CellId>> {
        if dependencies.contains(&changed) {
            return Some(vec![changed, changed]);
        }
        let range_nodes: Vec<usize> = range_nodes.collect();
        // A formula inside one of its own rectangles is a cycle even when
        // the cell does not exist yet, since a rectangle has no member edges
        // to walk.
        if range_nodes
            .iter()
            .any(|node| self.rectangle_covers(*node, changed))
        {
            return Some(vec![changed, changed]);
        }
        let targets: HashSet<usize> = dependencies
            .iter()
            .filter_map(|dependency| self.indices.get(dependency).copied())
            .chain(range_nodes)
            .collect();
        if targets.is_empty() {
            return None;
        }
        // Roots: the changed cell when it exists, otherwise the rectangles
        // that will cover it once it does. A new cell nothing covers cannot
        // close a cycle, and skipping the walk keeps building cheap.
        let roots: Vec<usize> = match self.indices.get(&changed) {
            Some(index) => vec![*index],
            None => self.covering_nodes(changed).collect(),
        };
        if roots.is_empty() {
            return None;
        }
        let mut parents = vec![usize::MAX; self.cells.len()];
        let mut queue = VecDeque::new();
        for root in roots {
            parents[root] = root;
            queue.push_back(root);
        }
        while let Some(current) = queue.pop_front() {
            if targets.contains(&current) {
                let mut path = vec![current];
                let mut cursor = current;
                while parents[cursor] != cursor {
                    cursor = parents[cursor];
                    path.push(cursor);
                }
                path.reverse();
                // Range nodes are not cells; the path names the member cell
                // through which the cycle enters the range.
                let mut cells: Vec<CellId> = path
                    .into_iter()
                    .filter(|index| !matches!(self.cells[*index].input, Input::Range { .. }))
                    .map(|index| self.cells[index].id)
                    .collect();
                if cells.first() != Some(&changed) {
                    cells.insert(0, changed);
                }
                cells.push(changed);
                return Some(cells);
            }
            self.for_each_successor(current, |successor| {
                if parents[successor] == usize::MAX {
                    parents[successor] = current;
                    queue.push_back(successor);
                }
            });
        }
        None
    }

    fn recalculate(&mut self, seeds: &[usize]) -> RecalcReport {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.dirty_marks.fill(0);
            self.generation = 1;
        }
        let generation = self.generation;
        let mut dirty = Vec::with_capacity(seeds.len());
        for seed in seeds {
            if self.dirty_marks[*seed] != generation {
                self.dirty_marks[*seed] = generation;
                dirty.push(*seed);
            }
        }
        let mut cursor = 0;
        // Rectangle nodes have no member edges: their pending count is the
        // number of dirty cells they cover, accumulated while marking.
        let mut rectangle_pending: HashMap<usize, usize> = HashMap::new();
        while let Some(index) = dirty.get(cursor).copied() {
            cursor += 1;
            for dependent in self.cells[index].dependents.iter().copied() {
                if self.dirty_marks[dependent] != generation {
                    self.dirty_marks[dependent] = generation;
                    dirty.push(dependent);
                }
            }
            if !matches!(self.cells[index].input, Input::Range { .. }) {
                let covering: Vec<usize> = self.covering_nodes(self.cells[index].id).collect();
                for node in covering {
                    if self.dirty_marks[node] != generation {
                        self.dirty_marks[node] = generation;
                        dirty.push(node);
                    }
                    *rectangle_pending.entry(node).or_default() += 1;
                }
            }
        }

        for index in &dirty {
            self.pending[*index] = match rectangle_pending.get(index) {
                Some(covered) => *covered,
                None => self.cells[*index]
                    .dependencies
                    .iter()
                    .filter(|dependency| self.dirty_marks[**dependency] == generation)
                    .count(),
            };
        }
        let mut ready: VecDeque<usize> = dirty
            .iter()
            .copied()
            .filter(|index| self.pending[*index] == 0)
            .collect();
        let mut evaluated = Vec::with_capacity(dirty.len());
        let mut range_nodes_passed = 0;
        let mut released = Vec::new();

        while let Some(cell_index) = ready.pop_front() {
            let value = match &self.cells[cell_index].input {
                Input::Literal(value) => Some(value.clone()),
                Input::Formula(expression) => {
                    self.evaluating.set(self.cells[cell_index].id);
                    Some(match self.evaluate(expression) {
                        // A formula whose result is an empty reference shows 0.
                        Value::Blank => Value::Number(0.0),
                        value => value,
                    })
                }
                // A range node only orders its members before its readers.
                Input::Range { .. } => None,
            };
            match value {
                Some(value) => {
                    self.cells[cell_index].value = value;
                    evaluated.push(self.cells[cell_index].id);
                }
                None => range_nodes_passed += 1,
            }
            released.clear();
            self.for_each_successor(cell_index, |successor| {
                if self.dirty_marks[successor] == generation {
                    released.push(successor);
                }
            });
            for successor in released.iter().copied() {
                let remaining = &mut self.pending[successor];
                *remaining -= 1;
                if *remaining == 0 {
                    ready.push_back(successor);
                }
            }
        }
        debug_assert_eq!(
            evaluated.len() + range_nodes_passed,
            dirty.len(),
            "cycles are rejected before commit"
        );
        RecalcReport { evaluated }
    }

    fn evaluate(&self, expression: &Expr<usize>) -> Value {
        match expression {
            Expr::Number(value) => Value::Number(*value),
            Expr::Boolean(value) => Value::Boolean(*value),
            Expr::Text(value) => Value::Text(value.clone()),
            Expr::Error(error) => Value::Error(error.clone()),
            Expr::Empty => Value::Blank,
            Expr::Reference(index) => self.cells[*index].value.clone(),
            Expr::UnaryMinus(inner) => match self.evaluate(inner) {
                Value::Number(value) => Value::Number(-value),
                Value::Blank => Value::Number(-0.0),
                Value::Boolean(value) => Value::Number(if value { -1.0 } else { -0.0 }),
                Value::Text(_) => Value::Error(CalcError::InvalidValue),
                other => other,
            },
            Expr::Percent(inner) => match number(self.evaluate(inner)) {
                Ok(value) => Value::Number(value / 100.0),
                Err(error) => Value::Error(error),
            },
            Expr::Binary(operator, left, right) => {
                let left = self.evaluate(left);
                let right = self.evaluate(right);
                apply_binary(*operator, left, right)
            }
            Expr::RangeNode {
                node,
                rows,
                columns,
            } => self.implicit_intersection(*node, *rows, *columns),
            Expr::Range { .. } => unreachable!("parsed ranges are compiled to range nodes"),
            Expr::Function(function, arguments) => self.evaluate_function(*function, arguments),
        }
    }

    /// A range used where one value is expected picks the cell in the
    /// evaluating formula's row (vertical range), column (horizontal range)
    /// or both, as Excel's implicit intersection does; otherwise `#VALUE!`.
    fn implicit_intersection(&self, node: usize, rows: usize, columns: usize) -> Value {
        if rows == 1 && columns == 1 {
            return self.range_value(node, 0);
        }
        let origin = self.evaluating.get();
        let chosen = match self.range_shape(node) {
            RangeShape::Rectangle { anchor, .. } => {
                let row_offset = if rows == 1 {
                    Some(0)
                } else {
                    origin
                        .row
                        .checked_sub(anchor.row)
                        .map(|offset| offset as usize)
                        .filter(|offset| *offset < rows)
                };
                let column_offset = if columns == 1 {
                    Some(0)
                } else {
                    origin
                        .column
                        .checked_sub(anchor.column)
                        .map(|offset| offset as usize)
                        .filter(|offset| *offset < columns)
                };
                row_offset
                    .zip(column_offset)
                    .map(|(row, column)| row * columns + column)
            }
            RangeShape::Members => self.cells[node].dependencies.iter().position(|member| {
                let id = self.cells[*member].id;
                (columns == 1 || id.column == origin.column) && (rows == 1 || id.row == origin.row)
            }),
        };
        match chosen {
            Some(index) => self.range_value(node, index),
            None => Value::Error(CalcError::InvalidValue),
        }
    }

    fn evaluate_function(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        if function == Function::If {
            if !matches!(arguments.len(), 2 | 3) {
                return Value::Error(CalcError::InvalidArguments);
            }
            return match truthy(self.evaluate(&arguments[0])) {
                Ok(true) => self.evaluate(&arguments[1]),
                Ok(false) if arguments.len() == 3 => self.evaluate(&arguments[2]),
                Ok(false) => Value::Boolean(false),
                Err(error) => Value::Error(error),
            };
        }
        if function == Function::Choose {
            if arguments.len() < 2 {
                return Value::Error(CalcError::InvalidArguments);
            }
            return match number(self.evaluate(&arguments[0])) {
                Ok(index) if index >= 1.0 && (index.trunc() as usize) < arguments.len() => {
                    self.evaluate(&arguments[index.trunc() as usize])
                }
                Ok(_) => Value::Error(CalcError::InvalidValue),
                Err(error) => Value::Error(error),
            };
        }
        if function == Function::SubTotal {
            return self.evaluate_subtotal(arguments);
        }
        if matches!(function, Function::Row | Function::Column) {
            return self.evaluate_position_function(function, arguments);
        }
        if function == Function::IfError {
            if arguments.len() != 2 {
                return Value::Error(CalcError::InvalidArguments);
            }
            let value = self.evaluate(&arguments[0]);
            return if matches!(value, Value::Error(_)) {
                self.evaluate(&arguments[1])
            } else {
                value
            };
        }
        if matches!(
            function,
            Function::CountIf
                | Function::SumIf
                | Function::CountIfs
                | Function::SumIfs
                | Function::AverageIf
                | Function::AverageIfs
        ) {
            return self.evaluate_criteria_function(function, arguments);
        }
        if matches!(
            function,
            Function::Index
                | Function::Match
                | Function::VLookup
                | Function::HLookup
                | Function::XLookup
        ) {
            return self.evaluate_lookup_function(function, arguments);
        }
        if matches!(
            function,
            Function::Date
                | Function::Year
                | Function::Month
                | Function::Day
                | Function::EDate
                | Function::EoMonth
                | Function::Weekday
        ) {
            return self.evaluate_date_function(function, arguments);
        }
        if matches!(
            function,
            Function::IsBlank
                | Function::IsNumber
                | Function::IsText
                | Function::IsLogical
                | Function::IsError
                | Function::IsNa
                | Function::N
                | Function::T
        ) {
            return self.evaluate_inspection_function(function, arguments);
        }
        if function == Function::Na {
            return if arguments.is_empty() {
                Value::Error(CalcError::NotAvailable)
            } else {
                Value::Error(CalcError::InvalidArguments)
            };
        }
        if function == Function::SumProduct {
            return self.evaluate_sumproduct(arguments);
        }
        if matches!(
            function,
            Function::YearFrac | Function::Days360 | Function::NetworkDays | Function::WorkDay
        ) {
            return self.evaluate_calendar_function(function, arguments);
        }
        if function == Function::Lookup {
            return self.evaluate_lookup_vector(arguments);
        }
        if matches!(
            function,
            Function::Pmt | Function::Npv | Function::Xnpv | Function::Xirr
        ) {
            return self.evaluate_financial_function(function, arguments);
        }
        if function == Function::Correl {
            return self.evaluate_correl(arguments);
        }

        let mut values = Vec::new();
        for argument in arguments {
            self.flatten_values(argument, &mut values);
        }
        if let Some(error) = values.iter().find_map(|value| match value {
            Value::Error(error) => Some(error.clone()),
            _ => None,
        }) {
            return Value::Error(error);
        }
        let numbers: Vec<f64> = values
            .iter()
            .filter_map(|value| match value {
                Value::Number(number) => Some(*number),
                _ => None,
            })
            .collect();

        match function {
            Function::Sum => Value::Number(numbers.iter().sum()),
            Function::Average if !numbers.is_empty() => {
                Value::Number(numbers.iter().sum::<f64>() / numbers.len() as f64)
            }
            Function::Average => Value::Error(CalcError::DivisionByZero),
            Function::Min => Value::Number(numbers.into_iter().reduce(f64::min).unwrap_or(0.0)),
            Function::Max => Value::Number(numbers.into_iter().reduce(f64::max).unwrap_or(0.0)),
            Function::Count => Value::Number(numbers.len() as f64),
            Function::CountA => Value::Number(
                values
                    .iter()
                    .filter(|value| !matches!(value, Value::Blank))
                    .count() as f64,
            ),
            Function::Product => Value::Number(numbers.into_iter().product()),
            Function::Median if !numbers.is_empty() => median(numbers),
            Function::NormDist => normal_distribution(&values),
            Function::AverageA => average_a(&values),
            Function::StDev => deviation(&numbers, true, true),
            Function::StDevP => deviation(&numbers, false, true),
            Function::Var => deviation(&numbers, true, false),
            Function::VarP => deviation(&numbers, false, false),
            Function::Find => find_text(&values),
            Function::Rept => repeat_text(&values),
            Function::Abs => unary_number(&values, f64::abs),
            Function::Int => unary_number(&values, f64::floor),
            Function::Sqrt => {
                unary_number(
                    &values,
                    |value| {
                        if value < 0.0 { f64::NAN } else { value.sqrt() }
                    },
                )
            }
            Function::Round => round_function(&values, RoundDirection::Nearest),
            Function::RoundUp => round_function(&values, RoundDirection::AwayFromZero),
            Function::RoundDown => round_function(&values, RoundDirection::TowardZero),
            Function::Mod => binary_number(&values, |left, right| {
                if right == 0.0 {
                    None
                } else {
                    Some(left.rem_euclid(right))
                }
            }),
            Function::Power => binary_number(&values, |left, right| Some(left.powf(right))),
            Function::Sign => unary_number(&values, |value| value.signum()),
            Function::Ceiling => binary_number(&values, |value, significance| {
                (significance != 0.0).then(|| (value / significance).ceil() * significance)
            }),
            Function::Floor => binary_number(&values, |value, significance| {
                (significance != 0.0).then(|| (value / significance).floor() * significance)
            }),
            Function::Trunc => trunc_function(&values),
            Function::Exp => unary_number(&values, f64::exp),
            Function::Ln => unary_number(
                &values,
                |value| {
                    if value > 0.0 { value.ln() } else { f64::NAN }
                },
            ),
            Function::Log => log_function(&values),
            Function::Log10 => {
                unary_number(
                    &values,
                    |value| {
                        if value > 0.0 { value.log10() } else { f64::NAN }
                    },
                )
            }
            Function::Pi if values.is_empty() => Value::Number(std::f64::consts::PI),
            Function::Len => text_length(&values),
            Function::Left => text_edge(&values, true),
            Function::Right => text_edge(&values, false),
            Function::Mid => text_mid(&values),
            Function::Trim => text_unary(&values, |value| {
                value.split_whitespace().collect::<Vec<_>>().join(" ")
            }),
            Function::Upper => text_unary(&values, |value| value.to_uppercase()),
            Function::Lower => text_unary(&values, |value| value.to_lowercase()),
            Function::Concat => concatenate(&values),
            Function::Value => parse_text_number(&values),
            Function::Exact => exact_text(&values),
            Function::And => logical_fold(&values, true, |left, right| left && right),
            Function::Or => logical_fold(&values, false, |left, right| left || right),
            Function::Not if values.len() == 1 => match truthy(values[0].clone()) {
                Ok(value) => Value::Boolean(!value),
                Err(error) => Value::Error(error),
            },
            Function::Not
            | Function::If
            | Function::IfError
            | Function::Pi
            | Function::CountIf
            | Function::SumIf
            | Function::CountIfs
            | Function::SumIfs
            | Function::AverageIf
            | Function::AverageIfs
            | Function::Index
            | Function::Match
            | Function::VLookup
            | Function::XLookup
            | Function::Lookup
            | Function::YearFrac
            | Function::Days360
            | Function::NetworkDays
            | Function::WorkDay
            | Function::Pmt
            | Function::Npv
            | Function::Xnpv
            | Function::Xirr
            | Function::Correl
            | Function::Date
            | Function::Year
            | Function::Month
            | Function::Day
            | Function::EDate
            | Function::EoMonth
            | Function::Weekday
            | Function::IsBlank
            | Function::IsNumber
            | Function::IsText
            | Function::IsLogical
            | Function::IsError
            | Function::N
            | Function::T
            | Function::SumProduct
            | Function::Median
            | Function::Choose
            | Function::SubTotal
            | Function::Na
            | Function::IsNa
            | Function::HLookup
            | Function::Row
            | Function::Column => Value::Error(CalcError::InvalidArguments),
        }
    }

    /// `SUBTOTAL(code, ref, ...)`: codes 1-11 and 101-111 select the
    /// aggregate; cells that themselves hold a `SUBTOTAL` formula are skipped
    /// as in Excel. Hidden-row semantics (101-111) are not modelled: the
    /// engine has no row visibility, so both ranges behave like 1-11.
    fn evaluate_subtotal(&self, arguments: &[Expr<usize>]) -> Value {
        if arguments.len() < 2 {
            return Value::Error(CalcError::InvalidArguments);
        }
        let code = match number(self.evaluate(&arguments[0])) {
            Ok(code) => code.trunc() as i64,
            Err(error) => return Value::Error(error),
        };
        let code = if (101..=111).contains(&code) {
            code - 100
        } else {
            code
        };
        let function = match code {
            1 => Function::Average,
            2 => Function::Count,
            3 => Function::CountA,
            4 => Function::Max,
            5 => Function::Min,
            6 => Function::Product,
            7 => Function::StDev,
            8 => Function::StDevP,
            9 => Function::Sum,
            10 => Function::Var,
            11 => Function::VarP,
            _ => return Value::Error(CalcError::InvalidValue),
        };
        let mut values = Vec::new();
        for argument in &arguments[1..] {
            self.flatten_values_skipping_subtotals(argument, &mut values);
        }
        let literals: Vec<Expr<usize>> = values
            .into_iter()
            .map(|value| match value {
                Value::Number(number) => Expr::Number(number),
                Value::Boolean(flag) => Expr::Boolean(flag),
                Value::Text(text) => Expr::Text(text),
                Value::Error(error) => Expr::Error(error),
                Value::Blank => Expr::Empty,
            })
            .collect();
        self.evaluate_function(function, &literals)
    }

    fn flatten_values_skipping_subtotals(&self, expression: &Expr<usize>, output: &mut Vec<Value>) {
        match expression {
            Expr::RangeNode { node, .. } => {
                for member in self.range_cells(*node) {
                    match member {
                        Some(member)
                            if matches!(
                                self.cells[member].input,
                                Input::Formula(Expr::Function(Function::SubTotal, _))
                            ) => {}
                        Some(member) => output.push(self.cells[member].value.clone()),
                        None => output.push(Value::Blank),
                    }
                }
            }
            Expr::Reference(index) => {
                if let Input::Formula(Expr::Function(Function::SubTotal, _)) =
                    &self.cells[*index].input
                {
                    return;
                }
                output.push(self.cells[*index].value.clone());
            }
            Expr::Empty => {}
            other => output.push(self.evaluate(other)),
        }
    }

    /// `ROW()` and `COLUMN()` report the evaluating cell; with a reference
    /// they report its first cell. One-based, as in Excel.
    fn evaluate_position_function(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        let id = match arguments {
            [] => self.evaluating.get(),
            [Expr::Reference(index)] => self.cells[*index].id,
            [Expr::RangeNode { node, .. }] => self.cells[*node].id,
            _ => return Value::Error(CalcError::InvalidArguments),
        };
        Value::Number(
            f64::from(if function == Function::Row {
                id.row
            } else {
                id.column
            }) + 1.0,
        )
    }

    /// Type inspection takes exactly one scalar argument and never propagates
    /// an error from it: `ISERROR(1/0)` is `TRUE`, `ISBLANK(1/0)` is `FALSE`.
    /// `N` and `T` do propagate errors, matching Excel.
    fn evaluate_inspection_function(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        if arguments.len() != 1 || matches!(arguments[0], Expr::RangeNode { .. }) {
            return Value::Error(CalcError::InvalidArguments);
        }
        let value = self.evaluate(&arguments[0]);
        match function {
            Function::IsBlank => Value::Boolean(matches!(value, Value::Blank)),
            Function::IsNumber => Value::Boolean(matches!(value, Value::Number(_))),
            Function::IsText => Value::Boolean(matches!(value, Value::Text(_))),
            Function::IsLogical => Value::Boolean(matches!(value, Value::Boolean(_))),
            Function::IsError => Value::Boolean(matches!(value, Value::Error(_))),
            Function::IsNa => {
                Value::Boolean(matches!(value, Value::Error(CalcError::NotAvailable)))
            }
            Function::N => match value {
                Value::Number(number) => Value::Number(number),
                Value::Boolean(true) => Value::Number(1.0),
                Value::Boolean(false) | Value::Blank | Value::Text(_) => Value::Number(0.0),
                error @ Value::Error(_) => error,
            },
            Function::T => match value {
                text @ Value::Text(_) => text,
                Value::Number(_) | Value::Boolean(_) | Value::Blank => Value::Text(String::new()),
                error @ Value::Error(_) => error,
            },
            _ => unreachable!("dispatched above"),
        }
    }

    /// `SUMPRODUCT(range, ...)`: every argument must have the same shape;
    /// non-numeric entries count as zero and errors propagate.
    fn evaluate_sumproduct(&self, arguments: &[Expr<usize>]) -> Value {
        if arguments.is_empty() {
            return Value::Error(CalcError::InvalidArguments);
        }
        let shape = |expression: &Expr<usize>| match expression {
            Expr::RangeNode { rows, columns, .. } => (*rows, *columns),
            _ => (1, 1),
        };
        let expected = shape(&arguments[0]);
        if arguments.iter().any(|argument| shape(argument) != expected) {
            return Value::Error(CalcError::InvalidValue);
        }
        let mut products = vec![1.0; expected.0 * expected.1];
        let mut values = Vec::with_capacity(products.len());
        for argument in arguments {
            values.clear();
            self.flatten_values(argument, &mut values);
            for (product, value) in products.iter_mut().zip(&values) {
                *product *= match value {
                    Value::Number(number) => *number,
                    Value::Error(error) => return Value::Error(error.clone()),
                    _ => 0.0,
                };
            }
        }
        Value::Number(products.iter().sum())
    }

    /// Date functions take scalar serial arguments and return serials; every
    /// calendar rule lives in [`serial_date`].
    fn evaluate_date_function(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        let expected_arity: &[usize] = match function {
            Function::Date => &[3],
            Function::Year | Function::Month | Function::Day => &[1],
            Function::EDate | Function::EoMonth => &[2],
            Function::Weekday => &[1, 2],
            _ => unreachable!("dispatched above"),
        };
        if !expected_arity.contains(&arguments.len()) {
            return Value::Error(CalcError::InvalidArguments);
        }
        let mut numbers = Vec::with_capacity(arguments.len());
        for argument in arguments {
            match date_number(self.evaluate(argument)) {
                Ok(value) => numbers.push(value),
                Err(error) => return Value::Error(error),
            }
        }
        let result = match function {
            Function::Date => serial_date::date_serial(numbers[0], numbers[1], numbers[2]),
            Function::Year | Function::Month | Function::Day => {
                serial_date::serial_from_number(numbers[0])
                    .and_then(serial_date::civil_from_serial)
                    .map(|date| match function {
                        Function::Year => date.year,
                        Function::Month => i64::from(date.month),
                        _ => i64::from(date.day),
                    })
            }
            Function::EDate | Function::EoMonth => {
                match (
                    serial_date::serial_from_number(numbers[0]),
                    serial_offset(numbers[1]),
                ) {
                    (Ok(start), Ok(months)) if function == Function::EDate => {
                        serial_date::add_months(start, months)
                    }
                    (Ok(start), Ok(months)) => serial_date::end_of_month(start, months),
                    (Err(error), _) | (_, Err(error)) => Err(error),
                }
            }
            Function::Weekday => {
                match (
                    serial_date::serial_from_number(numbers[0]),
                    numbers.get(1).copied().map_or(Ok(1), serial_offset),
                ) {
                    (Ok(serial), Ok(return_type)) => serial_date::weekday(serial, return_type),
                    (Err(error), _) | (_, Err(error)) => Err(error),
                }
            }
            _ => unreachable!("dispatched above"),
        };
        match result {
            Ok(value) => Value::Number(value as f64),
            Err(error) => Value::Error(error),
        }
    }

    /// Serials from a holiday argument: numbers truncate, blanks are skipped,
    /// anything else is `#VALUE!`.
    fn holiday_serials(&self, argument: &Expr<usize>) -> Result<HashSet<i64>, CalcError> {
        let mut values = Vec::new();
        self.flatten_values(argument, &mut values);
        let mut serials = HashSet::new();
        for value in values {
            match value {
                Value::Blank => {}
                Value::Number(number) => {
                    serials.insert(serial_date::serial_from_number(number)?);
                }
                Value::Error(error) => return Err(error),
                Value::Text(_) | Value::Boolean(_) => return Err(CalcError::InvalidValue),
            }
        }
        Ok(serials)
    }

    /// `YEARFRAC`, `DAYS360`, `NETWORKDAYS` and `WORKDAY`: day-count and
    /// working-day arithmetic on the 1900 serial system.
    fn evaluate_calendar_function(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        let expected_arity: &[usize] = match function {
            Function::YearFrac | Function::Days360 | Function::NetworkDays | Function::WorkDay => {
                &[2, 3]
            }
            _ => unreachable!("dispatched above"),
        };
        if !expected_arity.contains(&arguments.len()) {
            return Value::Error(CalcError::InvalidArguments);
        }
        let first = match date_number(self.evaluate(&arguments[0]))
            .and_then(serial_date::serial_from_number)
        {
            Ok(serial) => serial,
            Err(error) => return Value::Error(error),
        };
        let second = match number(self.evaluate(&arguments[1])) {
            Ok(value) => value,
            Err(error) => return Value::Error(error),
        };
        let result = match function {
            Function::YearFrac => {
                let basis = arguments.get(2).map_or(Ok(0), |argument| {
                    number(self.evaluate(argument)).and_then(serial_offset)
                });
                serial_date::serial_from_number(second)
                    .and_then(|end| {
                        basis.and_then(|basis| serial_date::year_fraction(first, end, basis))
                    })
                    .map(Value::Number)
            }
            Function::Days360 => {
                let european = arguments
                    .get(2)
                    .map_or(Ok(false), |argument| truthy(self.evaluate(argument)));
                serial_date::serial_from_number(second)
                    .and_then(|end| {
                        european.and_then(|european| serial_date::days_360(first, end, european))
                    })
                    .map(|days| Value::Number(days as f64))
            }
            Function::NetworkDays => {
                let holidays = arguments.get(2).map_or(Ok(HashSet::new()), |argument| {
                    self.holiday_serials(argument)
                });
                serial_date::serial_from_number(second).and_then(|end| {
                    holidays.map(|holidays| {
                        Value::Number(serial_date::network_days(first, end, &holidays) as f64)
                    })
                })
            }
            Function::WorkDay => {
                let holidays = arguments.get(2).map_or(Ok(HashSet::new()), |argument| {
                    self.holiday_serials(argument)
                });
                serial_offset(second)
                    .and_then(|days| {
                        holidays.and_then(|holidays| serial_date::work_day(first, days, &holidays))
                    })
                    .map(|serial| Value::Number(serial as f64))
            }
            _ => unreachable!("dispatched above"),
        };
        match result {
            Ok(value) => value,
            Err(error) => Value::Error(error),
        }
    }

    /// `LOOKUP(value, lookup_vector, [result_vector])` in vector form, and the
    /// array form that searches the first row or column of a rectangle.
    fn evaluate_lookup_vector(&self, arguments: &[Expr<usize>]) -> Value {
        if !matches!(arguments.len(), 2 | 3) {
            return Value::Error(CalcError::InvalidArguments);
        }
        let lookup = self.evaluate(&arguments[0]);
        if matches!(lookup, Value::Error(_)) {
            return lookup;
        }
        let Some((node, rows, columns)) = range_parts(&arguments[1]) else {
            return Value::Error(CalcError::InvalidArguments);
        };
        let (candidates, results): (Vec<Value>, Vec<Value>) = if arguments.len() == 3 {
            let Some((result_node, result_rows, result_columns)) = range_parts(&arguments[2])
            else {
                return Value::Error(CalcError::InvalidArguments);
            };
            if (rows != 1 && columns != 1)
                || (result_rows != 1 && result_columns != 1)
                || rows * columns != result_rows * result_columns
            {
                return Value::Error(CalcError::InvalidArguments);
            }
            (self.range_values(node), self.range_values(result_node))
        } else if rows == 1 || columns == 1 {
            let values = self.range_values(node);
            (values.clone(), values)
        } else if columns > rows {
            // More columns than rows: search the first row, answer from the last.
            let candidates = (0..columns)
                .map(|column| self.range_value(node, column))
                .collect();
            let results = (0..columns)
                .map(|column| self.range_value(node, (rows - 1) * columns + column))
                .collect();
            (candidates, results)
        } else {
            let candidates = (0..rows)
                .map(|row| self.range_value(node, row * columns))
                .collect();
            let results = (0..rows)
                .map(|row| self.range_value(node, row * columns + columns - 1))
                .collect();
            (candidates, results)
        };
        match match_position(&lookup, &candidates, MatchMode::Ascending) {
            Ok(position) => results[position].clone(),
            Err(error) => Value::Error(error),
        }
    }

    /// `PMT`, `NPV`, `XNPV` and `XIRR`.
    fn evaluate_financial_function(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        let result = match function {
            Function::Pmt => {
                if !matches!(arguments.len(), 3..=5) {
                    return Value::Error(CalcError::InvalidArguments);
                }
                let mut numbers = Vec::with_capacity(5);
                for argument in arguments {
                    match number(self.evaluate(argument)) {
                        Ok(value) => numbers.push(value),
                        Err(error) => return Value::Error(error),
                    }
                }
                payment(
                    numbers[0],
                    numbers[1],
                    numbers[2],
                    numbers.get(3).copied().unwrap_or(0.0),
                    numbers.get(4).copied().unwrap_or(0.0) != 0.0,
                )
            }
            Function::Npv => {
                if arguments.len() < 2 {
                    return Value::Error(CalcError::InvalidArguments);
                }
                let rate = match number(self.evaluate(&arguments[0])) {
                    Ok(value) => value,
                    Err(error) => return Value::Error(error),
                };
                let mut values = Vec::new();
                for argument in &arguments[1..] {
                    self.flatten_values(argument, &mut values);
                }
                if let Some(error) = first_error(&values) {
                    return Value::Error(error);
                }
                net_present_value(rate, &numeric_only(&values))
            }
            Function::Xnpv | Function::Xirr => {
                if !matches!(
                    (function, arguments.len()),
                    (Function::Xnpv, 3) | (Function::Xirr, 2 | 3)
                ) {
                    return Value::Error(CalcError::InvalidArguments);
                }
                let (values_argument, dates_argument) = if function == Function::Xnpv {
                    (&arguments[1], &arguments[2])
                } else {
                    (&arguments[0], &arguments[1])
                };
                let mut values = Vec::new();
                self.flatten_values(values_argument, &mut values);
                let mut dates = Vec::new();
                self.flatten_values(dates_argument, &mut dates);
                let cash_flows = match dated_cash_flows(&values, &dates) {
                    Ok(cash_flows) => cash_flows,
                    Err(error) => return Value::Error(error),
                };
                if function == Function::Xnpv {
                    match number(self.evaluate(&arguments[0])) {
                        Ok(rate) => dated_net_present_value(rate, &cash_flows),
                        Err(error) => Err(error),
                    }
                } else {
                    let guess = arguments
                        .get(2)
                        .map_or(Ok(0.1), |argument| number(self.evaluate(argument)));
                    guess.and_then(|guess| internal_rate_of_return(&cash_flows, guess))
                }
            }
            _ => unreachable!("dispatched above"),
        };
        match result {
            Ok(value) => Value::Number(value),
            Err(error) => Value::Error(error),
        }
    }

    /// `CORREL(array1, array2)`: Pearson correlation over the positions where
    /// both sides are numbers.
    fn evaluate_correl(&self, arguments: &[Expr<usize>]) -> Value {
        if arguments.len() != 2 {
            return Value::Error(CalcError::InvalidArguments);
        }
        let mut left = Vec::new();
        self.flatten_values(&arguments[0], &mut left);
        let mut right = Vec::new();
        self.flatten_values(&arguments[1], &mut right);
        if let Some(error) = first_error(&left).or_else(|| first_error(&right)) {
            return Value::Error(error);
        }
        if left.len() != right.len() {
            return Value::Error(CalcError::NotAvailable);
        }
        let pairs: Vec<(f64, f64)> = left
            .iter()
            .zip(&right)
            .filter_map(|(x, y)| match (x, y) {
                (Value::Number(x), Value::Number(y)) => Some((*x, *y)),
                _ => None,
            })
            .collect();
        match correlation(&pairs) {
            Ok(value) => Value::Number(value),
            Err(error) => Value::Error(error),
        }
    }

    fn flatten_values(&self, expression: &Expr<usize>, output: &mut Vec<Value>) {
        match expression {
            Expr::RangeNode { node, .. } => output.extend(self.range_values(*node)),
            Expr::Empty => {}
            other => output.push(self.evaluate(other)),
        }
    }

    fn evaluate_criteria_function(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        let (output_expression, criteria_arguments) = match function {
            Function::CountIf if arguments.len() == 2 => (None, arguments),
            Function::SumIf | Function::AverageIf if matches!(arguments.len(), 2 | 3) => {
                let output = arguments.get(2).unwrap_or(&arguments[0]);
                (Some(output), &arguments[..2])
            }
            Function::CountIfs if arguments.len() >= 2 && arguments.len().is_multiple_of(2) => {
                (None, arguments)
            }
            Function::SumIfs | Function::AverageIfs
                if arguments.len() >= 3 && arguments.len() % 2 == 1 =>
            {
                (Some(&arguments[0]), &arguments[1..])
            }
            _ => return Value::Error(CalcError::InvalidArguments),
        };

        let mut criteria = Vec::with_capacity(criteria_arguments.len() / 2);
        for pair in criteria_arguments.chunks_exact(2) {
            let mut range = Vec::new();
            self.flatten_values(&pair[0], &mut range);
            let criterion = self.evaluate(&pair[1]);
            if matches!(criterion, Value::Error(_)) {
                return criterion;
            }
            criteria.push((range, criterion));
        }
        let Some(length) = criteria.first().map(|(range, _)| range.len()) else {
            return Value::Error(CalcError::InvalidArguments);
        };
        if criteria.iter().any(|(range, _)| range.len() != length) {
            return Value::Error(CalcError::InvalidArguments);
        }

        let mut output_values = vec![Value::Blank; length];
        if let Some(expression) = output_expression {
            output_values.clear();
            self.flatten_values(expression, &mut output_values);
            if output_values.len() != length {
                return Value::Error(CalcError::InvalidArguments);
            }
        }
        let mut count = 0_usize;
        let mut numeric_count = 0_usize;
        let mut sum = 0.0;
        for (index, output_value) in output_values.into_iter().enumerate() {
            let mut matched = true;
            for (range, criterion) in &criteria {
                match criterion_matches(range[index].clone(), criterion.clone()) {
                    Ok(true) => {}
                    Ok(false) => {
                        matched = false;
                        break;
                    }
                    Err(error) => return Value::Error(error),
                }
            }
            if !matched {
                continue;
            }
            count += 1;
            match output_value {
                Value::Number(value) => {
                    sum += value;
                    numeric_count += 1;
                }
                Value::Error(error) => return Value::Error(error),
                _ => {}
            }
        }
        match function {
            Function::CountIf | Function::CountIfs => Value::Number(count as f64),
            Function::SumIf | Function::SumIfs => Value::Number(sum),
            Function::AverageIf | Function::AverageIfs if numeric_count > 0 => {
                Value::Number(sum / numeric_count as f64)
            }
            Function::AverageIf | Function::AverageIfs => Value::Error(CalcError::DivisionByZero),
            _ => unreachable!("validated above"),
        }
    }

    fn evaluate_lookup_function(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        match function {
            Function::Index if matches!(arguments.len(), 2 | 3) => {
                let Some((node, rows, columns)) = range_parts(&arguments[0]) else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                let Ok(row) = positive_index(self.evaluate(&arguments[1])) else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                let column = if arguments.len() == 3 {
                    match positive_index(self.evaluate(&arguments[2])) {
                        Ok(column) => column,
                        Err(error) => return Value::Error(error),
                    }
                } else if columns == 1 {
                    1
                } else if rows == 1 {
                    return if row <= columns {
                        self.range_value(node, row - 1)
                    } else {
                        Value::Error(CalcError::InvalidReference)
                    };
                } else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                if row > rows || column > columns {
                    return Value::Error(CalcError::InvalidReference);
                }
                self.range_value(node, (row - 1) * columns + column - 1)
            }
            Function::Match if matches!(arguments.len(), 2 | 3) => {
                let mode = if arguments.len() == 3 {
                    match number(self.evaluate(&arguments[2])) {
                        Ok(value) => match_mode_for(value),
                        Err(error) => return Value::Error(error),
                    }
                } else {
                    MatchMode::Ascending
                };
                let lookup = self.evaluate(&arguments[0]);
                if matches!(lookup, Value::Error(_)) {
                    return lookup;
                }
                let Some((node, rows, columns)) = range_parts(&arguments[1]) else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                if rows != 1 && columns != 1 {
                    return Value::Error(CalcError::InvalidArguments);
                }
                let candidates = self.range_values(node);
                match match_position(&lookup, &candidates, mode) {
                    Ok(position) => Value::Number((position + 1) as f64),
                    Err(error) => Value::Error(error),
                }
            }
            Function::VLookup | Function::HLookup if matches!(arguments.len(), 3 | 4) => {
                let mode = match arguments.get(3) {
                    None | Some(Expr::Empty) => MatchMode::Ascending,
                    Some(argument) => match self.evaluate(argument) {
                        Value::Boolean(false) => MatchMode::Exact,
                        Value::Boolean(true) | Value::Blank => MatchMode::Ascending,
                        Value::Number(value) => {
                            if value == 0.0 {
                                MatchMode::Exact
                            } else {
                                MatchMode::Ascending
                            }
                        }
                        Value::Error(error) => return Value::Error(error),
                        Value::Text(_) => return Value::Error(CalcError::InvalidValue),
                    },
                };
                let lookup = self.evaluate(&arguments[0]);
                if matches!(lookup, Value::Error(_)) {
                    return lookup;
                }
                let Some((node, rows, columns)) = range_parts(&arguments[1]) else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                let Ok(offset) = positive_index(self.evaluate(&arguments[2])) else {
                    return Value::Error(CalcError::InvalidValue);
                };
                let vertical = function == Function::VLookup;
                let (lanes, lane_length) = if vertical {
                    (rows, columns)
                } else {
                    (columns, rows)
                };
                if offset > lane_length {
                    return Value::Error(CalcError::InvalidReference);
                }
                let at = |lane: usize, position: usize| {
                    if vertical {
                        self.range_value(node, lane * columns + position)
                    } else {
                        self.range_value(node, position * columns + lane)
                    }
                };
                let candidates: Vec<Value> = (0..lanes).map(|lane| at(lane, 0)).collect();
                match match_position(&lookup, &candidates, mode) {
                    Ok(lane) => at(lane, offset - 1),
                    Err(error) => Value::Error(error),
                }
            }
            Function::XLookup if matches!(arguments.len(), 3 | 4) => {
                let lookup = self.evaluate(&arguments[0]);
                if matches!(lookup, Value::Error(_)) {
                    return lookup;
                }
                let Some((lookup_node, lookup_rows, lookup_columns)) = range_parts(&arguments[1])
                else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                let Some((return_node, return_rows, return_columns)) = range_parts(&arguments[2])
                else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                let length = self.range_len(lookup_node);
                if (lookup_rows != 1 && lookup_columns != 1)
                    || (return_rows != 1 && return_columns != 1)
                    || length != self.range_len(return_node)
                {
                    return Value::Error(CalcError::InvalidArguments);
                }
                for index in 0..length {
                    let candidate = self.range_value(lookup_node, index);
                    if matches!(candidate, Value::Error(_)) {
                        return candidate;
                    }
                    if lookup_equal(&lookup, &candidate) {
                        return self.range_value(return_node, index);
                    }
                }
                if arguments.len() == 4 {
                    return self.evaluate(&arguments[3]);
                }
                Value::Error(CalcError::NotAvailable)
            }
            _ => Value::Error(CalcError::InvalidArguments),
        }
    }
}

fn match_mode_for(value: f64) -> MatchMode {
    if value == 0.0 {
        MatchMode::Exact
    } else if value > 0.0 {
        MatchMode::Ascending
    } else {
        MatchMode::Descending
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MatchMode {
    Exact,
    /// Largest value less than or equal to the lookup; the candidates are
    /// assumed to be sorted ascending, as Excel documents.
    Ascending,
    /// Smallest value greater than or equal to the lookup; candidates are
    /// assumed to be sorted descending.
    Descending,
}

/// Position of `lookup` among `candidates`. Exact mode scans in order and
/// compares text case-insensitively. Approximate modes binary-search the
/// non-blank candidates by Excel's typed order (numbers before text before
/// booleans) and return `#N/A` when nothing qualifies, which is the documented
/// contract for sorted inputs; unsorted inputs are undefined in Excel too.
fn match_position(
    lookup: &Value,
    candidates: &[Value],
    mode: MatchMode,
) -> Result<usize, CalcError> {
    if let Value::Error(error) = lookup {
        return Err(error.clone());
    }
    if mode == MatchMode::Exact {
        for (index, candidate) in candidates.iter().enumerate() {
            if let Value::Error(error) = candidate {
                return Err(error.clone());
            }
            if lookup_equal(lookup, candidate) {
                return Ok(index);
            }
        }
        return Err(CalcError::NotAvailable);
    }
    let populated: Vec<(usize, &Value)> = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| !matches!(candidate, Value::Blank))
        .collect();
    if populated.is_empty() {
        return Err(CalcError::NotAvailable);
    }
    let same_type =
        |candidate: &Value| std::mem::discriminant(candidate) == std::mem::discriminant(lookup);
    // Binary search for the boundary, then require a same-typed neighbour.
    let (mut low, mut high) = (0_usize, populated.len());
    while low < high {
        let middle = (low + high) / 2;
        let candidate = populated[middle].1;
        let ordering = typed_compare(candidate, lookup)?;
        let goes_right = match mode {
            MatchMode::Ascending => ordering != std::cmp::Ordering::Greater,
            MatchMode::Descending => ordering != std::cmp::Ordering::Less,
            MatchMode::Exact => unreachable!("handled above"),
        };
        if goes_right {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let chosen = low.checked_sub(1).map(|index| populated[index]);
    match chosen {
        Some((position, candidate)) if same_type(candidate) => Ok(position),
        _ => Err(CalcError::NotAvailable),
    }
}

/// Excel's comparison order for `<`, `>` and approximate lookups: numbers
/// sort before text, text before booleans; text compares case-insensitively;
/// blanks take the other side's type.
fn typed_compare(left: &Value, right: &Value) -> Result<std::cmp::Ordering, CalcError> {
    fn rank(value: &Value) -> u8 {
        match value {
            Value::Number(_) | Value::Blank => 0,
            Value::Text(_) => 1,
            Value::Boolean(_) => 2,
            Value::Error(_) => 3,
        }
    }
    if let Value::Error(error) = left {
        return Err(error.clone());
    }
    if let Value::Error(error) = right {
        return Err(error.clone());
    }
    let (left, right) = match (left, right) {
        (Value::Blank, Value::Text(_)) => (Value::Text(String::new()), right.clone()),
        (Value::Text(_), Value::Blank) => (left.clone(), Value::Text(String::new())),
        (Value::Blank, Value::Boolean(_)) => (Value::Boolean(false), right.clone()),
        (Value::Boolean(_), Value::Blank) => (left.clone(), Value::Boolean(false)),
        (Value::Blank, _) => (Value::Number(0.0), right.clone()),
        (_, Value::Blank) => (left.clone(), Value::Number(0.0)),
        _ => (left.clone(), right.clone()),
    };
    Ok(match (&left, &right) {
        (Value::Number(left), Value::Number(right)) => left.total_cmp(right),
        (Value::Text(left), Value::Text(right)) => left.to_lowercase().cmp(&right.to_lowercase()),
        (Value::Boolean(left), Value::Boolean(right)) => left.cmp(right),
        _ => rank(&left).cmp(&rank(&right)),
    })
}

fn range_parts(expression: &Expr<usize>) -> Option<(usize, usize, usize)> {
    match expression {
        Expr::RangeNode {
            node,
            rows,
            columns,
        } => Some((*node, *rows, *columns)),
        _ => None,
    }
}

fn positive_index(value: Value) -> Result<usize, CalcError> {
    let value = number(value)?;
    if value >= 1.0 && value.fract() == 0.0 && value <= usize::MAX as f64 {
        Ok(value as usize)
    } else {
        Err(CalcError::InvalidArguments)
    }
}

fn lookup_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Text(left), Value::Text(right)) => left.eq_ignore_ascii_case(right),
        _ => left == right,
    }
}

/// Date arguments accept numbers and blanks only. Booleans and text are
/// `#VALUE!` rather than being coerced, so a text date never becomes a
/// plausible serial without an explicit parse step.
fn date_number(value: Value) -> Result<f64, CalcError> {
    match value {
        Value::Number(value) => Ok(value),
        Value::Blank => Ok(0.0),
        Value::Boolean(_) | Value::Text(_) => Err(CalcError::InvalidValue),
        Value::Error(error) => Err(error),
    }
}

/// Month offsets and weekday return types are truncated integers.
fn serial_offset(value: f64) -> Result<i64, CalcError> {
    if !value.is_finite() || value.abs() > 1.0e9 {
        return Err(CalcError::InvalidNumber);
    }
    Ok(value.trunc() as i64)
}

fn number(value: Value) -> Result<f64, CalcError> {
    match value {
        Value::Number(value) => Ok(value),
        Value::Blank => Ok(0.0),
        Value::Boolean(value) => Ok(if value { 1.0 } else { 0.0 }),
        Value::Text(_) => Err(CalcError::InvalidValue),
        Value::Error(error) => Err(error),
    }
}

fn truthy(value: Value) -> Result<bool, CalcError> {
    match value {
        Value::Boolean(value) => Ok(value),
        Value::Number(value) => Ok(value != 0.0),
        Value::Blank => Ok(false),
        Value::Text(_) => Err(CalcError::InvalidValue),
        Value::Error(error) => Err(error),
    }
}

fn apply_binary(operator: BinaryOp, left: Value, right: Value) -> Value {
    if let Value::Error(error) = &left {
        return Value::Error(error.clone());
    }
    if let Value::Error(error) = &right {
        return Value::Error(error.clone());
    }
    if operator == BinaryOp::Concat {
        return match (text_value(&left), text_value(&right)) {
            (Ok(left), Ok(right)) => Value::Text(left + &right),
            (Err(error), _) | (_, Err(error)) => Value::Error(error),
        };
    }
    if matches!(
        operator,
        BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Less
            | BinaryOp::LessOrEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterOrEqual
    ) {
        use std::cmp::Ordering;
        // Equality never crosses types (a number is never equal to text),
        // except that a blank takes the other side's type.
        let comparable = matches!(
            (&left, &right),
            (Value::Number(_), Value::Number(_))
                | (Value::Text(_), Value::Text(_))
                | (Value::Boolean(_), Value::Boolean(_))
                | (Value::Blank, _)
                | (_, Value::Blank)
        );
        let ordering = match typed_compare(&left, &right) {
            Ok(ordering) => ordering,
            Err(error) => return Value::Error(error),
        };
        return Value::Boolean(match operator {
            BinaryOp::Equal => comparable && ordering == Ordering::Equal,
            BinaryOp::NotEqual => !(comparable && ordering == Ordering::Equal),
            BinaryOp::Less => ordering == Ordering::Less,
            BinaryOp::LessOrEqual => ordering != Ordering::Greater,
            BinaryOp::Greater => ordering == Ordering::Greater,
            BinaryOp::GreaterOrEqual => ordering != Ordering::Less,
            _ => unreachable!("matched above"),
        });
    }
    let (left, right) = match (number(left), number(right)) {
        (Ok(left), Ok(right)) => (left, right),
        (Err(error), _) | (_, Err(error)) => return Value::Error(error),
    };
    match operator {
        BinaryOp::Add => Value::Number(left + right),
        BinaryOp::Subtract => Value::Number(left - right),
        BinaryOp::Multiply => Value::Number(left * right),
        BinaryOp::Divide if right == 0.0 => Value::Error(CalcError::DivisionByZero),
        BinaryOp::Divide => Value::Number(left / right),
        BinaryOp::Power => {
            let result = left.powf(right);
            if (left == 0.0 && right == 0.0) || !result.is_finite() {
                Value::Error(CalcError::InvalidNumber)
            } else {
                Value::Number(result)
            }
        }
        BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::Concat
        | BinaryOp::Less
        | BinaryOp::LessOrEqual
        | BinaryOp::Greater
        | BinaryOp::GreaterOrEqual => unreachable!("handled above"),
    }
}

/// Sample (`n - 1`) or population (`n`) standard deviation or variance.
fn deviation(numbers: &[f64], sample: bool, root: bool) -> Value {
    let count = numbers.len();
    if count < if sample { 2 } else { 1 } {
        return Value::Error(CalcError::DivisionByZero);
    }
    let mean = numbers.iter().sum::<f64>() / count as f64;
    let sum_of_squares: f64 = numbers.iter().map(|value| (value - mean).powi(2)).sum();
    let variance = sum_of_squares / if sample { count - 1 } else { count } as f64;
    Value::Number(if root { variance.sqrt() } else { variance })
}

/// `FIND(needle, haystack, [start])`: case-sensitive, one-based, `#VALUE!`
/// when absent.
fn find_text(values: &[Value]) -> Value {
    if !matches!(values.len(), 2 | 3) {
        return Value::Error(CalcError::InvalidArguments);
    }
    let (Ok(needle), Ok(haystack)) = (text_value(&values[0]), text_value(&values[1])) else {
        return Value::Error(CalcError::InvalidValue);
    };
    let start = if values.len() == 3 {
        match text_count(&values[2]) {
            Ok(0) => return Value::Error(CalcError::InvalidValue),
            Ok(start) => start,
            Err(error) => return Value::Error(error),
        }
    } else {
        1
    };
    let characters: Vec<char> = haystack.chars().collect();
    if start > characters.len() + 1 {
        return Value::Error(CalcError::InvalidValue);
    }
    let needle: Vec<char> = needle.chars().collect();
    (start - 1..=characters.len().saturating_sub(needle.len()))
        .find(|&index| characters[index..].starts_with(&needle))
        .map(|index| Value::Number((index + 1) as f64))
        .unwrap_or(Value::Error(CalcError::InvalidValue))
}

/// `REPT(text, count)`, bounded so a hostile count cannot exhaust memory.
fn repeat_text(values: &[Value]) -> Value {
    if values.len() != 2 {
        return Value::Error(CalcError::InvalidArguments);
    }
    let (Ok(text), Ok(count)) = (text_value(&values[0]), text_count(&values[1])) else {
        return Value::Error(CalcError::InvalidValue);
    };
    if text.chars().count().saturating_mul(count) > 32_767 {
        return Value::Error(CalcError::InvalidValue);
    }
    Value::Text(text.repeat(count))
}

fn first_error(values: &[Value]) -> Option<CalcError> {
    values.iter().find_map(|value| match value {
        Value::Error(error) => Some(error.clone()),
        _ => None,
    })
}

fn numeric_only(values: &[Value]) -> Vec<f64> {
    values
        .iter()
        .filter_map(|value| match value {
            Value::Number(number) => Some(*number),
            _ => None,
        })
        .collect()
}

/// `AVERAGEA`: numbers as they are, `TRUE`/`FALSE` as 1/0, text as 0,
/// blanks ignored.
fn average_a(values: &[Value]) -> Value {
    let mut sum = 0.0;
    let mut count = 0_usize;
    for value in values {
        match value {
            Value::Number(number) => sum += number,
            Value::Boolean(true) => sum += 1.0,
            Value::Boolean(false) | Value::Text(_) => {}
            Value::Blank => continue,
            Value::Error(error) => return Value::Error(error.clone()),
        }
        count += 1;
    }
    if count == 0 {
        Value::Error(CalcError::DivisionByZero)
    } else {
        Value::Number(sum / count as f64)
    }
}

/// `NORMDIST(x, mean, standard_dev, cumulative)`.
fn normal_distribution(values: &[Value]) -> Value {
    if values.len() != 4 {
        return Value::Error(CalcError::InvalidArguments);
    }
    let x = number(values[0].clone());
    let mean = number(values[1].clone());
    let deviation = number(values[2].clone());
    let cumulative = truthy(values[3].clone());
    let (x, mean, deviation, cumulative) = match (x, mean, deviation, cumulative) {
        (Ok(x), Ok(mean), Ok(deviation), Ok(cumulative)) => (x, mean, deviation, cumulative),
        (Err(error), ..) | (_, Err(error), ..) | (_, _, Err(error), _) | (_, _, _, Err(error)) => {
            return Value::Error(error);
        }
    };
    if deviation <= 0.0 {
        return Value::Error(CalcError::InvalidNumber);
    }
    let z = (x - mean) / deviation;
    Value::Number(if cumulative {
        0.5 * complementary_error(-z / std::f64::consts::SQRT_2)
    } else {
        (-0.5 * z * z).exp() / (deviation * (2.0 * std::f64::consts::PI).sqrt())
    })
}

/// `erfc(x)` to double precision: a Maclaurin series below 2, where its
/// cancellation stays within a few hundred units in the last place of the
/// tail, and the continued fraction from 2 upwards, evaluated backwards
/// from a fixed depth.
fn complementary_error(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    let magnitude = x.abs();
    let tail = if magnitude < 2.0 {
        let square = magnitude * magnitude;
        let mut term = magnitude;
        let mut sum = magnitude;
        let mut n = 0.0;
        loop {
            n += 1.0;
            term *= -square / n;
            let addend = term / (2.0 * n + 1.0);
            sum += addend;
            if addend.abs() < 1.0e-17 * sum.abs() || n > 200.0 {
                break;
            }
        }
        1.0 - sum * 2.0 / std::f64::consts::PI.sqrt()
    } else {
        let mut fraction = magnitude;
        for n in (1..=80).rev() {
            fraction = magnitude + (f64::from(n) / 2.0) / fraction;
        }
        (-square_of(magnitude)).exp() / std::f64::consts::PI.sqrt() / fraction
    };
    if x < 0.0 { 2.0 - tail } else { tail }
}

fn square_of(value: f64) -> f64 {
    value * value
}

/// `PMT`: the constant payment of an annuity.
fn payment(
    rate: f64,
    periods: f64,
    present: f64,
    future: f64,
    at_start: bool,
) -> Result<f64, CalcError> {
    if periods == 0.0 || !periods.is_finite() || !rate.is_finite() {
        return Err(CalcError::InvalidNumber);
    }
    if rate == 0.0 {
        return Ok(-(present + future) / periods);
    }
    let growth = (1.0 + rate).powf(periods);
    if !growth.is_finite() || growth == 1.0 {
        return Err(CalcError::InvalidNumber);
    }
    let timing = if at_start { 1.0 + rate } else { 1.0 };
    Ok(-(present * growth + future) * rate / (timing * (growth - 1.0)))
}

/// `NPV`: cash flows discounted from the end of the first period.
fn net_present_value(rate: f64, values: &[f64]) -> Result<f64, CalcError> {
    if rate == -1.0 {
        return Err(CalcError::DivisionByZero);
    }
    let mut total = 0.0;
    for (index, value) in values.iter().enumerate() {
        total += value / (1.0 + rate).powi(index as i32 + 1);
    }
    if total.is_finite() {
        Ok(total)
    } else {
        Err(CalcError::InvalidNumber)
    }
}

/// Cash flows paired with the number of days from the first date, as `XNPV`
/// and `XIRR` read them: both lists numeric and the same length, no date
/// before the first.
fn dated_cash_flows(values: &[Value], dates: &[Value]) -> Result<Vec<(f64, f64)>, CalcError> {
    if let Some(error) = first_error(values).or_else(|| first_error(dates)) {
        return Err(error);
    }
    if values.len() != dates.len() || values.is_empty() {
        return Err(CalcError::InvalidNumber);
    }
    let mut cash_flows = Vec::with_capacity(values.len());
    for (value, date) in values.iter().zip(dates) {
        match (value, date) {
            (Value::Number(value), Value::Number(date)) => {
                cash_flows.push((*value, serial_date::serial_from_number(*date)? as f64));
            }
            _ => return Err(CalcError::InvalidValue),
        }
    }
    let first = cash_flows[0].1;
    if cash_flows.iter().any(|(_, date)| *date < first) {
        return Err(CalcError::InvalidNumber);
    }
    Ok(cash_flows
        .into_iter()
        .map(|(value, date)| (value, date - first))
        .collect())
}

fn dated_net_present_value(rate: f64, cash_flows: &[(f64, f64)]) -> Result<f64, CalcError> {
    if rate <= -1.0 || !rate.is_finite() {
        return Err(CalcError::InvalidNumber);
    }
    let total: f64 = cash_flows
        .iter()
        .map(|(value, days)| value / (1.0 + rate).powf(days / 365.0))
        .sum();
    if total.is_finite() {
        Ok(total)
    } else {
        Err(CalcError::InvalidNumber)
    }
}

/// `XIRR` by Newton's method from `guess`, the rate at which the dated net
/// present value is zero.
fn internal_rate_of_return(cash_flows: &[(f64, f64)], guess: f64) -> Result<f64, CalcError> {
    if !cash_flows.iter().any(|(value, _)| *value > 0.0)
        || !cash_flows.iter().any(|(value, _)| *value < 0.0)
    {
        return Err(CalcError::InvalidNumber);
    }
    let mut rate = if guess > -1.0 && guess.is_finite() {
        guess
    } else {
        0.1
    };
    for _ in 0..100 {
        let base = 1.0 + rate;
        let mut value = 0.0;
        let mut slope = 0.0;
        for (cash, days) in cash_flows {
            let years = days / 365.0;
            let discount = base.powf(years);
            value += cash / discount;
            slope -= years * cash / (discount * base);
        }
        if slope == 0.0 || !slope.is_finite() {
            return Err(CalcError::InvalidNumber);
        }
        let next = rate - value / slope;
        if !next.is_finite() || next <= -1.0 {
            return Err(CalcError::InvalidNumber);
        }
        if (next - rate).abs() <= 1.0e-10 * rate.abs().max(1.0) {
            return Ok(next);
        }
        rate = next;
    }
    Err(CalcError::InvalidNumber)
}

/// Pearson correlation of numeric pairs.
fn correlation(pairs: &[(f64, f64)]) -> Result<f64, CalcError> {
    if pairs.len() < 2 {
        return Err(CalcError::DivisionByZero);
    }
    let count = pairs.len() as f64;
    let mean_x = pairs.iter().map(|(x, _)| x).sum::<f64>() / count;
    let mean_y = pairs.iter().map(|(_, y)| y).sum::<f64>() / count;
    let (mut covariance, mut variance_x, mut variance_y) = (0.0, 0.0, 0.0);
    for (x, y) in pairs {
        covariance += (x - mean_x) * (y - mean_y);
        variance_x += (x - mean_x) * (x - mean_x);
        variance_y += (y - mean_y) * (y - mean_y);
    }
    if variance_x == 0.0 || variance_y == 0.0 {
        return Err(CalcError::DivisionByZero);
    }
    Ok(covariance / (variance_x * variance_y).sqrt())
}

fn median(mut numbers: Vec<f64>) -> Value {
    numbers.sort_by(f64::total_cmp);
    let middle = numbers.len() / 2;
    Value::Number(if numbers.len() % 2 == 1 {
        numbers[middle]
    } else {
        (numbers[middle - 1] + numbers[middle]) / 2.0
    })
}

fn unary_number(values: &[Value], operation: impl FnOnce(f64) -> f64) -> Value {
    if values.len() != 1 {
        return Value::Error(CalcError::InvalidArguments);
    }
    match number(values[0].clone()) {
        Ok(value) => {
            let result = operation(value);
            if result.is_nan() {
                Value::Error(CalcError::InvalidValue)
            } else {
                Value::Number(result)
            }
        }
        Err(error) => Value::Error(error),
    }
}

fn binary_number(values: &[Value], operation: impl FnOnce(f64, f64) -> Option<f64>) -> Value {
    if values.len() != 2 {
        return Value::Error(CalcError::InvalidArguments);
    }
    match (number(values[0].clone()), number(values[1].clone())) {
        (Ok(left), Ok(right)) => operation(left, right)
            .map(Value::Number)
            .unwrap_or(Value::Error(CalcError::DivisionByZero)),
        (Err(error), _) | (_, Err(error)) => Value::Error(error),
    }
}

enum RoundDirection {
    Nearest,
    AwayFromZero,
    TowardZero,
}

fn round_function(values: &[Value], direction: RoundDirection) -> Value {
    if values.len() != 2 {
        return Value::Error(CalcError::InvalidArguments);
    }
    let (Ok(value), Ok(digits)) = (number(values[0].clone()), number(values[1].clone())) else {
        return Value::Error(CalcError::InvalidValue);
    };
    if !digits.is_finite() || !(-308.0..=308.0).contains(&digits) {
        return Value::Error(CalcError::InvalidValue);
    }
    let factor = 10_f64.powi(digits.trunc() as i32);
    let scaled = value * factor;
    let rounded = match direction {
        RoundDirection::Nearest => scaled.round(),
        RoundDirection::AwayFromZero => {
            if scaled.is_sign_negative() {
                scaled.floor()
            } else {
                scaled.ceil()
            }
        }
        RoundDirection::TowardZero => scaled.trunc(),
    };
    Value::Number(rounded / factor)
}

fn logical_fold(values: &[Value], initial: bool, operation: impl Fn(bool, bool) -> bool) -> Value {
    if values.is_empty() {
        return Value::Error(CalcError::InvalidArguments);
    }
    let mut result = initial;
    for value in values {
        match truthy(value.clone()) {
            Ok(value) => result = operation(result, value),
            Err(error) => return Value::Error(error),
        }
    }
    Value::Boolean(result)
}

fn trunc_function(values: &[Value]) -> Value {
    if !matches!(values.len(), 1 | 2) {
        return Value::Error(CalcError::InvalidArguments);
    }
    let Ok(value) = number(values[0].clone()) else {
        return Value::Error(CalcError::InvalidValue);
    };
    let digits = if values.len() == 2 {
        match number(values[1].clone()) {
            Ok(value) if (-308.0..=308.0).contains(&value) => value.trunc() as i32,
            _ => return Value::Error(CalcError::InvalidValue),
        }
    } else {
        0
    };
    let factor = 10_f64.powi(digits);
    Value::Number((value * factor).trunc() / factor)
}

fn log_function(values: &[Value]) -> Value {
    if !matches!(values.len(), 1 | 2) {
        return Value::Error(CalcError::InvalidArguments);
    }
    let (Ok(value), Ok(base)) = (
        number(values[0].clone()),
        if values.len() == 2 {
            number(values[1].clone())
        } else {
            Ok(10.0)
        },
    ) else {
        return Value::Error(CalcError::InvalidValue);
    };
    if value <= 0.0 || base <= 0.0 || base == 1.0 {
        Value::Error(CalcError::InvalidValue)
    } else {
        Value::Number(value.log(base))
    }
}

fn text_value(value: &Value) -> Result<String, CalcError> {
    match value {
        Value::Text(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Boolean(true) => Ok("TRUE".into()),
        Value::Boolean(false) => Ok("FALSE".into()),
        Value::Blank => Ok(String::new()),
        Value::Error(error) => Err(error.clone()),
    }
}

fn text_unary(values: &[Value], operation: impl FnOnce(&str) -> String) -> Value {
    if values.len() != 1 {
        return Value::Error(CalcError::InvalidArguments);
    }
    match text_value(&values[0]) {
        Ok(value) => Value::Text(operation(&value)),
        Err(error) => Value::Error(error),
    }
}

fn text_length(values: &[Value]) -> Value {
    if values.len() != 1 {
        return Value::Error(CalcError::InvalidArguments);
    }
    match text_value(&values[0]) {
        Ok(value) => Value::Number(value.chars().count() as f64),
        Err(error) => Value::Error(error),
    }
}

fn text_count(value: &Value) -> Result<usize, CalcError> {
    let value = number(value.clone())?;
    if !value.is_finite() || value < 0.0 || value > usize::MAX as f64 {
        Err(CalcError::InvalidValue)
    } else {
        Ok(value.trunc() as usize)
    }
}

fn text_edge(values: &[Value], left: bool) -> Value {
    if !matches!(values.len(), 1 | 2) {
        return Value::Error(CalcError::InvalidArguments);
    }
    let Ok(value) = text_value(&values[0]) else {
        return Value::Error(CalcError::InvalidValue);
    };
    let count = if values.len() == 2 {
        match text_count(&values[1]) {
            Ok(value) => value,
            Err(error) => return Value::Error(error),
        }
    } else {
        1
    };
    let characters: Vec<char> = value.chars().collect();
    let output = if left {
        characters.into_iter().take(count).collect()
    } else {
        characters
            .into_iter()
            .rev()
            .take(count)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };
    Value::Text(output)
}

fn text_mid(values: &[Value]) -> Value {
    if values.len() != 3 {
        return Value::Error(CalcError::InvalidArguments);
    }
    let (Ok(value), Ok(start), Ok(count)) = (
        text_value(&values[0]),
        text_count(&values[1]),
        text_count(&values[2]),
    ) else {
        return Value::Error(CalcError::InvalidValue);
    };
    if start == 0 {
        return Value::Error(CalcError::InvalidValue);
    }
    Value::Text(value.chars().skip(start - 1).take(count).collect())
}

fn concatenate(values: &[Value]) -> Value {
    let mut output = String::new();
    for value in values {
        match text_value(value) {
            Ok(value) => output.push_str(&value),
            Err(error) => return Value::Error(error),
        }
    }
    Value::Text(output)
}

fn parse_text_number(values: &[Value]) -> Value {
    if values.len() != 1 {
        return Value::Error(CalcError::InvalidArguments);
    }
    match text_value(&values[0]).and_then(|value| {
        value
            .trim()
            .parse::<f64>()
            .map_err(|_| CalcError::InvalidValue)
    }) {
        Ok(value) => Value::Number(value),
        Err(error) => Value::Error(error),
    }
}

fn exact_text(values: &[Value]) -> Value {
    if values.len() != 2 {
        return Value::Error(CalcError::InvalidArguments);
    }
    match (text_value(&values[0]), text_value(&values[1])) {
        (Ok(left), Ok(right)) => Value::Boolean(left == right),
        (Err(error), _) | (_, Err(error)) => Value::Error(error),
    }
}

fn criterion_matches(candidate: Value, criterion: Value) -> Result<bool, CalcError> {
    let (operator, expected) = match criterion {
        Value::Text(text) => parse_criterion(&text),
        value => (BinaryOp::Equal, value),
    };
    if let (Value::Text(left), Value::Text(right)) = (&candidate, &expected) {
        let left = left.to_lowercase();
        let right = right.to_lowercase();
        return Ok(match operator {
            BinaryOp::Equal => left == right,
            BinaryOp::NotEqual => left != right,
            BinaryOp::Less => left < right,
            BinaryOp::LessOrEqual => left <= right,
            BinaryOp::Greater => left > right,
            BinaryOp::GreaterOrEqual => left >= right,
            _ => return Err(CalcError::InvalidArguments),
        });
    }
    match apply_binary(operator, candidate, expected) {
        Value::Boolean(value) => Ok(value),
        Value::Error(CalcError::InvalidValue) => Ok(false),
        Value::Error(error) => Err(error),
        _ => Err(CalcError::InvalidValue),
    }
}

fn parse_criterion(criterion: &str) -> (BinaryOp, Value) {
    let criterion = criterion.trim();
    let (operator, operand) = if let Some(value) = criterion.strip_prefix("<=") {
        (BinaryOp::LessOrEqual, value)
    } else if let Some(value) = criterion.strip_prefix(">=") {
        (BinaryOp::GreaterOrEqual, value)
    } else if let Some(value) = criterion.strip_prefix("<>") {
        (BinaryOp::NotEqual, value)
    } else if let Some(value) = criterion.strip_prefix('=') {
        (BinaryOp::Equal, value)
    } else if let Some(value) = criterion.strip_prefix('<') {
        (BinaryOp::Less, value)
    } else if let Some(value) = criterion.strip_prefix('>') {
        (BinaryOp::Greater, value)
    } else {
        (BinaryOp::Equal, criterion)
    };
    let operand = operand.trim();
    let value = if operand.is_empty() {
        Value::Blank
    } else if operand.eq_ignore_ascii_case("TRUE") {
        Value::Boolean(true)
    } else if operand.eq_ignore_ascii_case("FALSE") {
        Value::Boolean(false)
    } else if let Ok(value) = operand.parse::<f64>() {
        Value::Number(value)
    } else {
        Value::Text(operand.into())
    };
    (operator, value)
}

fn compile_expression(
    expression: Expr<CellId>,
    indices: &HashMap<CellId, usize>,
    range_nodes: &HashMap<RangeKey, usize>,
) -> Expr<usize> {
    match expression {
        Expr::Number(value) => Expr::Number(value),
        Expr::Boolean(value) => Expr::Boolean(value),
        Expr::Text(value) => Expr::Text(value),
        Expr::Error(error) => Expr::Error(error),
        Expr::Empty => Expr::Empty,
        Expr::Reference(cell) => Expr::Reference(indices[&cell]),
        Expr::UnaryMinus(inner) => {
            Expr::UnaryMinus(Box::new(compile_expression(*inner, indices, range_nodes)))
        }
        Expr::Percent(inner) => {
            Expr::Percent(Box::new(compile_expression(*inner, indices, range_nodes)))
        }
        Expr::Binary(operator, left, right) => Expr::Binary(
            operator,
            Box::new(compile_expression(*left, indices, range_nodes)),
            Box::new(compile_expression(*right, indices, range_nodes)),
        ),
        Expr::Range {
            anchor,
            members,
            rows,
            columns,
        } => Expr::RangeNode {
            node: range_nodes[&range_key(anchor, members, rows, columns)],
            rows,
            columns,
        },
        Expr::RangeNode { .. } => unreachable!("parsed formulas never hold range nodes"),
        Expr::Function(function, arguments) => Expr::Function(
            function,
            arguments
                .into_iter()
                .map(|argument| compile_expression(argument, indices, range_nodes))
                .collect(),
        ),
    }
}

fn count_nodes<R>(expression: &Expr<R>) -> usize {
    1 + match expression {
        Expr::UnaryMinus(inner) | Expr::Percent(inner) => count_nodes(inner),
        Expr::Binary(_, left, right) => count_nodes(left) + count_nodes(right),
        Expr::Function(_, arguments) => arguments.iter().map(count_nodes).sum(),
        Expr::Range { members, .. } => members.as_ref().map_or(0, Vec::len),
        _ => 0,
    }
}

fn collect_dependencies(
    expression: &Expr,
    cells: &mut BTreeSet<CellId>,
    ranges: &mut Vec<RangeKey>,
) {
    match expression {
        Expr::Reference(cell) => {
            cells.insert(*cell);
        }
        Expr::UnaryMinus(inner) | Expr::Percent(inner) => {
            collect_dependencies(inner, cells, ranges)
        }
        Expr::Binary(_, left, right) => {
            collect_dependencies(left, cells, ranges);
            collect_dependencies(right, cells, ranges);
        }
        Expr::Range {
            anchor,
            members,
            rows,
            columns,
        } => ranges.push(range_key(*anchor, members.clone(), *rows, *columns)),
        Expr::Function(_, arguments) => {
            for argument in arguments {
                collect_dependencies(argument, cells, ranges);
            }
        }
        Expr::RangeNode { .. }
        | Expr::Number(_)
        | Expr::Boolean(_)
        | Expr::Text(_)
        | Expr::Error(_)
        | Expr::Empty => {}
    }
}

struct Parser<'source, 'sheets> {
    source: &'source str,
    offset: usize,
    sheet: u32,
    sheet_names: &'sheets HashMap<String, u32>,
    defined_names: &'sheets HashMap<String, String>,
    name_depth: usize,
}

impl<'source, 'sheets> Parser<'source, 'sheets> {
    fn new(
        source: &'source str,
        sheet: u32,
        sheet_names: &'sheets HashMap<String, u32>,
        defined_names: &'sheets HashMap<String, String>,
    ) -> Self {
        let source = source.strip_prefix('=').unwrap_or(source);
        Self {
            source,
            offset: 0,
            sheet,
            sheet_names,
            defined_names,
            name_depth: 0,
        }
    }

    fn parse(mut self) -> Result<Expr, FormulaError> {
        self.skip_space();
        if self.offset == self.source.len() {
            return Err(FormulaError::Empty);
        }
        let expression = self.parse_comparison()?;
        self.skip_space();
        if self.offset != self.source.len() {
            return Err(FormulaError::UnexpectedToken(self.offset));
        }
        Ok(expression)
    }

    fn parse_comparison(&mut self) -> Result<Expr, FormulaError> {
        let mut left = self.parse_concatenation()?;
        loop {
            self.skip_space();
            let (operator, width) = if self.remaining().starts_with("<>") {
                (BinaryOp::NotEqual, 2)
            } else if self.remaining().starts_with("<=") {
                (BinaryOp::LessOrEqual, 2)
            } else if self.remaining().starts_with(">=") {
                (BinaryOp::GreaterOrEqual, 2)
            } else {
                match self.peek() {
                    Some(b'=') => (BinaryOp::Equal, 1),
                    Some(b'<') => (BinaryOp::Less, 1),
                    Some(b'>') => (BinaryOp::Greater, 1),
                    _ => break,
                }
            };
            self.offset += width;
            let right = self.parse_concatenation()?;
            left = Expr::Binary(operator, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_concatenation(&mut self) -> Result<Expr, FormulaError> {
        let mut left = self.parse_additive()?;
        loop {
            self.skip_space();
            if self.peek() != Some(b'&') {
                break;
            }
            self.offset += 1;
            let right = self.parse_additive()?;
            left = Expr::Binary(BinaryOp::Concat, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, FormulaError> {
        let mut left = self.parse_multiplicative()?;
        loop {
            self.skip_space();
            let operator = match self.peek() {
                Some(b'+') => BinaryOp::Add,
                Some(b'-') => BinaryOp::Subtract,
                _ => break,
            };
            self.offset += 1;
            let right = self.parse_multiplicative()?;
            left = Expr::Binary(operator, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, FormulaError> {
        let mut left = self.parse_power()?;
        loop {
            self.skip_space();
            let operator = match self.peek() {
                Some(b'*') => BinaryOp::Multiply,
                Some(b'/') => BinaryOp::Divide,
                _ => break,
            };
            self.offset += 1;
            let right = self.parse_power()?;
            left = Expr::Binary(operator, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// Excel's `^` is left-associative: `2^3^2` is 64.
    fn parse_power(&mut self) -> Result<Expr, FormulaError> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_space();
            if self.peek() != Some(b'^') {
                break;
            }
            self.offset += 1;
            let right = self.parse_unary()?;
            left = Expr::Binary(BinaryOp::Power, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, FormulaError> {
        self.skip_space();
        match self.peek() {
            Some(b'-') => {
                self.offset += 1;
                Ok(Expr::UnaryMinus(Box::new(self.parse_unary()?)))
            }
            Some(b'+') => {
                self.offset += 1;
                self.parse_unary()
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, FormulaError> {
        let mut expression = self.parse_primary()?;
        loop {
            self.skip_space();
            if self.peek() != Some(b'%') {
                break;
            }
            self.offset += 1;
            expression = Expr::Percent(Box::new(expression));
        }
        Ok(expression)
    }

    fn parse_primary(&mut self) -> Result<Expr, FormulaError> {
        self.skip_space();
        match self.peek() {
            Some(b'(') => {
                self.offset += 1;
                let expression = self.parse_comparison()?;
                self.skip_space();
                self.expect(b')')?;
                Ok(expression)
            }
            Some(b'"') => self.parse_string(),
            Some(b'\'') => self.parse_quoted_sheet_reference(),
            Some(b'#') => self.parse_error_literal(),
            Some(b'[') => Err(FormulaError::ExternalReference(self.bounded_remainder())),
            Some(byte) if byte.is_ascii_digit() || byte == b'.' => self.parse_number(),
            Some(byte) if byte.is_ascii_alphabetic() || byte == b'$' || byte == b'_' => {
                self.parse_reference_or_function()
            }
            _ => Err(FormulaError::UnexpectedToken(self.offset)),
        }
    }

    fn bounded_remainder(&self) -> String {
        self.remaining().chars().take(64).collect()
    }

    fn parse_error_literal(&mut self) -> Result<Expr, FormulaError> {
        let start = self.offset;
        while matches!(self.peek(), Some(byte) if byte.is_ascii_alphanumeric() || matches!(byte, b'#' | b'/' | b'!' | b'?'))
        {
            self.offset += 1;
        }
        CalcError::parse_literal(&self.source[start..self.offset])
            .map(Expr::Error)
            .ok_or(FormulaError::UnexpectedToken(start))
    }

    fn parse_number(&mut self) -> Result<Expr, FormulaError> {
        let start = self.offset;
        while matches!(self.peek(), Some(byte) if byte.is_ascii_digit() || byte == b'.') {
            self.offset += 1;
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.offset += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.offset += 1;
            }
            while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
                self.offset += 1;
            }
        }
        self.source[start..self.offset]
            .parse::<f64>()
            .map(Expr::Number)
            .map_err(|_| FormulaError::UnexpectedToken(start))
    }

    fn parse_string(&mut self) -> Result<Expr, FormulaError> {
        let start = self.offset;
        self.offset += 1;
        let mut value = String::new();
        while let Some(byte) = self.peek() {
            if byte == b'"' {
                self.offset += 1;
                if self.peek() == Some(b'"') {
                    value.push('"');
                    self.offset += 1;
                    continue;
                }
                return Ok(Expr::Text(value));
            }
            if !byte.is_ascii() {
                let character = self.source[self.offset..]
                    .chars()
                    .next()
                    .ok_or(FormulaError::UnexpectedToken(self.offset))?;
                value.push(character);
                self.offset += character.len_utf8();
            } else {
                value.push(char::from(byte));
                self.offset += 1;
            }
        }
        Err(FormulaError::UnexpectedToken(start))
    }

    fn parse_reference_or_function(&mut self) -> Result<Expr, FormulaError> {
        let start = self.offset;
        while matches!(self.peek(), Some(byte) if byte.is_ascii_alphanumeric() || matches!(byte, b'$' | b'_' | b'.'))
        {
            self.offset += 1;
        }
        let token = &self.source[start..self.offset];
        self.skip_space();
        if self.peek() == Some(b'!') {
            let sheet = self.resolve_sheet(token)?;
            self.offset += 1;
            return self.parse_qualified_reference(sheet);
        }
        if self.peek() == Some(b'(') {
            return self.parse_function(token);
        }
        if token.eq_ignore_ascii_case("TRUE") {
            return Ok(Expr::Boolean(true));
        }
        if token.eq_ignore_ascii_case("FALSE") {
            return Ok(Expr::Boolean(false));
        }
        match parse_a1(token, self.sheet) {
            Ok(first) => self.parse_range_tail(first, self.sheet),
            Err(_) => self.parse_defined_name(token),
        }
    }

    /// Expands a defined name by compiling its definition in place, with the
    /// same sheet table and a bounded depth for names that use names.
    fn parse_defined_name(&mut self, token: &str) -> Result<Expr, FormulaError> {
        let name = token.trim_start_matches('$');
        let Some(definition) = self.defined_names.get(&name.to_lowercase()) else {
            return Err(FormulaError::UnknownName(name.into()));
        };
        if self.name_depth >= MAX_NAME_DEPTH {
            return Err(FormulaError::UnsupportedName(name.into()));
        }
        let mut inner = Parser::new(definition, self.sheet, self.sheet_names, self.defined_names);
        inner.name_depth = self.name_depth + 1;
        inner
            .parse()
            .map_err(|_| FormulaError::UnsupportedName(name.into()))
    }

    fn parse_quoted_sheet_reference(&mut self) -> Result<Expr, FormulaError> {
        let start = self.offset;
        self.offset += 1;
        let mut sheet_name = String::new();
        loop {
            let Some(character) = self.source[self.offset..].chars().next() else {
                return Err(FormulaError::UnexpectedToken(start));
            };
            self.offset += character.len_utf8();
            if character == '\'' {
                if self.peek() == Some(b'\'') {
                    sheet_name.push('\'');
                    self.offset += 1;
                    continue;
                }
                break;
            }
            sheet_name.push(character);
        }
        self.skip_space();
        self.expect(b'!')?;
        if sheet_name.starts_with('[') {
            return Err(FormulaError::ExternalReference(
                sheet_name.chars().take(64).collect(),
            ));
        }
        let sheet = self.resolve_sheet(&sheet_name)?;
        self.parse_qualified_reference(sheet)
    }

    fn parse_qualified_reference(&mut self, sheet: u32) -> Result<Expr, FormulaError> {
        self.skip_space();
        let start = self.offset;
        while matches!(self.peek(), Some(byte) if byte.is_ascii_alphanumeric() || byte == b'$') {
            self.offset += 1;
        }
        let first = parse_a1(&self.source[start..self.offset], sheet)?;
        self.parse_range_tail(first, sheet)
    }

    fn parse_range_tail(&mut self, first: CellId, sheet: u32) -> Result<Expr, FormulaError> {
        self.skip_space();
        if self.peek() != Some(b':') {
            return Ok(Expr::Reference(first));
        }
        self.offset += 1;
        self.skip_space();
        let second_start = self.offset;
        while matches!(self.peek(), Some(byte) if byte.is_ascii_alphanumeric() || byte == b'$') {
            self.offset += 1;
        }
        let second = parse_a1(&self.source[second_start..self.offset], sheet)?;
        expand_range(first, second)
    }

    fn resolve_sheet(&self, name: &str) -> Result<u32, FormulaError> {
        self.sheet_names
            .get(&name.to_lowercase())
            .copied()
            .ok_or_else(|| FormulaError::UnknownSheet(name.into()))
    }

    fn parse_function(&mut self, name: &str) -> Result<Expr, FormulaError> {
        let function = parse_function_name(name)?;
        self.expect(b'(')?;
        let mut arguments = Vec::new();
        self.skip_space();
        if self.peek() == Some(b')') {
            self.offset += 1;
            return Ok(Expr::Function(function, arguments));
        }
        loop {
            self.skip_space();
            // An omitted argument, as in `IF(x,,y)` or `SUM(,A1)`.
            let argument = match self.peek() {
                Some(b',') | Some(b')') => Expr::Empty,
                _ => self.parse_comparison()?,
            };
            arguments.push(argument);
            self.skip_space();
            match self.peek() {
                Some(b',') => self.offset += 1,
                Some(b')') => {
                    self.offset += 1;
                    break;
                }
                _ => return Err(FormulaError::UnexpectedToken(self.offset)),
            }
        }
        Ok(Expr::Function(function, arguments))
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(byte) if byte.is_ascii_whitespace()) {
            self.offset += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), FormulaError> {
        if self.peek() == Some(byte) {
            self.offset += 1;
            Ok(())
        } else {
            Err(FormulaError::UnexpectedToken(self.offset))
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.offset).copied()
    }

    fn remaining(&self) -> &str {
        &self.source[self.offset..]
    }
}

/// Every function name the parser accepts, with its implementation. This
/// table is the single registry: `parse_function_name` and
/// [`supported_function_names`] both read it, and a test keeps
/// `docs/FUNCTIONS.md` in step with it so documented counts cannot drift.
const FUNCTION_REGISTRY: &[(&str, Function)] = &[
    ("SUM", Function::Sum),
    ("AVERAGE", Function::Average),
    ("MIN", Function::Min),
    ("MAX", Function::Max),
    ("COUNT", Function::Count),
    ("COUNTA", Function::CountA),
    ("PRODUCT", Function::Product),
    ("ABS", Function::Abs),
    ("ROUND", Function::Round),
    ("ROUNDUP", Function::RoundUp),
    ("ROUNDDOWN", Function::RoundDown),
    ("INT", Function::Int),
    ("MOD", Function::Mod),
    ("POWER", Function::Power),
    ("SQRT", Function::Sqrt),
    ("IF", Function::If),
    ("AND", Function::And),
    ("OR", Function::Or),
    ("NOT", Function::Not),
    ("IFERROR", Function::IfError),
    ("SIGN", Function::Sign),
    ("CEILING", Function::Ceiling),
    ("FLOOR", Function::Floor),
    ("TRUNC", Function::Trunc),
    ("EXP", Function::Exp),
    ("LN", Function::Ln),
    ("LOG", Function::Log),
    ("LOG10", Function::Log10),
    ("PI", Function::Pi),
    ("LEN", Function::Len),
    ("LEFT", Function::Left),
    ("RIGHT", Function::Right),
    ("MID", Function::Mid),
    ("TRIM", Function::Trim),
    ("UPPER", Function::Upper),
    ("LOWER", Function::Lower),
    ("CONCAT", Function::Concat),
    ("CONCATENATE", Function::Concat),
    ("VALUE", Function::Value),
    ("EXACT", Function::Exact),
    ("COUNTIF", Function::CountIf),
    ("SUMIF", Function::SumIf),
    ("COUNTIFS", Function::CountIfs),
    ("SUMIFS", Function::SumIfs),
    ("AVERAGEIF", Function::AverageIf),
    ("AVERAGEIFS", Function::AverageIfs),
    ("INDEX", Function::Index),
    ("MATCH", Function::Match),
    ("VLOOKUP", Function::VLookup),
    ("XLOOKUP", Function::XLookup),
    ("DATE", Function::Date),
    ("YEAR", Function::Year),
    ("MONTH", Function::Month),
    ("DAY", Function::Day),
    ("EDATE", Function::EDate),
    ("EOMONTH", Function::EoMonth),
    ("WEEKDAY", Function::Weekday),
    ("YEARFRAC", Function::YearFrac),
    ("DAYS360", Function::Days360),
    ("NETWORKDAYS", Function::NetworkDays),
    ("WORKDAY", Function::WorkDay),
    ("LOOKUP", Function::Lookup),
    ("PMT", Function::Pmt),
    ("NPV", Function::Npv),
    ("XNPV", Function::Xnpv),
    ("XIRR", Function::Xirr),
    ("NORMDIST", Function::NormDist),
    ("AVERAGEA", Function::AverageA),
    ("CORREL", Function::Correl),
    ("ISBLANK", Function::IsBlank),
    ("ISNUMBER", Function::IsNumber),
    ("ISTEXT", Function::IsText),
    ("ISLOGICAL", Function::IsLogical),
    ("ISERROR", Function::IsError),
    ("N", Function::N),
    ("T", Function::T),
    ("SUMPRODUCT", Function::SumProduct),
    ("MEDIAN", Function::Median),
    ("CHOOSE", Function::Choose),
    ("SUBTOTAL", Function::SubTotal),
    ("STDEV", Function::StDev),
    ("STDEV.S", Function::StDev),
    ("STDEVP", Function::StDevP),
    ("STDEV.P", Function::StDevP),
    ("VAR", Function::Var),
    ("VAR.S", Function::Var),
    ("VARP", Function::VarP),
    ("VAR.P", Function::VarP),
    ("NA", Function::Na),
    ("ISNA", Function::IsNa),
    ("HLOOKUP", Function::HLookup),
    ("FIND", Function::Find),
    ("REPT", Function::Rept),
    ("ROW", Function::Row),
    ("COLUMN", Function::Column),
];

/// The supported function names in registry order.
pub fn supported_function_names() -> impl Iterator<Item = &'static str> {
    FUNCTION_REGISTRY.iter().map(|(name, _)| *name)
}

fn parse_function_name(name: &str) -> Result<Function, FormulaError> {
    let upper = name.to_ascii_uppercase();
    FUNCTION_REGISTRY
        .iter()
        .find(|(candidate, _)| *candidate == upper)
        .map(|(_, function)| *function)
        .ok_or(FormulaError::UnsupportedFunction(upper))
}

fn parse_a1(reference: &str, sheet: u32) -> Result<CellId, FormulaError> {
    let normalized = reference.replace('$', "").to_ascii_uppercase();
    let split = normalized
        .find(|character: char| character.is_ascii_digit())
        .ok_or_else(|| FormulaError::InvalidReference(reference.into()))?;
    let (column_text, row_text) = normalized.split_at(split);
    if column_text.is_empty()
        || row_text.is_empty()
        || !column_text.bytes().all(|byte| byte.is_ascii_uppercase())
        || !row_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(FormulaError::InvalidReference(reference.into()));
    }
    let mut column = 0_u32;
    for byte in column_text.bytes() {
        column = column
            .checked_mul(26)
            .and_then(|value| value.checked_add(u32::from(byte - b'A') + 1))
            .ok_or_else(|| FormulaError::InvalidReference(reference.into()))?;
    }
    let row = row_text
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| FormulaError::InvalidReference(reference.into()))?;
    Ok(CellId::new(sheet, row - 1, column - 1))
}

fn expand_range(first: CellId, second: CellId) -> Result<Expr, FormulaError> {
    let first_row = first.row.min(second.row);
    let last_row = first.row.max(second.row);
    let first_column = first.column.min(second.column);
    let last_column = first.column.max(second.column);
    let rows =
        usize::try_from(last_row - first_row + 1).map_err(|_| FormulaError::RangeTooLarge)?;
    let columns =
        usize::try_from(last_column - first_column + 1).map_err(|_| FormulaError::RangeTooLarge)?;
    let count = rows
        .checked_mul(columns)
        .ok_or(FormulaError::RangeTooLarge)?;
    if count > MAX_RANGE_CELLS {
        return Err(FormulaError::RangeTooLarge);
    }
    Ok(Expr::Range {
        anchor: CellId::new(first.sheet, first_row, first_column),
        members: None,
        rows,
        columns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(row: u32, column: u32) -> CellId {
        CellId::new(0, row, column)
    }

    #[test]
    fn evaluates_precedence_references_and_sum_ranges() {
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 3.0);
        workbook.set_number(cell(1, 0), 5.0);
        workbook
            .set_formula(cell(0, 1), "=SUM($A$1:A2) * 2 + 1")
            .unwrap();
        assert_eq!(workbook.value(cell(0, 1)), Value::Number(17.0));
    }

    #[test]
    fn recalculates_only_the_dirty_transitive_closure() {
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 2.0);
        workbook.set_formula(cell(0, 1), "=A1 + 1").unwrap();
        workbook.set_formula(cell(0, 2), "=B1 * 4").unwrap();
        workbook.set_formula(cell(0, 3), "=99").unwrap();

        let report = workbook.set_number(cell(0, 0), 5.0);
        assert_eq!(report.evaluated, vec![cell(0, 0), cell(0, 1), cell(0, 2)]);
        assert_eq!(workbook.value(cell(0, 2)), Value::Number(24.0));
        assert_eq!(workbook.value(cell(0, 3)), Value::Number(99.0));
    }

    #[test]
    fn rejects_cycles_without_mutating_the_previous_formula() {
        let mut workbook = Workbook::default();
        workbook.set_formula(cell(0, 0), "=1").unwrap();
        workbook.set_formula(cell(0, 1), "=A1 + 1").unwrap();

        let error = workbook.set_formula(cell(0, 0), "=B1 + 1").unwrap_err();
        assert_eq!(
            error,
            FormulaError::Cycle(vec![cell(0, 0), cell(0, 1), cell(0, 0)])
        );
        assert_eq!(workbook.value(cell(0, 0)), Value::Number(1.0));
        assert_eq!(workbook.value(cell(0, 1)), Value::Number(2.0));
    }

    #[test]
    fn propagates_explicit_calculation_errors() {
        let mut workbook = Workbook::default();
        workbook.set_formula(cell(0, 0), "=1 / 0").unwrap();
        workbook.set_formula(cell(0, 1), "=A1 + 2").unwrap();
        assert_eq!(
            workbook.value(cell(0, 1)),
            Value::Error(CalcError::DivisionByZero)
        );
    }

    #[test]
    fn constructs_a_long_append_only_chain_without_quadratic_cycle_walks() {
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 1.0);
        for row in 1..10_000 {
            workbook
                .set_formula(cell(row, 0), &format!("=A{row} + 1"))
                .unwrap();
        }
        assert_eq!(workbook.value(cell(9_999, 0)), Value::Number(10_000.0));
        let error = workbook.set_formula(cell(0, 0), "=A10000 + 1").unwrap_err();
        let FormulaError::Cycle(path) = error else {
            panic!("expected a cycle");
        };
        assert_eq!(path.len(), 10_001);
        assert_eq!(workbook.value(cell(0, 0)), Value::Number(1.0));
    }

    #[test]
    fn waits_for_every_dirty_input_at_a_diamond_join() {
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 2.0);
        workbook.set_formula(cell(0, 1), "=A1 + 1").unwrap();
        workbook.set_formula(cell(0, 2), "=A1 * 10").unwrap();
        workbook.set_formula(cell(0, 3), "=B1 + C1").unwrap();

        let report = workbook.set_number(cell(0, 0), 4.0);
        assert_eq!(
            report.evaluated,
            vec![cell(0, 0), cell(0, 1), cell(0, 2), cell(0, 3)]
        );
        assert_eq!(workbook.value(cell(0, 3)), Value::Number(45.0));
    }

    #[test]
    fn replacing_a_formula_removes_its_old_dependency_edge() {
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 1.0);
        workbook.set_number(cell(0, 1), 10.0);
        workbook.set_formula(cell(0, 2), "=A1 + 1").unwrap();
        workbook.set_formula(cell(0, 2), "=B1 + 1").unwrap();

        let report = workbook.set_number(cell(0, 0), 2.0);
        assert_eq!(report.evaluated, vec![cell(0, 0)]);
        assert_eq!(workbook.value(cell(0, 2)), Value::Number(11.0));
    }

    #[test]
    fn evaluates_aggregate_functions_over_typed_ranges() {
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 2.0);
        workbook.set_number(cell(1, 0), 4.0);
        workbook.set_text(cell(2, 0), "note");
        workbook.clear(cell(3, 0));

        for (column, formula, expected) in [
            (1, "=AVERAGE(A1:A4)", 3.0),
            (2, "=MIN(A1:A4)", 2.0),
            (3, "=MAX(A1:A4)", 4.0),
            (4, "=COUNT(A1:A4)", 2.0),
            (5, "=COUNTA(A1:A4)", 3.0),
            (6, "=PRODUCT(A1:A2)", 8.0),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_eq!(workbook.value(cell(0, column)), Value::Number(expected));
        }
    }

    #[test]
    fn evaluates_math_rounding_and_scientific_literals() {
        let mut workbook = Workbook::default();
        workbook
            .set_formula(
                cell(0, 0),
                "=ABS(-2)+ROUND(1.235,2)+ROUNDUP(-1.21,1)+ROUNDDOWN(1.29,1)+INT(-1.2)+MOD(7,3)+POWER(2,3)+SQRT(9)+1e2",
            )
            .unwrap();
        let Value::Number(value) = workbook.value(cell(0, 0)) else {
            panic!("expected a number");
        };
        assert!((value - 113.14).abs() < 1e-9);
    }

    #[test]
    fn evaluates_comparisons_and_lazy_logical_branches() {
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 4.0);
        workbook
            .set_formula(cell(0, 1), "=IF(A1>=4,10,1/0)")
            .unwrap();
        workbook.set_formula(cell(0, 2), "=IFERROR(1/0,7)").unwrap();
        workbook
            .set_formula(cell(0, 3), "=AND(TRUE,A1>3,NOT(FALSE))")
            .unwrap();
        workbook
            .set_formula(cell(0, 4), "=OR(FALSE,A1<3,A1<>0)")
            .unwrap();

        assert_eq!(workbook.value(cell(0, 1)), Value::Number(10.0));
        assert_eq!(workbook.value(cell(0, 2)), Value::Number(7.0));
        assert_eq!(workbook.value(cell(0, 3)), Value::Boolean(true));
        assert_eq!(workbook.value(cell(0, 4)), Value::Boolean(true));
    }

    #[test]
    fn parses_typed_literals_and_reports_bad_function_arity() {
        let mut workbook = Workbook::default();
        workbook
            .set_formula(cell(0, 0), "=\"agent \"\"safe\"\"\"")
            .unwrap();
        workbook.set_formula(cell(0, 1), "=ABS(1,2)").unwrap();
        assert_eq!(
            workbook.value(cell(0, 0)),
            Value::Text("agent \"safe\"".into())
        );
        assert_eq!(
            workbook.value(cell(0, 1)),
            Value::Error(CalcError::InvalidArguments)
        );
    }

    #[test]
    fn evaluates_the_extended_math_head() {
        let mut workbook = Workbook::default();
        for (column, formula, expected) in [
            (0, "=SIGN(-2)", -1.0),
            (1, "=CEILING(4.2,1)", 5.0),
            (2, "=FLOOR(4.8,1)", 4.0),
            (3, "=TRUNC(1.239,2)", 1.23),
            (4, "=LN(EXP(1))", 1.0),
            (5, "=LOG(100)", 2.0),
            (6, "=LOG(8,2)", 3.0),
            (7, "=LOG10(1000)", 3.0),
            (8, "=PI()", std::f64::consts::PI),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            let Value::Number(value) = workbook.value(cell(0, column)) else {
                panic!("expected a number for {formula}");
            };
            assert!((value - expected).abs() < 1e-9, "{formula}");
        }
    }

    #[test]
    fn evaluates_common_text_functions_with_unicode_characters() {
        let mut workbook = Workbook::default();
        for (column, formula, expected) in [
            (0, "=LEN(\"Grüße\")", Value::Number(5.0)),
            (1, "=LEFT(\"Agent\",2)", Value::Text("Ag".into())),
            (2, "=RIGHT(\"Agent\",2)", Value::Text("nt".into())),
            (3, "=MID(\"Agent\",2,3)", Value::Text("gen".into())),
            (
                4,
                "=TRIM(\"  agent   safe  \")",
                Value::Text("agent safe".into()),
            ),
            (5, "=UPPER(\"Agent\")", Value::Text("AGENT".into())),
            (6, "=LOWER(\"Agent\")", Value::Text("agent".into())),
            (
                7,
                "=CONCAT(\"agent\",\"-\",42)",
                Value::Text("agent-42".into()),
            ),
            (8, "=VALUE(\" 42.5 \")", Value::Number(42.5)),
            (9, "=EXACT(\"Agent\",\"agent\")", Value::Boolean(false)),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_eq!(workbook.value(cell(0, column)), expected, "{formula}");
        }
    }

    #[test]
    fn evaluates_countif_and_sumif_without_losing_range_alignment() {
        let mut workbook = Workbook::default();
        for row in 0..5 {
            workbook.set_number(cell(row, 0), f64::from(row + 1));
            workbook.set_number(cell(row, 1), f64::from((row + 1) * 10));
        }
        workbook
            .set_formula(cell(0, 2), "=COUNTIF(A1:A5,\">2\")")
            .unwrap();
        workbook
            .set_formula(cell(1, 2), "=SUMIF(A1:A5,\">=3\",B1:B5)")
            .unwrap();
        assert_eq!(workbook.value(cell(0, 2)), Value::Number(3.0));
        assert_eq!(workbook.value(cell(1, 2)), Value::Number(120.0));
    }

    #[test]
    fn evaluates_multi_criteria_and_average_aggregates() {
        let mut workbook = Workbook::default();
        for (row, group, active, value) in [
            (0, "A", true, 10.0),
            (1, "A", false, 20.0),
            (2, "B", true, 30.0),
            (3, "A", true, 40.0),
        ] {
            workbook.set_text(cell(row, 0), group);
            workbook.set_boolean(cell(row, 1), active);
            workbook.set_number(cell(row, 2), value);
        }

        for (column, formula, expected) in [
            (3, "=COUNTIFS(A1:A4,\"A\",B1:B4,TRUE)", 2.0),
            (4, "=SUMIFS(C1:C4,A1:A4,\"A\",B1:B4,TRUE)", 50.0),
            (5, "=AVERAGEIF(A1:A4,\"A\",C1:C4)", 70.0 / 3.0),
            (6, "=AVERAGEIFS(C1:C4,A1:A4,\"A\",B1:B4,TRUE)", 25.0),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            let Value::Number(value) = workbook.value(cell(0, column)) else {
                panic!("expected number for {formula}");
            };
            assert!((value - expected).abs() < 1e-9, "{formula}");
        }
    }

    #[test]
    fn evaluates_exact_lookup_functions_over_rectangular_ranges() {
        let mut workbook = Workbook::default();
        for (row, key, value) in [(0, "Alpha", 10.0), (1, "Beta", 20.0), (2, "Gamma", 30.0)] {
            workbook.set_text(cell(row, 0), key);
            workbook.set_number(cell(row, 1), value);
        }

        for (column, formula, expected) in [
            (2, "=INDEX(B1:B3,2)", Value::Number(20.0)),
            (3, "=INDEX(A1:B3,3,2)", Value::Number(30.0)),
            (4, "=MATCH(\"beta\",A1:A3,0)", Value::Number(2.0)),
            (5, "=VLOOKUP(\"Gamma\",A1:B3,2,FALSE)", Value::Number(30.0)),
            (6, "=XLOOKUP(\"Beta\",A1:A3,B1:B3)", Value::Number(20.0)),
            (
                7,
                "=XLOOKUP(\"Missing\",A1:A3,B1:B3,\"not found\")",
                Value::Text("not found".into()),
            ),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_eq!(workbook.value(cell(0, column)), expected, "{formula}");
        }

        workbook
            .set_formula(cell(1, 2), "=VLOOKUP(\"Beta\",A1:B3,2,TRUE)")
            .unwrap();
        assert_eq!(workbook.value(cell(1, 2)), Value::Number(20.0));
        workbook
            .set_formula(cell(1, 3), "=VLOOKUP(\"Zeta\",A1:B3,2,FALSE)")
            .unwrap();
        assert_eq!(
            workbook.value(cell(1, 3)),
            Value::Error(CalcError::NotAvailable)
        );
    }

    #[test]
    fn resolves_quoted_and_unquoted_cross_sheet_references() {
        let mut workbook = Workbook::default();
        workbook.define_sheet(0, "Summary");
        workbook.define_sheet(1, "Inputs");
        workbook.define_sheet(2, "Owner's Data");
        workbook.set_number(CellId::new(1, 0, 0), 2.0);
        workbook.set_number(CellId::new(1, 1, 0), 3.0);
        workbook.set_number(CellId::new(2, 0, 0), 5.0);
        workbook
            .set_formula(
                CellId::new(0, 0, 0),
                "=SUM(Inputs!A1:A2)+'Owner''s Data'!A1",
            )
            .unwrap();
        assert_eq!(workbook.value(CellId::new(0, 0, 0)), Value::Number(10.0));

        let error = workbook
            .set_formula(CellId::new(0, 0, 0), "=Missing!A1")
            .unwrap_err();
        assert_eq!(error, FormulaError::UnknownSheet("Missing".into()));
        assert_eq!(workbook.value(CellId::new(0, 0, 0)), Value::Number(10.0));
    }

    #[test]
    fn evaluates_date_functions_on_serials_with_the_1900_quirk() {
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 45_323.6); // 2024-02-01 14:24
        workbook.set_number(cell(1, 0), 60.0); // Excel's fictitious 1900-02-29
        workbook.set_text(cell(2, 0), "2024-02-01");
        workbook.set_boolean(cell(3, 0), true);

        for (column, formula, expected) in [
            (1, "=DATE(2024,2,1)", Value::Number(45_323.0)),
            (2, "=DATE(1900,2,29)", Value::Number(60.0)),
            (3, "=DATE(1900,1,0)", Value::Number(0.0)),
            (4, "=YEAR(A1)", Value::Number(2024.0)),
            (5, "=MONTH(A1)", Value::Number(2.0)),
            (6, "=DAY(A1)", Value::Number(1.0)),
            (7, "=YEAR(A2)+MONTH(A2)+DAY(A2)", Value::Number(1931.0)),
            (8, "=EDATE(A1,1)", Value::Number(45_352.0)),
            (9, "=EDATE(DATE(2024,1,31),1)", Value::Number(45_351.0)),
            (10, "=EOMONTH(A1,0)", Value::Number(45_351.0)),
            (11, "=EOMONTH(A1,-1)", Value::Number(45_322.0)),
            (12, "=WEEKDAY(A1)", Value::Number(5.0)),
            (13, "=WEEKDAY(A1,2)", Value::Number(4.0)),
            (14, "=DAY(A5)", Value::Number(0.0)),
            (15, "=YEAR(A3)", Value::Error(CalcError::InvalidValue)),
            (16, "=YEAR(A4)", Value::Error(CalcError::InvalidValue)),
            (17, "=YEAR(-1)", Value::Error(CalcError::InvalidNumber)),
            (
                18,
                "=DATE(10000,1,1)",
                Value::Error(CalcError::InvalidNumber),
            ),
            (19, "=WEEKDAY(A1,4)", Value::Error(CalcError::InvalidNumber)),
            (20, "=EDATE(A1)", Value::Error(CalcError::InvalidArguments)),
            (21, "=YEAR(A1:A2)", Value::Number(2024.0)),
            (22, "=IFERROR(YEAR(A3),YEAR(A1))", Value::Number(2024.0)),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_eq!(workbook.value(cell(0, column)), expected, "{formula}");
        }

        let report = workbook.set_number(cell(0, 0), 45_292.0); // 2024-01-01
        assert!(report.evaluated.contains(&cell(0, 4)));
        assert_eq!(workbook.value(cell(0, 5)), Value::Number(1.0));
        assert_eq!(workbook.value(cell(0, 8)), Value::Number(45_323.0));
    }

    #[test]
    fn applies_excel_operator_precedence_for_power_percent_and_concatenation() {
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 3.0);
        workbook.set_text(cell(1, 0), "x");
        for (column, formula, expected) in [
            (1, "=-2^2", Value::Number(4.0)),
            (2, "=2^3^2", Value::Number(64.0)),
            (3, "=2*3^2+1", Value::Number(19.0)),
            (4, "=50%*2", Value::Number(1.0)),
            (5, "=10%%", Value::Number(0.001)),
            (6, "=A1^2%", Value::Number(3_f64.powf(0.02))),
            (7, "=\"a\"&1&TRUE&A2", Value::Text("a1TRUEx".into())),
            (8, "=1+2&\"x\"", Value::Text("3x".into())),
            (9, "=\"1\"&\"2\"=12", Value::Boolean(false)),
            (10, "=+A1--A1", Value::Number(6.0)),
            (11, "=4^0.5", Value::Number(2.0)),
            (12, "=0^0", Value::Error(CalcError::InvalidNumber)),
            (13, "=(-8)^(1/3)", Value::Error(CalcError::InvalidNumber)),
            (14, "=10^400", Value::Error(CalcError::InvalidNumber)),
            (15, "=A2^2", Value::Error(CalcError::InvalidValue)),
            (16, "=A2%", Value::Error(CalcError::InvalidValue)),
            (17, "=\"a\"&1/0", Value::Error(CalcError::DivisionByZero)),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_eq!(workbook.value(cell(0, column)), expected, "{formula}");
        }
        assert_eq!(
            workbook.set_formula(cell(2, 0), "=2^"),
            Err(FormulaError::UnexpectedToken(2))
        );
        assert_eq!(
            workbook.set_formula(cell(2, 0), "=%2"),
            Err(FormulaError::UnexpectedToken(0))
        );
    }

    #[test]
    fn inspects_value_types_without_propagating_errors() {
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 2.0);
        workbook.set_text(cell(1, 0), "note");
        workbook.set_boolean(cell(2, 0), true);
        workbook.clear(cell(3, 0));
        workbook.set_formula(cell(4, 0), "=1/0").unwrap();
        for (column, formula, expected) in [
            (1, "=ISBLANK(A4)", Value::Boolean(true)),
            (2, "=ISBLANK(A5)", Value::Boolean(false)),
            (3, "=ISNUMBER(A1)", Value::Boolean(true)),
            (4, "=ISNUMBER(A2)", Value::Boolean(false)),
            (5, "=ISTEXT(A2)", Value::Boolean(true)),
            (6, "=ISLOGICAL(A3)", Value::Boolean(true)),
            (7, "=ISERROR(A5)", Value::Boolean(true)),
            (8, "=ISERROR(A1)", Value::Boolean(false)),
            (9, "=N(A3)", Value::Number(1.0)),
            (10, "=N(A2)", Value::Number(0.0)),
            (11, "=N(A5)", Value::Error(CalcError::DivisionByZero)),
            (12, "=T(A2)", Value::Text("note".into())),
            (13, "=T(A1)", Value::Text(String::new())),
            (
                14,
                "=ISNUMBER(A1:A2)",
                Value::Error(CalcError::InvalidArguments),
            ),
            (15, "=ISBLANK()", Value::Error(CalcError::InvalidArguments)),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_eq!(workbook.value(cell(0, column)), expected, "{formula}");
        }
    }

    #[test]
    fn evaluates_sumproduct_and_median_over_aligned_ranges() {
        let mut workbook = Workbook::default();
        for (row, quantity, price) in [(0, 2.0, 10.0), (1, 3.0, 20.0), (2, 4.0, 30.0)] {
            workbook.set_number(cell(row, 0), quantity);
            workbook.set_number(cell(row, 1), price);
        }
        workbook.set_text(cell(3, 0), "n/a");
        workbook.set_number(cell(3, 1), 40.0);
        for (column, formula, expected) in [
            (2, "=SUMPRODUCT(A1:A3,B1:B3)", Value::Number(200.0)),
            (3, "=SUMPRODUCT(A1:A4,B1:B4)", Value::Number(200.0)),
            (4, "=SUMPRODUCT(A1:A3)", Value::Number(9.0)),
            (5, "=SUMPRODUCT(A1,B1)", Value::Number(20.0)),
            (
                6,
                "=SUMPRODUCT(A1:A3,B1:B2)",
                Value::Error(CalcError::InvalidValue),
            ),
            (
                7,
                "=SUMPRODUCT()",
                Value::Error(CalcError::InvalidArguments),
            ),
            (8, "=MEDIAN(B1:B4)", Value::Number(25.0)),
            (9, "=MEDIAN(A1:A4)", Value::Number(3.0)),
            (10, "=MEDIAN(A4)", Value::Error(CalcError::InvalidArguments)),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_eq!(workbook.value(cell(0, column)), expected, "{formula}");
        }
        workbook.set_formula(cell(4, 0), "=1/0").unwrap();
        workbook
            .set_formula(cell(1, 2), "=SUMPRODUCT(A1:A5,B1:B5)")
            .unwrap();
        assert_eq!(
            workbook.value(cell(1, 2)),
            Value::Error(CalcError::DivisionByZero)
        );
    }

    #[test]
    fn omitted_arguments_error_literals_and_blank_results_follow_excel() {
        let mut workbook = Workbook::default();
        for (column, formula, expected) in [
            (0, "=SUM(,1,2)", Value::Number(3.0)),
            (1, "=IF(TRUE,,5)", Value::Number(0.0)),
            (2, "=IF(FALSE,1)", Value::Boolean(false)),
            (3, "=1+IF(TRUE,,5)", Value::Number(1.0)),
            (4, "=#REF!+1", Value::Error(CalcError::InvalidReference)),
            (5, "=ISNA(#N/A)", Value::Boolean(true)),
            (6, "=NA()", Value::Error(CalcError::NotAvailable)),
            (7, "=ISERROR(#DIV/0!)", Value::Boolean(true)),
            (8, "=Z99", Value::Number(0.0)),
            (9, "=\"\"", Value::Text(String::new())),
            (10, "=SUM()", Value::Number(0.0)),
            (11, "=IF(TRUE,1,)", Value::Number(1.0)),
            (12, "=IF(FALSE,1,)", Value::Number(0.0)),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_eq!(workbook.value(cell(0, column)), expected, "{formula}");
        }
        assert_eq!(
            workbook.set_formula(cell(1, 0), "=#BOGUS!"),
            Err(FormulaError::UnexpectedToken(0))
        );
    }

    #[test]
    fn external_references_and_defined_names_compile_explicitly() {
        let mut workbook = Workbook::default();
        workbook.define_sheet(0, "Data");
        workbook.set_number(cell(0, 0), 10.0);
        workbook.set_number(cell(1, 0), 20.0);
        workbook.define_name("Rates", "Data!$A$1:$A$2");
        workbook.define_name("rate_total", "SUM(Rates)*2");
        workbook.define_name("Broken", "[2]External!A1");
        workbook.define_name("Loop", "Loop+1");

        workbook.set_formula(cell(0, 1), "=SUM(Rates)").unwrap();
        workbook.set_formula(cell(0, 2), "=RATE_TOTAL+1").unwrap();
        assert_eq!(workbook.value(cell(0, 1)), Value::Number(30.0));
        assert_eq!(workbook.value(cell(0, 2)), Value::Number(61.0));
        let report = workbook.set_number(cell(0, 0), 15.0);
        assert!(report.evaluated.contains(&cell(0, 1)));
        assert_eq!(workbook.value(cell(0, 2)), Value::Number(71.0));

        assert_eq!(
            workbook.set_formula(cell(0, 3), "=Missing+1"),
            Err(FormulaError::UnknownName("Missing".into()))
        );
        assert_eq!(
            workbook.set_formula(cell(0, 3), "=Broken"),
            Err(FormulaError::UnsupportedName("Broken".into()))
        );
        assert_eq!(
            workbook.set_formula(cell(0, 3), "=Loop"),
            Err(FormulaError::UnsupportedName("Loop".into()))
        );
        assert!(matches!(
            workbook.set_formula(cell(0, 3), "='[1]Data Sheet'!B11"),
            Err(FormulaError::ExternalReference(_))
        ));
        assert!(matches!(
            workbook.set_formula(cell(0, 3), "=[3]daily!$Y$140"),
            Err(FormulaError::ExternalReference(_))
        ));
        assert!(matches!(
            workbook.set_formula(cell(0, 3), "=VLOOKUP(1,[5]Q!$A$1:$C$5,3)"),
            Err(FormulaError::ExternalReference(_))
        ));
    }

    /// Graph vertices beyond the cells that exist: the shared range nodes.
    fn range_node_count(workbook: &Workbook) -> usize {
        workbook.ranges.len()
    }

    #[test]
    fn formulas_over_the_same_range_share_one_node() {
        let mut workbook = Workbook::default();
        for row in 0..1000 {
            workbook.set_number(cell(row, 0), 1.0);
        }
        workbook.define_name("Data", "$A$1:$A$1000");
        for column in 1..=200 {
            workbook.set_formula(cell(0, column), "=SUM(Data)").unwrap();
            workbook
                .set_formula(cell(1, column), "=SUM(A1:A1000)+MAX(A1:A1000)")
                .unwrap();
        }
        assert_eq!(range_node_count(&workbook), 1);
        // Rectangles keep no per-member edges; the node's readers are the
        // only edges the range costs.
        assert_eq!(
            workbook.cells[workbook.indices[&cell(500, 0)]]
                .dependents
                .len(),
            0
        );
        assert_eq!(workbook.statistics().dependent_edges, 400);
        let node = workbook.ranges.values().next().copied().unwrap();
        assert_eq!(workbook.cells[node].dependents.len(), 400);
        assert_eq!(workbook.value(cell(0, 200)), Value::Number(1000.0));
        assert_eq!(workbook.value(cell(1, 200)), Value::Number(1001.0));

        let report = workbook.set_number(cell(999, 0), 5.0);
        assert_eq!(
            report.evaluated.len(),
            401,
            "changed cell plus every reader, no node"
        );
        assert_eq!(workbook.value(cell(0, 200)), Value::Number(1004.0));
        assert_eq!(workbook.value(cell(1, 200)), Value::Number(1009.0));

        // Replacing a reader drops it from the node without disturbing others.
        workbook.set_number(cell(0, 1), 0.0);
        assert_eq!(workbook.cells[node].dependents.len(), 399);
        assert_eq!(range_node_count(&workbook), 1);
        workbook.set_number(cell(998, 0), 5.0);
        assert_eq!(workbook.value(cell(0, 2)), Value::Number(1008.0));
    }

    #[test]
    fn bulk_loads_evaluate_once_and_land_the_same_values() {
        let mut direct = Workbook::default();
        let mut bulk = Workbook::default();
        bulk.begin_bulk();
        for workbook in [&mut direct, &mut bulk] {
            workbook.set_formula(cell(0, 1), "=SUM(A1:A1000)").unwrap();
            for row in 0..1000 {
                workbook.set_number(cell(row, 0), 1.0);
            }
            workbook.set_formula(cell(1, 1), "=B1*2").unwrap();
        }
        assert_eq!(
            bulk.value(cell(0, 1)),
            Value::Blank,
            "nothing evaluates until end_bulk"
        );
        let report = bulk.end_bulk();
        assert_eq!(report.evaluated.len(), 1002);
        assert_eq!(bulk.value(cell(0, 1)), Value::Number(1000.0));
        assert_eq!(bulk.value(cell(1, 1)), direct.value(cell(1, 1)));
        assert_eq!(bulk.end_bulk(), RecalcReport::default());
        assert!(matches!(
            bulk.set_formula(cell(500, 0), "=B2"),
            Err(FormulaError::Cycle(_))
        ));
        let report = bulk.set_number(cell(999, 0), 2.0);
        assert_eq!(report.evaluated.len(), 3);
        assert_eq!(bulk.value(cell(1, 1)), Value::Number(2002.0));
    }

    #[test]
    fn a_formula_inside_its_own_range_is_a_cycle_named_by_the_member_cell() {
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 1.0);
        assert_eq!(
            workbook.set_formula(cell(4, 0), "=SUM(A1:A10)"),
            Err(FormulaError::Cycle(vec![cell(4, 0), cell(4, 0)]))
        );
        workbook.set_formula(cell(0, 1), "=SUM(A1:A3)").unwrap();
        assert!(matches!(
            workbook.set_formula(cell(2, 0), "=B1*2"),
            Err(FormulaError::Cycle(_))
        ));
        assert_eq!(workbook.value(cell(0, 1)), Value::Number(1.0));
    }

    #[test]
    fn rebound_ranges_stay_rectangular_or_become_explicit_member_lists() {
        let parsed = ParsedFormula::parse("=SUM(A1:B2)", 0, &HashMap::new()).unwrap();
        assert_eq!(
            parsed.references(),
            vec![cell(0, 0), cell(0, 1), cell(1, 0), cell(1, 1)]
        );
        let shifted = parsed
            .clone()
            .map_references(|id| CellId::new(id.sheet, id.row + 3, id.column + 1));
        assert!(matches!(
            &shifted.expression,
            Expr::Function(Function::Sum, arguments)
                if matches!(arguments[0], Expr::Range { anchor, members: None, rows: 2, columns: 2 }
                    if anchor == cell(3, 1))
        ));
        // A row inserted through the middle of the range keeps the original
        // four cells, which no longer form a rectangle.
        let split = parsed.map_references(|id| {
            CellId::new(
                id.sheet,
                if id.row >= 1 { id.row + 1 } else { id.row },
                id.column,
            )
        });
        assert!(matches!(
            &split.expression,
            Expr::Function(Function::Sum, arguments)
                if matches!(&arguments[0], Expr::Range { members: Some(members), rows: 2, columns: 2, .. }
                    if members == &vec![cell(0, 0), cell(0, 1), cell(2, 0), cell(2, 1)])
        ));
        let mut workbook = Workbook::default();
        for (row, column, value) in [
            (0, 0, 1.0),
            (0, 1, 2.0),
            (1, 0, 100.0),
            (1, 1, 100.0),
            (2, 0, 3.0),
            (2, 1, 4.0),
        ] {
            workbook.set_number(cell(row, column), value);
        }
        workbook.set_parsed_formula(cell(5, 5), split).unwrap();
        assert_eq!(workbook.value(cell(5, 5)), Value::Number(10.0));
        workbook
            .set_formula(cell(6, 5), "=INDEX(A1:B3,3,2)")
            .unwrap();
        assert_eq!(workbook.value(cell(6, 5)), Value::Number(4.0));
        assert_eq!(range_node_count(&workbook), 2);
    }

    #[test]
    fn ranges_in_scalar_position_intersect_with_the_formula_cell() {
        let mut workbook = Workbook::default();
        for row in 0..5 {
            workbook.set_number(cell(row, 0), f64::from(row + 1) * 10.0);
            workbook.set_number(cell(0, row + 2), f64::from(row + 1));
        }
        workbook.set_formula(cell(2, 1), "=A1:A5").unwrap();
        assert_eq!(workbook.value(cell(2, 1)), Value::Number(30.0));
        workbook.set_formula(cell(2, 1), "=A1:A5*2").unwrap();
        assert_eq!(workbook.value(cell(2, 1)), Value::Number(60.0));
        workbook.set_formula(cell(1, 4), "=C1:G1").unwrap();
        assert_eq!(workbook.value(cell(1, 4)), Value::Number(3.0));
        workbook.set_formula(cell(7, 1), "=A1:A5").unwrap();
        assert_eq!(
            workbook.value(cell(7, 1)),
            Value::Error(CalcError::InvalidValue)
        );
        // A two-dimensional range intersects on both axes; a formula outside
        // the range's columns gets #VALUE!, and one inside would be circular.
        workbook.set_formula(cell(1, 8), "=A1:G5").unwrap();
        assert_eq!(
            workbook.value(cell(1, 8)),
            Value::Error(CalcError::InvalidValue)
        );
        assert!(matches!(
            workbook.set_formula(cell(1, 3), "=A1:G5"),
            Err(FormulaError::Cycle(_))
        ));
        workbook.set_formula(cell(4, 1), "=ROW()").unwrap();
        workbook.set_formula(cell(4, 2), "=COLUMN()").unwrap();
        workbook
            .set_formula(cell(4, 3), "=ROW(B7)+COLUMN(A1:A3)")
            .unwrap();
        assert_eq!(workbook.value(cell(4, 1)), Value::Number(5.0));
        assert_eq!(workbook.value(cell(4, 2)), Value::Number(3.0));
        assert_eq!(workbook.value(cell(4, 3)), Value::Number(8.0));
    }

    #[test]
    fn comparisons_use_excel_type_order_and_case_folding() {
        let mut workbook = Workbook::default();
        workbook.set_text(cell(0, 0), "Alpha");
        for (column, formula, expected) in [
            (1, "=B9=\"\"", Value::Boolean(true)),
            (2, "=B9=0", Value::Boolean(true)),
            (3, "=B9=FALSE", Value::Boolean(true)),
            (4, "=\"a\"=\"A\"", Value::Boolean(true)),
            (5, "=1=\"1\"", Value::Boolean(false)),
            (6, "=\"b\">\"a\"", Value::Boolean(true)),
            (7, "=1<\"a\"", Value::Boolean(true)),
            (8, "=TRUE>\"z\"", Value::Boolean(true)),
            (9, "=A1<>\"alpha\"", Value::Boolean(false)),
            (10, "=IF(B9=\"\",\"\",1)", Value::Text(String::new())),
            (11, "=2>=2", Value::Boolean(true)),
            (12, "=#N/A=1", Value::Error(CalcError::NotAvailable)),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_eq!(workbook.value(cell(0, column)), expected, "{formula}");
        }
    }

    #[test]
    fn choose_subtotal_and_dispersion_functions_follow_excel() {
        let mut workbook = Workbook::default();
        for (row, value) in [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]
            .into_iter()
            .enumerate()
        {
            workbook.set_number(cell(row as u32, 0), value);
        }
        workbook
            .set_formula(cell(8, 0), "=SUBTOTAL(9,A1:A8)")
            .unwrap();
        workbook
            .set_formula(cell(9, 0), "=SUBTOTAL(109,A1:A9)")
            .unwrap();
        assert_eq!(workbook.value(cell(8, 0)), Value::Number(40.0));
        assert_eq!(workbook.value(cell(9, 0)), Value::Number(40.0));
        for (column, formula, expected) in [
            (1, "=CHOOSE(2,\"a\",\"b\",\"c\")", Value::Text("b".into())),
            (2, "=CHOOSE(1.9,5,1/0)", Value::Number(5.0)),
            (3, "=CHOOSE(0,1,2)", Value::Error(CalcError::InvalidValue)),
            (4, "=CHOOSE(3,1,2)", Value::Error(CalcError::InvalidValue)),
            (5, "=SUBTOTAL(1,A1:A8)", Value::Number(5.0)),
            (6, "=SUBTOTAL(2,A1:A10)", Value::Number(8.0)),
            (7, "=SUBTOTAL(4,A1:A10)", Value::Number(9.0)),
            (
                8,
                "=SUBTOTAL(12,A1:A8)",
                Value::Error(CalcError::InvalidValue),
            ),
            (9, "=SUBTOTAL(9,A11:A12)", Value::Number(0.0)),
            (
                10,
                "=SUBTOTAL(1,A11:A12)",
                Value::Error(CalcError::DivisionByZero),
            ),
            (11, "=ROUND(STDEV(A1:A8),5)", Value::Number(2.13809)),
            (12, "=STDEVP(A1:A8)", Value::Number(2.0)),
            (13, "=ROUND(VAR.S(A1:A8),6)", Value::Number(4.571429)),
            (14, "=VAR.P(A1:A8)", Value::Number(4.0)),
            (15, "=STDEV(A1)", Value::Error(CalcError::DivisionByZero)),
            (16, "=ROUND(SUBTOTAL(7,A1:A9),5)", Value::Number(2.13809)),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_eq!(workbook.value(cell(0, column)), expected, "{formula}");
        }
    }

    #[test]
    fn approximate_lookups_binary_search_sorted_keys() {
        let mut workbook = Workbook::default();
        for (row, (key, label)) in [(10.0, "ten"), (20.0, "twenty"), (30.0, "thirty")]
            .into_iter()
            .enumerate()
        {
            workbook.set_number(cell(row as u32, 0), key);
            workbook.set_text(cell(row as u32, 1), label);
            workbook.set_number(cell(5, row as u32), key);
            workbook.set_text(cell(6, row as u32), label);
        }
        for (column, formula, expected) in [
            (2, "=VLOOKUP(25,A1:B3,2)", Value::Text("twenty".into())),
            (3, "=VLOOKUP(30,A1:B3,2,TRUE)", Value::Text("thirty".into())),
            (
                4,
                "=VLOOKUP(5,A1:B3,2)",
                Value::Error(CalcError::NotAvailable),
            ),
            (5, "=VLOOKUP(20,A1:B3,2,0)", Value::Text("twenty".into())),
            (
                6,
                "=VLOOKUP(25,A1:B3,2,0)",
                Value::Error(CalcError::NotAvailable),
            ),
            (
                7,
                "=VLOOKUP(\"x\",A1:B3,2)",
                Value::Error(CalcError::NotAvailable),
            ),
            (
                8,
                "=VLOOKUP(25,A1:B3,3)",
                Value::Error(CalcError::InvalidReference),
            ),
            (9, "=VLOOKUP(25,A1:B5,2)", Value::Text("twenty".into())),
            (10, "=HLOOKUP(25,A6:C7,2)", Value::Text("twenty".into())),
            (
                11,
                "=HLOOKUP(20,A6:C7,2,FALSE)",
                Value::Text("twenty".into()),
            ),
            (12, "=MATCH(25,A1:A3)", Value::Number(2.0)),
            (13, "=MATCH(25,A1:A3,1)", Value::Number(2.0)),
            (
                14,
                "=MATCH(25,A1:A3,0)",
                Value::Error(CalcError::NotAvailable),
            ),
            (15, "=MATCH(35,A1:A3,1)", Value::Number(3.0)),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_eq!(workbook.value(cell(0, column)), expected, "{formula}");
        }
        for (row, key) in [30.0, 20.0, 10.0].into_iter().enumerate() {
            workbook.set_number(cell(row as u32 + 10, 0), key);
        }
        workbook
            .set_formula(cell(1, 2), "=MATCH(25,A11:A13,-1)")
            .unwrap();
        assert_eq!(workbook.value(cell(1, 2)), Value::Number(1.0));
        workbook
            .set_formula(cell(1, 3), "=MATCH(5,A11:A13,-1)")
            .unwrap();
        assert_eq!(workbook.value(cell(1, 3)), Value::Number(3.0));
    }

    #[test]
    fn find_and_rept_follow_excel_text_rules() {
        let mut workbook = Workbook::default();
        for (column, formula, expected) in [
            (0, "=FIND(\"x\",\"axb\")", Value::Number(2.0)),
            (
                1,
                "=FIND(\"X\",\"axb\")",
                Value::Error(CalcError::InvalidValue),
            ),
            (2, "=FIND(\"b\",\"abab\",3)", Value::Number(4.0)),
            (3, "=FIND(\"\",\"abc\")", Value::Number(1.0)),
            (
                4,
                "=FIND(\"a\",\"abc\",0)",
                Value::Error(CalcError::InvalidValue),
            ),
            (5, "=REPT(\"ab\",3)", Value::Text("ababab".into())),
            (6, "=REPT(\"ab\",0)", Value::Text(String::new())),
            (
                7,
                "=REPT(\"ab\",1e9)",
                Value::Error(CalcError::InvalidValue),
            ),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_eq!(workbook.value(cell(0, column)), expected, "{formula}");
        }
    }

    #[test]
    fn documented_function_list_matches_the_registry() {
        let documented = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/FUNCTIONS.md"
        ))
        .expect("docs/FUNCTIONS.md exists");
        let mut listed: Vec<&str> = documented
            .lines()
            .filter_map(|line| line.strip_prefix("- `"))
            .filter_map(|line| line.split('`').next())
            .collect();
        listed.sort_unstable();
        let registry: Vec<&str> = supported_function_names().collect();
        let mut sorted = registry.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), registry.len(), "registry names are unique");
        assert_eq!(
            listed, sorted,
            "docs/FUNCTIONS.md must list exactly the parser registry"
        );
        assert!(documented.contains(&format!("{} function names", registry.len())));
    }

    #[test]
    fn parsed_formulas_expose_and_rebind_references_in_a_stable_order() {
        let names = HashMap::from([("Inputs".to_string(), 1_u32)]);
        let parsed = ParsedFormula::parse("=SUM(A1:A2)+Inputs!B1*A1", 0, &names).unwrap();
        assert_eq!(
            parsed.references(),
            vec![
                CellId::new(0, 0, 0),
                CellId::new(0, 1, 0),
                CellId::new(1, 0, 1),
                CellId::new(0, 0, 0),
            ]
        );

        let rebound = parsed
            .clone()
            .map_references(|cell| CellId::new(cell.sheet, cell.row + 10, cell.column + 1));
        assert_eq!(
            rebound.references(),
            vec![
                CellId::new(0, 10, 1),
                CellId::new(0, 11, 1),
                CellId::new(1, 10, 2),
                CellId::new(0, 10, 1),
            ]
        );

        let mut workbook = Workbook::default();
        workbook.define_sheet(1, "Inputs");
        workbook.set_number(CellId::new(0, 10, 1), 2.0);
        workbook.set_number(CellId::new(0, 11, 1), 3.0);
        workbook.set_number(CellId::new(1, 10, 2), 4.0);
        workbook
            .set_parsed_formula(CellId::new(0, 0, 0), rebound)
            .unwrap();
        assert_eq!(workbook.value(CellId::new(0, 0, 0)), Value::Number(13.0));
        assert_eq!(
            workbook.set_parsed_formula(
                CellId::new(0, 10, 1),
                parsed.map_references(|_| CellId::new(0, 0, 0))
            ),
            Err(FormulaError::Cycle(vec![
                CellId::new(0, 10, 1),
                CellId::new(0, 0, 0),
                CellId::new(0, 10, 1)
            ]))
        );
        assert_eq!(
            ParsedFormula::parse("=Missing!A1", 0, &names),
            Err(FormulaError::UnknownSheet("Missing".into()))
        );
    }

    fn assert_close(actual: Value, expected: f64, tolerance: f64, label: &str) {
        match actual {
            Value::Number(number) => assert!(
                (number - expected).abs() <= tolerance * expected.abs().max(1.0),
                "{label}: {number} != {expected}"
            ),
            other => panic!("{label}: {other:?}"),
        }
    }

    #[test]
    fn day_count_and_working_day_functions_follow_excel() {
        let mut workbook = Workbook::default();
        for (column, formula, expected) in [
            (0, "=YEARFRAC(DATE(2024,1,1),DATE(2024,7,1))", 0.5),
            (
                1,
                "=YEARFRAC(DATE(2024,1,1),DATE(2024,7,1),1)",
                182.0 / 366.0,
            ),
            (
                2,
                "=YEARFRAC(DATE(2024,1,1),DATE(2024,7,1),2)",
                182.0 / 360.0,
            ),
            (
                3,
                "=YEARFRAC(DATE(2024,1,1),DATE(2024,7,1),3)",
                182.0 / 365.0,
            ),
            (4, "=YEARFRAC(DATE(2024,1,1),DATE(2024,7,1),4)", 0.5),
            (
                5,
                "=YEARFRAC(DATE(2023,1,31),DATE(2023,3,31),0)",
                60.0 / 360.0,
            ),
            (
                6,
                "=YEARFRAC(DATE(2023,3,31),DATE(2023,1,31))",
                60.0 / 360.0,
            ),
            (
                7,
                "=YEARFRAC(DATE(2021,1,1),DATE(2024,1,1),1)",
                1095.0 / (1461.0 / 4.0),
            ),
            (8, "=DAYS360(DATE(2023,1,30),DATE(2023,3,31))", 60.0),
            (9, "=DAYS360(DATE(2023,1,1),DATE(2023,1,31))", 30.0),
            (10, "=DAYS360(DATE(2023,2,28),DATE(2023,3,31))", 30.0),
            (11, "=DAYS360(DATE(2023,1,1),DATE(2023,1,31),TRUE)", 29.0),
            (12, "=DAYS360(DATE(2023,3,31),DATE(2023,1,30))", -60.0),
            (13, "=NETWORKDAYS(DATE(2024,1,1),DATE(2024,1,31))", 23.0),
            (14, "=NETWORKDAYS(DATE(2024,1,31),DATE(2024,1,1))", -23.0),
            (
                15,
                "=NETWORKDAYS(DATE(2024,1,1),DATE(2024,1,31),DATE(2024,1,15))",
                22.0,
            ),
            (16, "=WORKDAY(DATE(2024,1,5),1)-DATE(2024,1,8)", 0.0),
            (17, "=WORKDAY(DATE(2024,1,8),-1)-DATE(2024,1,5)", 0.0),
            (
                18,
                "=WORKDAY(DATE(2024,1,5),1,DATE(2024,1,8))-DATE(2024,1,9)",
                0.0,
            ),
            (19, "=WORKDAY(DATE(2024,1,5),0)-DATE(2024,1,5)", 0.0),
        ] {
            workbook.set_formula(cell(0, column), formula).unwrap();
            assert_close(workbook.value(cell(0, column)), expected, 1e-12, formula);
        }
        workbook.set_number(
            cell(5, 0),
            serial_date::date_serial(2024.0, 1.0, 15.0).unwrap() as f64,
        );
        workbook.set_number(
            cell(6, 0),
            serial_date::date_serial(2024.0, 1.0, 16.0).unwrap() as f64,
        );
        workbook
            .set_formula(
                cell(7, 0),
                "=NETWORKDAYS(DATE(2024,1,1),DATE(2024,1,31),A6:A7)",
            )
            .unwrap();
        assert_eq!(workbook.value(cell(7, 0)), Value::Number(21.0));
        for (formula, expected) in [
            (
                "=YEARFRAC(DATE(2024,1,1),DATE(2024,7,1),5)",
                CalcError::InvalidNumber,
            ),
            (
                "=NETWORKDAYS(DATE(2024,1,1),DATE(2024,1,31),\"x\")",
                CalcError::InvalidValue,
            ),
            ("=DAYS360(1,\"x\")", CalcError::InvalidValue),
        ] {
            workbook.set_formula(cell(8, 0), formula).unwrap();
            assert_eq!(
                workbook.value(cell(8, 0)),
                Value::Error(expected),
                "{formula}"
            );
        }
    }

    #[test]
    fn lookup_vector_and_array_forms_follow_excel() {
        let mut workbook = Workbook::default();
        for (row, (frequency, color)) in [
            (4.14, "red"),
            (4.19, "orange"),
            (5.17, "yellow"),
            (5.77, "green"),
            (6.39, "blue"),
        ]
        .into_iter()
        .enumerate()
        {
            workbook.set_number(cell(row as u32, 0), frequency);
            workbook.set_text(cell(row as u32, 1), color);
        }
        for (formula, expected) in [
            ("=LOOKUP(4.19,A1:A5,B1:B5)", Value::Text("orange".into())),
            ("=LOOKUP(5.75,A1:A5,B1:B5)", Value::Text("yellow".into())),
            ("=LOOKUP(7.66,A1:A5,B1:B5)", Value::Text("blue".into())),
            (
                "=LOOKUP(0,A1:A5,B1:B5)",
                Value::Error(CalcError::NotAvailable),
            ),
            ("=LOOKUP(5.2,A1:B5)", Value::Text("yellow".into())),
            ("=LOOKUP(5.2,A1:A5)", Value::Number(5.17)),
            (
                "=LOOKUP(1,A1:A5,B1:B4)",
                Value::Error(CalcError::InvalidArguments),
            ),
        ] {
            workbook.set_formula(cell(0, 3), formula).unwrap();
            assert_eq!(workbook.value(cell(0, 3)), expected, "{formula}");
        }
    }

    #[test]
    fn financial_and_statistical_functions_follow_excel() {
        let mut workbook = Workbook::default();
        for (row, (value, date)) in [
            (-10000.0, (2008.0, 1.0, 1.0)),
            (2750.0, (2008.0, 3.0, 1.0)),
            (4250.0, (2008.0, 10.0, 30.0)),
            (3250.0, (2009.0, 2.0, 15.0)),
            (2750.0, (2009.0, 4.0, 1.0)),
        ]
        .into_iter()
        .enumerate()
        {
            workbook.set_number(cell(row as u32, 0), value);
            workbook.set_number(
                cell(row as u32, 1),
                serial_date::date_serial(date.0, date.1, date.2).unwrap() as f64,
            );
        }
        for (row, (x, y)) in [
            (3.0, 9.0),
            (2.0, 7.0),
            (4.0, 12.0),
            (5.0, 15.0),
            (6.0, 17.0),
        ]
        .into_iter()
        .enumerate()
        {
            workbook.set_number(cell(row as u32, 2), x);
            workbook.set_number(cell(row as u32, 3), y);
        }
        workbook.set_number(cell(0, 4), 10.0);
        workbook.set_text(cell(1, 4), "text");
        workbook.set_boolean(cell(2, 4), true);
        for (formula, expected, tolerance) in [
            ("=PMT(0.08/12,10,10000)", -1037.0320893, 1e-9),
            (
                "=PMT(0.08/12,10,10000,0,1)",
                -1037.0320893 / (1.0 + 0.08 / 12.0),
                1e-9,
            ),
            ("=PMT(0,10,1000)", -100.0, 1e-12),
            ("=NPV(0.1,-10000,3000,4200,6800)", 1188.4434123, 1e-9),
            ("=NPV(0.1,A2:A5)", 10332.456799, 1e-9),
            ("=XNPV(0.09,A1:A5,B1:B5)", 2086.647602, 1e-8),
            ("=XIRR(A1:A5,B1:B5)", 0.373362535, 1e-8),
            ("=XIRR(A1:A5,B1:B5,0.5)", 0.373362535, 1e-8),
            ("=NORMDIST(42,40,1.5,TRUE)", 0.9087887802, 1e-9),
            ("=NORMDIST(42,40,1.5,FALSE)", 0.1093400498, 1e-8),
            ("=NORMDIST(0,0,1,TRUE)", 0.5, 1e-15),
            ("=NORMDIST(-6,0,1,TRUE)", 9.865876450377e-10, 1e-9),
            ("=NORMDIST(1.959963984540054,0,1,TRUE)", 0.975, 1e-12),
            ("=AVERAGEA(E1:E4)", 11.0 / 3.0, 1e-12),
            ("=AVERAGEA(10,TRUE,\"x\")", 11.0 / 3.0, 1e-12),
            ("=CORREL(C1:C5,D1:D5)", 0.997054486, 1e-9),
        ] {
            workbook.set_formula(cell(0, 6), formula).unwrap();
            assert_close(workbook.value(cell(0, 6)), expected, tolerance, formula);
        }
        for (formula, expected) in [
            ("=PMT(0.1,0,1000)", CalcError::InvalidNumber),
            ("=NORMDIST(1,0,0,TRUE)", CalcError::InvalidNumber),
            ("=XNPV(0.09,A1:A5,B1:B4)", CalcError::InvalidNumber),
            ("=XNPV(-1,A1:A5,B1:B5)", CalcError::InvalidNumber),
            ("=XIRR(A2:A5,B2:B5)", CalcError::InvalidNumber),
            ("=CORREL(C1:C5,D1:D4)", CalcError::NotAvailable),
            ("=CORREL(C1:C1,D1:D1)", CalcError::DivisionByZero),
            ("=AVERAGEA(E5:E6)", CalcError::DivisionByZero),
        ] {
            workbook.set_formula(cell(0, 6), formula).unwrap();
            assert_eq!(
                workbook.value(cell(0, 6)),
                Value::Error(expected),
                "{formula}"
            );
        }
    }

    #[test]
    fn complementary_error_function_is_accurate_across_the_join() {
        for (x, expected) in [
            (0.5, 1.0 - 0.5204998778130465),
            (1.0, 0.15729920705028513),
            (2.0, 0.004677734981047266),
            (2.5, 4.069520174449589e-4),
            (3.0, 2.209049699858544e-5),
            (4.0, 1.541725790028002e-8),
            (5.0, 1.537459794428035e-12),
            (-1.0, 1.0 + 0.8427007929497149),
        ] {
            let actual = complementary_error(x);
            assert!(
                (actual - expected).abs() <= 1e-12 * expected.abs(),
                "erfc({x}) = {actual}, expected {expected}"
            );
        }
        // The series and the continued fraction meet at 2 without a step.
        let below = complementary_error(2.0 - 1e-9);
        let above = complementary_error(2.0);
        assert!((below - above).abs() < 1e-10, "join: {below} vs {above}");
    }

    #[test]
    fn keeps_clock_dependent_date_functions_out_of_the_engine() {
        let mut workbook = Workbook::default();
        assert_eq!(
            workbook.set_formula(cell(0, 0), "=TODAY()"),
            Err(FormulaError::UnsupportedFunction("TODAY".into()))
        );
        assert_eq!(
            workbook.set_formula(cell(0, 0), "=NOW()"),
            Err(FormulaError::UnsupportedFunction("NOW".into()))
        );
    }

    #[test]
    fn rejects_unsupported_functions_and_oversized_ranges() {
        let mut workbook = Workbook::default();
        assert_eq!(
            workbook.set_formula(cell(0, 0), "=CUBEVALUE(A1:A2)"),
            Err(FormulaError::UnsupportedFunction("CUBEVALUE".into()))
        );
        assert_eq!(
            workbook.set_formula(cell(0, 0), "=SUM(A1:ZZZ999999)"),
            Err(FormulaError::RangeTooLarge)
        );
    }
}
