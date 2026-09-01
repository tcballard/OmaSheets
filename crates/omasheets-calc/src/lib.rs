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
enum Expr {
    Number(f64),
    Reference(CellId),
    UnaryMinus(Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    Sum(Vec<Expr>),
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
    Formula(Expr),
}

#[derive(Clone, Debug)]
struct Cell {
    input: Input,
    dependencies: BTreeSet<CellId>,
    value: Value,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecalcReport {
    /// Changed cell plus downstream formula cells evaluated in this pass.
    pub evaluated: Vec<CellId>,
}

#[derive(Default)]
pub struct Workbook {
    cells: HashMap<CellId, Cell>,
    dependents: HashMap<CellId, BTreeSet<CellId>>,
}

impl Workbook {
    pub fn value(&self, cell: CellId) -> Value {
        self.cells
            .get(&cell)
            .map(|entry| entry.value.clone())
            .unwrap_or(Value::Blank)
    }

    pub fn set_number(&mut self, cell: CellId, value: f64) -> RecalcReport {
        self.commit(cell, Input::Literal(Value::Number(value)), BTreeSet::new())
    }

    pub fn clear(&mut self, cell: CellId) -> RecalcReport {
        self.commit(cell, Input::Literal(Value::Blank), BTreeSet::new())
    }

    pub fn set_formula(
        &mut self,
        cell: CellId,
        formula: &str,
    ) -> Result<RecalcReport, FormulaError> {
        let expression = Parser::new(formula, cell.sheet).parse()?;
        let mut dependencies = BTreeSet::new();
        collect_dependencies(&expression, &mut dependencies);
        if let Some(path) = self.prospective_cycle(cell, &dependencies) {
            return Err(FormulaError::Cycle(path));
        }
        Ok(self.commit(cell, Input::Formula(expression), dependencies))
    }

    fn commit(
        &mut self,
        cell: CellId,
        input: Input,
        dependencies: BTreeSet<CellId>,
    ) -> RecalcReport {
        if let Some(previous) = self.cells.get(&cell) {
            for dependency in &previous.dependencies {
                if let Some(users) = self.dependents.get_mut(dependency) {
                    users.remove(&cell);
                }
            }
        }
        for dependency in &dependencies {
            self.dependents.entry(*dependency).or_default().insert(cell);
        }
        self.cells.insert(
            cell,
            Cell {
                input,
                dependencies,
                value: Value::Blank,
            },
        );
        self.recalculate_from(cell)
    }

    fn prospective_cycle(
        &self,
        changed: CellId,
        dependencies: &BTreeSet<CellId>,
    ) -> Option<Vec<CellId>> {
        for dependency in dependencies {
            let mut path = vec![changed];
            let mut visited = HashSet::new();
            if self.find_dependency_path(
                *dependency,
                changed,
                changed,
                dependencies,
                &mut visited,
                &mut path,
            ) {
                path.push(changed);
                return Some(path);
            }
        }
        None
    }

    fn find_dependency_path(
        &self,
        current: CellId,
        target: CellId,
        changed: CellId,
        changed_dependencies: &BTreeSet<CellId>,
        visited: &mut HashSet<CellId>,
        path: &mut Vec<CellId>,
    ) -> bool {
        path.push(current);
        if current == target {
            path.pop();
            return true;
        }
        if !visited.insert(current) {
            path.pop();
            return false;
        }
        let dependencies = if current == changed {
            Some(changed_dependencies)
        } else {
            self.cells.get(&current).map(|cell| &cell.dependencies)
        };
        if let Some(dependencies) = dependencies {
            for dependency in dependencies {
                if self.find_dependency_path(
                    *dependency,
                    target,
                    changed,
                    changed_dependencies,
                    visited,
                    path,
                ) {
                    return true;
                }
            }
        }
        path.pop();
        false
    }

    fn recalculate_from(&mut self, changed: CellId) -> RecalcReport {
        let mut dirty = BTreeSet::from([changed]);
        let mut queue = VecDeque::from([changed]);
        while let Some(cell) = queue.pop_front() {
            if let Some(users) = self.dependents.get(&cell) {
                for user in users {
                    if dirty.insert(*user) {
                        queue.push_back(*user);
                    }
                }
            }
        }

        let mut pending: HashMap<CellId, usize> = dirty
            .iter()
            .map(|cell| {
                let count = self
                    .cells
                    .get(cell)
                    .map(|entry| {
                        entry
                            .dependencies
                            .iter()
                            .filter(|item| dirty.contains(item))
                            .count()
                    })
                    .unwrap_or_default();
                (*cell, count)
            })
            .collect();
        let mut ready: BTreeSet<CellId> = pending
            .iter()
            .filter_map(|(cell, count)| (*count == 0).then_some(*cell))
            .collect();
        let mut evaluated = Vec::with_capacity(dirty.len());

        while let Some(cell) = ready.pop_first() {
            let input = self.cells.get(&cell).map(|entry| entry.input.clone());
            if let Some(input) = input {
                let value = match input {
                    Input::Literal(value) => value,
                    Input::Formula(expression) => self.evaluate(&expression),
                };
                if let Some(entry) = self.cells.get_mut(&cell) {
                    entry.value = value;
                }
            }
            evaluated.push(cell);
            if let Some(users) = self.dependents.get(&cell) {
                for user in users.iter().filter(|user| dirty.contains(user)) {
                    let remaining = pending.get_mut(user).expect("dirty cell has an indegree");
                    *remaining -= 1;
                    if *remaining == 0 {
                        ready.insert(*user);
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

    fn evaluate(&self, expression: &Expr) -> Value {
        match expression {
            Expr::Number(value) => Value::Number(*value),
            Expr::Reference(cell) => self.value(*cell),
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
        assert!(matches!(error, FormulaError::Cycle(_)));
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
