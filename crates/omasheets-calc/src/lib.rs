//! Owned M0 calculation experiments for OmaSheets.
//!
//! This crate deliberately starts with a narrow Excel-compatible surface. It
//! proves the dependency and incremental-recalculation semantics independently
//! of the candidate import engine; it is not yet the installed product engine.

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
    DivisionByZero,
    InvalidReference,
    InvalidValue,
    InvalidArguments,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FormulaError {
    Empty,
    UnexpectedToken(usize),
    UnsupportedFunction(String),
    InvalidReference(String),
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
    Binary(BinaryOp, Box<Expr<R>>, Box<Expr<R>>),
    Range(Vec<Expr<R>>),
    Function(Function, Vec<Expr<R>>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
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
    generation: u64,
}

impl Default for Workbook {
    fn default() -> Self {
        Self {
            indices: HashMap::new(),
            cells: Vec::new(),
            dirty_marks: Vec::new(),
            pending: Vec::new(),
            generation: 1,
        }
    }
}

impl Workbook {
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
        let parsed = Parser::new(formula, cell.sheet).parse()?;
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
            Expr::Binary(operator, left, right) => {
                let left = self.evaluate(left);
                let right = self.evaluate(right);
                apply_binary(*operator, left, right)
            }
            Expr::Range(_) => Value::Error(CalcError::InvalidArguments),
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
            Function::And => logical_fold(&values, true, |left, right| left && right),
            Function::Or => logical_fold(&values, false, |left, right| left || right),
            Function::Not if values.len() == 1 => match truthy(values[0].clone()) {
                Ok(value) => Value::Boolean(!value),
                Err(error) => Value::Error(error),
            },
            Function::Average | Function::Not | Function::If | Function::IfError => {
                Value::Error(CalcError::InvalidArguments)
            }
        }
    }

    fn flatten_values(&self, expression: &Expr<usize>, output: &mut Vec<Value>) {
        if let Expr::Range(items) = expression {
            for item in items {
                self.flatten_values(item, output);
            }
        } else {
            output.push(self.evaluate(expression));
        }
    }
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
        BinaryOp::Equal | BinaryOp::NotEqual => unreachable!("handled above"),
        BinaryOp::Less => Value::Boolean(left < right),
        BinaryOp::LessOrEqual => Value::Boolean(left <= right),
        BinaryOp::Greater => Value::Boolean(left > right),
        BinaryOp::GreaterOrEqual => Value::Boolean(left >= right),
    }
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

fn compile_expression(expression: Expr<CellId>, indices: &HashMap<CellId, usize>) -> Expr<usize> {
    match expression {
        Expr::Number(value) => Expr::Number(value),
        Expr::Boolean(value) => Expr::Boolean(value),
        Expr::Text(value) => Expr::Text(value),
        Expr::Reference(cell) => Expr::Reference(indices[&cell]),
        Expr::UnaryMinus(inner) => Expr::UnaryMinus(Box::new(compile_expression(*inner, indices))),
        Expr::Binary(operator, left, right) => Expr::Binary(
            operator,
            Box::new(compile_expression(*left, indices)),
            Box::new(compile_expression(*right, indices)),
        ),
        Expr::Range(arguments) => Expr::Range(
            arguments
                .into_iter()
                .map(|argument| compile_expression(argument, indices))
                .collect(),
        ),
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
        Expr::UnaryMinus(inner) => collect_dependencies(inner, output),
        Expr::Binary(_, left, right) => {
            collect_dependencies(left, output);
            collect_dependencies(right, output);
        }
        Expr::Range(arguments) | Expr::Function(_, arguments) => {
            for argument in arguments {
                collect_dependencies(argument, output);
            }
        }
        Expr::Number(_) | Expr::Boolean(_) | Expr::Text(_) => {}
    }
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
    sheet: u32,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str, sheet: u32) -> Self {
        let source = source.strip_prefix('=').unwrap_or(source);
        Self {
            source,
            offset: 0,
            sheet,
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
        let mut left = self.parse_additive()?;
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
            let right = self.parse_additive()?;
            left = Expr::Binary(operator, Box::new(left), Box::new(right));
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
        let mut left = self.parse_primary()?;
        loop {
            self.skip_space();
            let operator = match self.peek() {
                Some(b'*') => BinaryOp::Multiply,
                Some(b'/') => BinaryOp::Divide,
                _ => break,
            };
            self.offset += 1;
            let right = self.parse_primary()?;
            left = Expr::Binary(operator, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<Expr, FormulaError> {
        self.skip_space();
        match self.peek() {
            Some(b'-') => {
                self.offset += 1;
                Ok(Expr::UnaryMinus(Box::new(self.parse_primary()?)))
            }
            Some(b'(') => {
                self.offset += 1;
                let expression = self.parse_comparison()?;
                self.skip_space();
                self.expect(b')')?;
                Ok(expression)
            }
            Some(b'"') => self.parse_string(),
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
        let second = parse_a1(&self.source[second_start..self.offset], self.sheet)?;
        Ok(Expr::Range(expand_range(first, second)?))
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

fn expand_range(first: CellId, second: CellId) -> Result<Vec<Expr>, FormulaError> {
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
    Ok(references)
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
    fn rejects_unsupported_functions_and_oversized_ranges() {
        let mut workbook = Workbook::default();
        assert_eq!(
            workbook.set_formula(cell(0, 0), "=MEDIAN(A1:A2)"),
            Err(FormulaError::UnsupportedFunction("MEDIAN".into()))
        );
        assert_eq!(
            workbook.set_formula(cell(0, 0), "=SUM(A1:ZZZ999999)"),
            Err(FormulaError::RangeTooLarge)
        );
    }
}
