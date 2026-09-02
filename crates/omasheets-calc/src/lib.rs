//! Owned M0 calculation experiments for OmaSheets.
//!
//! This crate deliberately starts with a narrow Excel-compatible surface. It
//! proves the dependency and incremental-recalculation semantics independently
//! of the candidate import engine; it is not yet the installed product engine.
//!
//! Dates are Excel 1900-system serial numbers; see [`serial_date`] for the
//! boundary rules and the deliberately unsupported cases.

pub mod serial_date;

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;

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
    /// Excel `#REF!` and `#N/A`.
    InvalidReference,
    /// Excel `#VALUE!`.
    InvalidValue,
    /// Excel `#NUM!`: a numeric argument outside the function's domain.
    InvalidNumber,
    /// Wrong argument count or shape for the function.
    InvalidArguments,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FormulaError {
    Empty,
    UnexpectedToken(usize),
    UnsupportedFunction(String),
    InvalidReference(String),
    UnknownSheet(String),
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
    Reference(R),
    UnaryMinus(Box<Expr<R>>),
    Percent(Box<Expr<R>>),
    Binary(BinaryOp, Box<Expr<R>>, Box<Expr<R>>),
    Range {
        items: Vec<Expr<R>>,
        rows: usize,
        columns: usize,
    },
    Function(Function, Vec<Expr<R>>),
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
    IsBlank,
    IsNumber,
    IsText,
    IsLogical,
    IsError,
    N,
    T,
    SumProduct,
    Median,
}

#[derive(Clone, Debug)]
enum Input {
    Literal(Value),
    Formula(Expr<usize>),
}

#[derive(Clone, Debug)]
struct Cell {
    id: CellId,
    input: Input,
    dependencies: Vec<usize>,
    dependents: Vec<usize>,
    value: Value,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecalcReport {
    /// Changed cell plus downstream formula cells evaluated in this pass.
    pub evaluated: Vec<CellId>,
}

pub struct Workbook {
    indices: HashMap<CellId, usize>,
    cells: Vec<Cell>,
    dirty_marks: Vec<u64>,
    pending: Vec<usize>,
    sheet_names: HashMap<String, u32>,
    generation: u64,
}

impl Default for Workbook {
    fn default() -> Self {
        Self {
            indices: HashMap::new(),
            cells: Vec::new(),
            dirty_marks: Vec::new(),
            pending: Vec::new(),
            sheet_names: HashMap::new(),
            generation: 1,
        }
    }
}

impl Workbook {
    pub fn define_sheet(&mut self, index: u32, name: impl Into<String>) {
        self.sheet_names.insert(name.into().to_lowercase(), index);
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
        let parsed = Parser::new(formula, cell.sheet, &self.sheet_names).parse()?;
        let mut dependencies = BTreeSet::new();
        collect_dependencies(&parsed, &mut dependencies);
        if let Some(path) = self.prospective_cycle(cell, &dependencies) {
            return Err(FormulaError::Cycle(path));
        }
        let dependencies: Vec<usize> = dependencies
            .into_iter()
            .map(|dependency| self.ensure_cell(dependency))
            .collect();
        let expression = compile_expression(parsed, &self.indices);
        Ok(self.commit(cell, Input::Formula(expression), dependencies))
    }

    fn ensure_cell(&mut self, cell: CellId) -> usize {
        if let Some(index) = self.indices.get(&cell) {
            return *index;
        }
        let index = self.cells.len();
        self.indices.insert(cell, index);
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
        self.recalculate_from(changed)
    }

    fn prospective_cycle(
        &self,
        changed: CellId,
        dependencies: &BTreeSet<CellId>,
    ) -> Option<Vec<CellId>> {
        if dependencies.contains(&changed) {
            return Some(vec![changed, changed]);
        }
        let changed_index = *self.indices.get(&changed)?;
        let targets: HashSet<usize> = dependencies
            .iter()
            .filter_map(|dependency| self.indices.get(dependency).copied())
            .collect();
        if targets.is_empty() {
            return None;
        }
        let mut parents = vec![usize::MAX; self.cells.len()];
        parents[changed_index] = changed_index;
        let mut queue = VecDeque::from([changed_index]);
        while let Some(current) = queue.pop_front() {
            if targets.contains(&current) {
                let mut path = vec![current];
                let mut cursor = current;
                while cursor != changed_index {
                    cursor = parents[cursor];
                    path.push(cursor);
                }
                path.reverse();
                let mut path: Vec<CellId> =
                    path.into_iter().map(|index| self.cells[index].id).collect();
                path.push(changed);
                return Some(path);
            }
            for dependent in &self.cells[current].dependents {
                if parents[*dependent] == usize::MAX {
                    parents[*dependent] = current;
                    queue.push_back(*dependent);
                }
            }
        }
        None
    }

    fn recalculate_from(&mut self, changed: usize) -> RecalcReport {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.dirty_marks.fill(0);
            self.generation = 1;
        }
        let generation = self.generation;
        self.dirty_marks[changed] = generation;
        let mut dirty = vec![changed];
        let mut cursor = 0;
        while let Some(index) = dirty.get(cursor).copied() {
            cursor += 1;
            for dependent in self.cells[index].dependents.iter().copied() {
                if self.dirty_marks[dependent] != generation {
                    self.dirty_marks[dependent] = generation;
                    dirty.push(dependent);
                }
            }
        }

        for index in &dirty {
            self.pending[*index] = self.cells[*index]
                .dependencies
                .iter()
                .filter(|dependency| self.dirty_marks[**dependency] == generation)
                .count();
        }
        let mut ready: VecDeque<usize> = dirty
            .iter()
            .copied()
            .filter(|index| self.pending[*index] == 0)
            .collect();
        let mut evaluated = Vec::with_capacity(dirty.len());

        while let Some(cell_index) = ready.pop_front() {
            let value = match &self.cells[cell_index].input {
                Input::Literal(value) => value.clone(),
                Input::Formula(expression) => self.evaluate(expression),
            };
            self.cells[cell_index].value = value;
            evaluated.push(self.cells[cell_index].id);
            for dependent in self.cells[cell_index].dependents.iter().copied() {
                if self.dirty_marks[dependent] == generation {
                    let remaining = &mut self.pending[dependent];
                    *remaining -= 1;
                    if *remaining == 0 {
                        ready.push_back(dependent);
                    }
                }
            }
        }
        debug_assert_eq!(
            evaluated.len(),
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
            Expr::Range { .. } => Value::Error(CalcError::InvalidArguments),
            Expr::Function(function, arguments) => self.evaluate_function(*function, arguments),
        }
    }

    fn evaluate_function(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        if function == Function::If {
            if arguments.len() != 3 {
                return Value::Error(CalcError::InvalidArguments);
            }
            return match truthy(self.evaluate(&arguments[0])) {
                Ok(true) => self.evaluate(&arguments[1]),
                Ok(false) => self.evaluate(&arguments[2]),
                Err(error) => Value::Error(error),
            };
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
            Function::Index | Function::Match | Function::VLookup | Function::XLookup
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
                | Function::N
                | Function::T
        ) {
            return self.evaluate_inspection_function(function, arguments);
        }
        if function == Function::SumProduct {
            return self.evaluate_sumproduct(arguments);
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
            Function::Average
            | Function::Not
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
            | Function::Median => Value::Error(CalcError::InvalidArguments),
        }
    }

    /// Type inspection takes exactly one scalar argument and never propagates
    /// an error from it: `ISERROR(1/0)` is `TRUE`, `ISBLANK(1/0)` is `FALSE`.
    /// `N` and `T` do propagate errors, matching Excel.
    fn evaluate_inspection_function(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        if arguments.len() != 1 || matches!(arguments[0], Expr::Range { .. }) {
            return Value::Error(CalcError::InvalidArguments);
        }
        let value = self.evaluate(&arguments[0]);
        match function {
            Function::IsBlank => Value::Boolean(matches!(value, Value::Blank)),
            Function::IsNumber => Value::Boolean(matches!(value, Value::Number(_))),
            Function::IsText => Value::Boolean(matches!(value, Value::Text(_))),
            Function::IsLogical => Value::Boolean(matches!(value, Value::Boolean(_))),
            Function::IsError => Value::Boolean(matches!(value, Value::Error(_))),
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
            Expr::Range { rows, columns, .. } => (*rows, *columns),
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

    fn flatten_values(&self, expression: &Expr<usize>, output: &mut Vec<Value>) {
        if let Expr::Range { items, .. } = expression {
            for item in items {
                self.flatten_values(item, output);
            }
        } else {
            output.push(self.evaluate(expression));
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
                let Some((items, rows, columns)) = range_parts(&arguments[0]) else {
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
                } else if *columns == 1 {
                    1
                } else if *rows == 1 {
                    return items
                        .get(row - 1)
                        .map(|item| self.evaluate(item))
                        .unwrap_or(Value::Error(CalcError::InvalidReference));
                } else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                if row > *rows || column > *columns {
                    return Value::Error(CalcError::InvalidReference);
                }
                self.evaluate(&items[(row - 1) * *columns + column - 1])
            }
            Function::Match if matches!(arguments.len(), 2 | 3) => {
                if arguments.len() == 3 {
                    match number(self.evaluate(&arguments[2])) {
                        Ok(0.0) => {}
                        Ok(_) => return Value::Error(CalcError::InvalidArguments),
                        Err(error) => return Value::Error(error),
                    }
                }
                let lookup = self.evaluate(&arguments[0]);
                if matches!(lookup, Value::Error(_)) {
                    return lookup;
                }
                let Some((items, rows, columns)) = range_parts(&arguments[1]) else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                if *rows != 1 && *columns != 1 {
                    return Value::Error(CalcError::InvalidArguments);
                }
                for (index, item) in items.iter().enumerate() {
                    let candidate = self.evaluate(item);
                    if matches!(candidate, Value::Error(_)) {
                        return candidate;
                    }
                    if lookup_equal(&lookup, &candidate) {
                        return Value::Number((index + 1) as f64);
                    }
                }
                Value::Error(CalcError::InvalidReference)
            }
            Function::VLookup if arguments.len() == 4 => {
                if !matches!(self.evaluate(&arguments[3]), Value::Boolean(false)) {
                    return Value::Error(CalcError::InvalidArguments);
                }
                let lookup = self.evaluate(&arguments[0]);
                if matches!(lookup, Value::Error(_)) {
                    return lookup;
                }
                let Some((items, rows, columns)) = range_parts(&arguments[1]) else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                let Ok(column) = positive_index(self.evaluate(&arguments[2])) else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                if column > *columns {
                    return Value::Error(CalcError::InvalidReference);
                }
                for row in 0..*rows {
                    let candidate = self.evaluate(&items[row * *columns]);
                    if matches!(candidate, Value::Error(_)) {
                        return candidate;
                    }
                    if lookup_equal(&lookup, &candidate) {
                        return self.evaluate(&items[row * *columns + column - 1]);
                    }
                }
                Value::Error(CalcError::InvalidReference)
            }
            Function::XLookup if matches!(arguments.len(), 3 | 4) => {
                let lookup = self.evaluate(&arguments[0]);
                if matches!(lookup, Value::Error(_)) {
                    return lookup;
                }
                let Some((lookup_items, lookup_rows, lookup_columns)) = range_parts(&arguments[1])
                else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                let Some((return_items, return_rows, return_columns)) = range_parts(&arguments[2])
                else {
                    return Value::Error(CalcError::InvalidArguments);
                };
                if (*lookup_rows != 1 && *lookup_columns != 1)
                    || (*return_rows != 1 && *return_columns != 1)
                    || lookup_items.len() != return_items.len()
                {
                    return Value::Error(CalcError::InvalidArguments);
                }
                for (index, item) in lookup_items.iter().enumerate() {
                    let candidate = self.evaluate(item);
                    if matches!(candidate, Value::Error(_)) {
                        return candidate;
                    }
                    if lookup_equal(&lookup, &candidate) {
                        return self.evaluate(&return_items[index]);
                    }
                }
                if arguments.len() == 4 {
                    return self.evaluate(&arguments[3]);
                }
                Value::Error(CalcError::InvalidReference)
            }
            _ => Value::Error(CalcError::InvalidArguments),
        }
    }
}

fn range_parts<R>(expression: &Expr<R>) -> Option<(&[Expr<R>], &usize, &usize)> {
    match expression {
        Expr::Range {
            items,
            rows,
            columns,
        } => Some((items, rows, columns)),
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
    if matches!(operator, BinaryOp::Equal | BinaryOp::NotEqual) {
        let numerically_equal = match (number(left.clone()), number(right.clone())) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        };
        let equal = left == right || numerically_equal;
        return Value::Boolean(if operator == BinaryOp::Equal {
            equal
        } else {
            !equal
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
        BinaryOp::Equal | BinaryOp::NotEqual | BinaryOp::Concat => {
            unreachable!("handled above")
        }
        BinaryOp::Less => Value::Boolean(left < right),
        BinaryOp::LessOrEqual => Value::Boolean(left <= right),
        BinaryOp::Greater => Value::Boolean(left > right),
        BinaryOp::GreaterOrEqual => Value::Boolean(left >= right),
    }
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

fn compile_expression(expression: Expr<CellId>, indices: &HashMap<CellId, usize>) -> Expr<usize> {
    match expression {
        Expr::Number(value) => Expr::Number(value),
        Expr::Boolean(value) => Expr::Boolean(value),
        Expr::Text(value) => Expr::Text(value),
        Expr::Reference(cell) => Expr::Reference(indices[&cell]),
        Expr::UnaryMinus(inner) => Expr::UnaryMinus(Box::new(compile_expression(*inner, indices))),
        Expr::Percent(inner) => Expr::Percent(Box::new(compile_expression(*inner, indices))),
        Expr::Binary(operator, left, right) => Expr::Binary(
            operator,
            Box::new(compile_expression(*left, indices)),
            Box::new(compile_expression(*right, indices)),
        ),
        Expr::Range {
            items,
            rows,
            columns,
        } => Expr::Range {
            items: items
                .into_iter()
                .map(|argument| compile_expression(argument, indices))
                .collect(),
            rows,
            columns,
        },
        Expr::Function(function, arguments) => Expr::Function(
            function,
            arguments
                .into_iter()
                .map(|argument| compile_expression(argument, indices))
                .collect(),
        ),
    }
}

fn collect_dependencies(expression: &Expr, output: &mut BTreeSet<CellId>) {
    match expression {
        Expr::Reference(cell) => {
            output.insert(*cell);
        }
        Expr::UnaryMinus(inner) | Expr::Percent(inner) => collect_dependencies(inner, output),
        Expr::Binary(_, left, right) => {
            collect_dependencies(left, output);
            collect_dependencies(right, output);
        }
        Expr::Range {
            items: arguments, ..
        }
        | Expr::Function(_, arguments) => {
            for argument in arguments {
                collect_dependencies(argument, output);
            }
        }
        Expr::Number(_) | Expr::Boolean(_) | Expr::Text(_) => {}
    }
}

struct Parser<'source, 'sheets> {
    source: &'source str,
    offset: usize,
    sheet: u32,
    sheet_names: &'sheets HashMap<String, u32>,
}

impl<'source, 'sheets> Parser<'source, 'sheets> {
    fn new(source: &'source str, sheet: u32, sheet_names: &'sheets HashMap<String, u32>) -> Self {
        let source = source.strip_prefix('=').unwrap_or(source);
        Self {
            source,
            offset: 0,
            sheet,
            sheet_names,
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
            Some(byte) if byte.is_ascii_digit() || byte == b'.' => self.parse_number(),
            Some(byte) if byte.is_ascii_alphabetic() || byte == b'$' => {
                self.parse_reference_or_function()
            }
            _ => Err(FormulaError::UnexpectedToken(self.offset)),
        }
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
        while matches!(self.peek(), Some(byte) if byte.is_ascii_alphanumeric() || byte == b'$' || byte == b'_')
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
        let first = parse_a1(token, self.sheet)?;
        self.parse_range_tail(first, self.sheet)
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
        loop {
            self.skip_space();
            if self.peek() == Some(b')') {
                self.offset += 1;
                break;
            }
            arguments.push(self.parse_comparison()?);
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

fn parse_function_name(name: &str) -> Result<Function, FormulaError> {
    match name.to_ascii_uppercase().as_str() {
        "SUM" => Ok(Function::Sum),
        "AVERAGE" => Ok(Function::Average),
        "MIN" => Ok(Function::Min),
        "MAX" => Ok(Function::Max),
        "COUNT" => Ok(Function::Count),
        "COUNTA" => Ok(Function::CountA),
        "PRODUCT" => Ok(Function::Product),
        "ABS" => Ok(Function::Abs),
        "ROUND" => Ok(Function::Round),
        "ROUNDUP" => Ok(Function::RoundUp),
        "ROUNDDOWN" => Ok(Function::RoundDown),
        "INT" => Ok(Function::Int),
        "MOD" => Ok(Function::Mod),
        "POWER" => Ok(Function::Power),
        "SQRT" => Ok(Function::Sqrt),
        "IF" => Ok(Function::If),
        "AND" => Ok(Function::And),
        "OR" => Ok(Function::Or),
        "NOT" => Ok(Function::Not),
        "IFERROR" => Ok(Function::IfError),
        "SIGN" => Ok(Function::Sign),
        "CEILING" => Ok(Function::Ceiling),
        "FLOOR" => Ok(Function::Floor),
        "TRUNC" => Ok(Function::Trunc),
        "EXP" => Ok(Function::Exp),
        "LN" => Ok(Function::Ln),
        "LOG" => Ok(Function::Log),
        "LOG10" => Ok(Function::Log10),
        "PI" => Ok(Function::Pi),
        "LEN" => Ok(Function::Len),
        "LEFT" => Ok(Function::Left),
        "RIGHT" => Ok(Function::Right),
        "MID" => Ok(Function::Mid),
        "TRIM" => Ok(Function::Trim),
        "UPPER" => Ok(Function::Upper),
        "LOWER" => Ok(Function::Lower),
        "CONCAT" | "CONCATENATE" => Ok(Function::Concat),
        "VALUE" => Ok(Function::Value),
        "EXACT" => Ok(Function::Exact),
        "COUNTIF" => Ok(Function::CountIf),
        "SUMIF" => Ok(Function::SumIf),
        "COUNTIFS" => Ok(Function::CountIfs),
        "SUMIFS" => Ok(Function::SumIfs),
        "AVERAGEIF" => Ok(Function::AverageIf),
        "AVERAGEIFS" => Ok(Function::AverageIfs),
        "INDEX" => Ok(Function::Index),
        "MATCH" => Ok(Function::Match),
        "VLOOKUP" => Ok(Function::VLookup),
        "XLOOKUP" => Ok(Function::XLookup),
        "DATE" => Ok(Function::Date),
        "YEAR" => Ok(Function::Year),
        "MONTH" => Ok(Function::Month),
        "DAY" => Ok(Function::Day),
        "EDATE" => Ok(Function::EDate),
        "EOMONTH" => Ok(Function::EoMonth),
        "WEEKDAY" => Ok(Function::Weekday),
        "ISBLANK" => Ok(Function::IsBlank),
        "ISNUMBER" => Ok(Function::IsNumber),
        "ISTEXT" => Ok(Function::IsText),
        "ISLOGICAL" => Ok(Function::IsLogical),
        "ISERROR" => Ok(Function::IsError),
        "N" => Ok(Function::N),
        "T" => Ok(Function::T),
        "SUMPRODUCT" => Ok(Function::SumProduct),
        "MEDIAN" => Ok(Function::Median),
        _ => Err(FormulaError::UnsupportedFunction(name.to_ascii_uppercase())),
    }
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
    let mut references = Vec::with_capacity(count);
    for row in first_row..=last_row {
        for column in first_column..=last_column {
            references.push(Expr::Reference(CellId::new(first.sheet, row, column)));
        }
    }
    Ok(Expr::Range {
        items: references,
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
        assert_eq!(
            workbook.value(cell(1, 2)),
            Value::Error(CalcError::InvalidArguments)
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
            (
                21,
                "=YEAR(A1:A2)",
                Value::Error(CalcError::InvalidArguments),
            ),
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
