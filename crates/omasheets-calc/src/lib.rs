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
    Error(CalcError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalcError {
    DivisionByZero,
    InvalidReference,
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
    Reference(R),
    UnaryMinus(Box<Expr<R>>),
    Binary(BinaryOp, Box<Expr<R>>, Box<Expr<R>>),
    Sum(Vec<Expr<R>>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
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
            Expr::Reference(index) => self.cells[*index].value.clone(),
            Expr::UnaryMinus(inner) => match self.evaluate(inner) {
                Value::Number(value) => Value::Number(-value),
                other => other,
            },
            Expr::Binary(operator, left, right) => {
                let left = self.evaluate(left);
                let right = self.evaluate(right);
                match (left, right) {
                    (Value::Error(error), _) | (_, Value::Error(error)) => Value::Error(error),
                    (Value::Blank, Value::Blank) => apply_binary(*operator, 0.0, 0.0),
                    (Value::Blank, Value::Number(right)) => apply_binary(*operator, 0.0, right),
                    (Value::Number(left), Value::Blank) => apply_binary(*operator, left, 0.0),
                    (Value::Number(left), Value::Number(right)) => {
                        apply_binary(*operator, left, right)
                    }
                }
            }
            Expr::Sum(arguments) => {
                let mut total = 0.0;
                for argument in arguments {
                    match self.evaluate(argument) {
                        Value::Number(value) => total += value,
                        Value::Blank => {}
                        Value::Error(error) => return Value::Error(error),
                    }
                }
                Value::Number(total)
            }
        }
    }
}

fn apply_binary(operator: BinaryOp, left: f64, right: f64) -> Value {
    match operator {
        BinaryOp::Add => Value::Number(left + right),
        BinaryOp::Subtract => Value::Number(left - right),
        BinaryOp::Multiply => Value::Number(left * right),
        BinaryOp::Divide if right == 0.0 => Value::Error(CalcError::DivisionByZero),
        BinaryOp::Divide => Value::Number(left / right),
    }
}

fn compile_expression(expression: Expr<CellId>, indices: &HashMap<CellId, usize>) -> Expr<usize> {
    match expression {
        Expr::Number(value) => Expr::Number(value),
        Expr::Reference(cell) => Expr::Reference(indices[&cell]),
        Expr::UnaryMinus(inner) => Expr::UnaryMinus(Box::new(compile_expression(*inner, indices))),
        Expr::Binary(operator, left, right) => Expr::Binary(
            operator,
            Box::new(compile_expression(*left, indices)),
            Box::new(compile_expression(*right, indices)),
        ),
        Expr::Sum(arguments) => Expr::Sum(
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
        Expr::Sum(arguments) => {
            for argument in arguments {
                collect_dependencies(argument, output);
            }
        }
        Expr::Number(_) => {}
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
        let expression = self.parse_additive()?;
        self.skip_space();
        if self.offset != self.source.len() {
            return Err(FormulaError::UnexpectedToken(self.offset));
        }
        Ok(expression)
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
                let expression = self.parse_additive()?;
                self.skip_space();
                self.expect(b')')?;
                Ok(expression)
            }
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
        self.source[start..self.offset]
            .parse::<f64>()
            .map(Expr::Number)
            .map_err(|_| FormulaError::UnexpectedToken(start))
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
        Ok(Expr::Sum(expand_range(first, second)?))
    }

    fn parse_function(&mut self, name: &str) -> Result<Expr, FormulaError> {
        if !name.eq_ignore_ascii_case("SUM") {
            return Err(FormulaError::UnsupportedFunction(name.to_ascii_uppercase()));
        }
        self.expect(b'(')?;
        let mut arguments = Vec::new();
        loop {
            self.skip_space();
            if self.peek() == Some(b')') {
                self.offset += 1;
                break;
            }
            arguments.push(self.parse_additive()?);
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
        Ok(Expr::Sum(arguments))
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
    fn rejects_unsupported_functions_and_oversized_ranges() {
        let mut workbook = Workbook::default();
        assert_eq!(
            workbook.set_formula(cell(0, 0), "=AVERAGE(A1:A2)"),
            Err(FormulaError::UnsupportedFunction("AVERAGE".into()))
        );
        assert_eq!(
            workbook.set_formula(cell(0, 0), "=SUM(A1:ZZZ999999)"),
            Err(FormulaError::RangeTooLarge)
        );
    }
}
