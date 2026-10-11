//! Aggregate argument provenance, sparse traversal and numeric accumulation.
use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum ArgumentKind {
    Scalar,
    Reference,
    Array,
    ErrorLiteral,
}

impl Workbook {
    pub(super) fn evaluate_simple_aggregate(
        &self,
        function: Function,
        arguments: &[Expr<usize>],
        skip_subtotals: bool,
    ) -> Value {
        let mut count = 0usize;
        let mut nonblank = 0usize;
        let mut accumulator: f64 = if function == Function::Product {
            1.0
        } else {
            0.0
        };
        let mut sum = CompensatedSum::default();
        let mut error = None;
        let mut logical_format = None;
        let mut visit = |value: &Value, kind| {
            if !matches!(value, Value::Blank) {
                nonblank += 1;
            }
            let numeric = match value {
                Value::Number(n) => Some(*n),
                Value::Boolean(b) => Some(f64::from(*b)),
                Value::Text(_)
                    if matches!(
                        function,
                        Function::AverageA | Function::MinA | Function::MaxA
                    ) =>
                {
                    Some(0.0)
                }
                Value::Text(text) if kind == ArgumentKind::Scalar => match function {
                    Function::Count => text.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
                    Function::CountA => None,
                    _ => {
                        error.get_or_insert(
                            if matches!(
                                function,
                                Function::Min | Function::Max | Function::MinA | Function::MaxA
                            ) {
                                CalcError::IllegalParameter
                            } else {
                                CalcError::InvalidValue
                            },
                        );
                        None
                    }
                },
                Value::Error(e) => {
                    if !matches!(function, Function::Count | Function::CountA) {
                        error.get_or_insert_with(|| e.clone());
                    }
                    None
                }
                _ => None,
            };
            if let Some(n) = numeric {
                logical_format.get_or_insert(
                    kind == ArgumentKind::Reference && matches!(value, Value::Boolean(_)),
                );
                match function {
                    Function::Sum | Function::Average | Function::AverageA => sum.add(n),
                    Function::SumSq => sum.add(n * n),
                    Function::Product => accumulator *= n,
                    Function::Min | Function::MinA if count != 0 => {
                        accumulator = accumulator.min(n)
                    }
                    Function::Max | Function::MaxA if count != 0 => {
                        accumulator = accumulator.max(n)
                    }
                    Function::Min | Function::Max | Function::MinA | Function::MaxA => {
                        accumulator = n
                    }
                    _ => {}
                }
                count += 1;
            }
        };
        if matches!(
            function,
            Function::Sum | Function::SumSq | Function::Average | Function::AverageA
        ) {
            // Calc consumes scalar arguments from its stack in reverse order;
            // each range/array retains its own traversal order.
            for argument in arguments.iter().rev() {
                self.visit_aggregate_arguments(
                    std::slice::from_ref(argument),
                    skip_subtotals,
                    &mut visit,
                );
            }
        } else if matches!(function, Function::MinA | Function::MaxA) {
            self.visit_statistical_arguments(arguments, true, false, &mut visit);
        } else {
            self.visit_aggregate_arguments(arguments, skip_subtotals, &mut visit);
        }
        if let Some(error) = error {
            return Value::Error(error);
        }
        match function {
            Function::Sum | Function::SumSq => number_value(sum.total()),
            Function::Average | Function::AverageA if count == 0 => {
                Value::Error(CalcError::DivisionByZero)
            }
            Function::Average | Function::AverageA => number_value(sum.total() / count as f64),
            Function::Count => Value::Number(count as f64),
            Function::CountA => Value::Number(nonblank as f64),
            Function::Product if count == 0 => Value::Number(0.0),
            Function::Min | Function::Max | Function::MinA | Function::MaxA
                if logical_format == Some(true) && (accumulator == 0.0 || accumulator == 1.0) =>
            {
                Value::Boolean(accumulator != 0.0)
            }
            Function::Min | Function::Max | Function::MinA | Function::MaxA | Function::Product => {
                number_value(accumulator)
            }
            _ => unreachable!("simple aggregate only"),
        }
    }

    pub(super) fn evaluate_integer_aggregate(
        &self,
        function: Function,
        arguments: &[Expr<usize>],
    ) -> Value {
        if arguments.is_empty() {
            return Value::Error(CalcError::InvalidArguments);
        }
        let mut values = Vec::new();
        for argument in arguments {
            if contains_array_operand(argument) && !matches!(argument, Expr::RangeNode { .. }) {
                match self.evaluate_array(argument) {
                    Ok(array) if array.empty_paths.iter().any(|empty| *empty) => {
                        return Value::Error(CalcError::IllegalArgument);
                    }
                    Ok(array) => {
                        let mut matrix_values = Vec::with_capacity(array.values.len());
                        for column in 0..array.columns {
                            for row in 0..array.rows {
                                matrix_values.push(match array.at(row, column) {
                                    Value::Text(_) | Value::Blank => {
                                        Value::Error(CalcError::IllegalArgument)
                                    }
                                    value => value.clone(),
                                });
                            }
                        }
                        values.push(super::common::integer_aggregate(
                            function,
                            &matrix_values,
                            true,
                        ));
                    }
                    Err(error) => return Value::Error(error),
                }
            } else {
                self.visit_aggregate_arguments(
                    std::slice::from_ref(argument),
                    false,
                    &mut |value, kind| {
                        values.push(match value {
                            Value::Blank if matches!(argument, Expr::Reference(_)) => {
                                Value::Number(0.0)
                            }
                            Value::Text(text)
                                if kind == ArgumentKind::Scalar
                                    || matches!(argument, Expr::Reference(_)) =>
                            {
                                match text.trim().parse::<f64>() {
                                    Ok(n) if n.is_finite() => Value::Number(n),
                                    _ => Value::Error(CalcError::InvalidValue),
                                }
                            }
                            value => value.clone(),
                        });
                    },
                );
            }
        }
        super::common::integer_aggregate(function, &values, false)
    }

    pub(super) fn evaluate_statistical_aggregate(
        &self,
        function: Function,
        arguments: &[Expr<usize>],
        skip_subtotals: bool,
    ) -> Value {
        if arguments.is_empty() {
            return Value::Error(CalcError::InvalidArguments);
        }
        let mut numbers = Vec::new();
        let mut error = None;
        self.visit_aggregate_arguments(arguments, skip_subtotals, &mut |value, kind| match value {
            Value::Number(n) => numbers.push(*n),
            Value::Boolean(b) => numbers.push(f64::from(*b)),
            Value::Error(e) => {
                error.get_or_insert_with(|| e.clone());
            }
            Value::Text(_) if kind == ArgumentKind::Scalar => {
                error.get_or_insert(CalcError::IllegalParameter);
            }
            _ => {}
        });
        if let Some(error) = error {
            return Value::Error(error);
        }
        match function {
            Function::Median if numbers.is_empty() => Value::Error(CalcError::InvalidValue),
            Function::Median => median(numbers),
            Function::StDev => deviation(&numbers, true, true),
            Function::StDevP => deviation(&numbers, false, true),
            Function::Var => deviation(&numbers, true, false),
            Function::VarP => deviation(&numbers, false, false),
            _ => unreachable!("statistical aggregate only"),
        }
    }

    /// Keep scalar text distinct from text stored in cells or arrays. Empty IF
    /// paths are absent observations; explicit FALSE is a numeric observation.
    pub(super) fn visit_aggregate_arguments(
        &self,
        arguments: &[Expr<usize>],
        skip_subtotals: bool,
        visit: &mut impl FnMut(&Value, ArgumentKind),
    ) {
        for argument in arguments {
            match argument {
                Expr::RangeNode { node, .. } => match self.range_shape(*node) {
                    RangeShape::Rectangle {
                        anchor,
                        rows,
                        columns,
                    } => {
                        self.for_each_rectangle_cell(anchor, rows, columns, |_, index| {
                            if !skip_subtotals || !self.cell_is_subtotal(index) {
                                visit(&self.cells[index].value, ArgumentKind::Reference);
                            }
                        });
                    }
                    RangeShape::Members { .. } => {
                        for index in &self.cells[*node].dependencies {
                            if !skip_subtotals || !self.cell_is_subtotal(*index) {
                                visit(&self.cells[*index].value, ArgumentKind::Reference);
                            }
                        }
                    }
                },
                Expr::Function(Function::ReferenceSpan | Function::Index, _) => {
                    match self.reference_view(argument) {
                        Ok(reference) => reference.visit(self, &mut |value: &Value| {
                            visit(value, ArgumentKind::Reference)
                        }),
                        Err(CalcError::InvalidArguments) => match self.evaluate_array(argument) {
                            Ok(array) => array
                                .aggregate_values()
                                .for_each(|value| visit(value, ArgumentKind::Array)),
                            Err(error) => visit(&Value::Error(error), ArgumentKind::Array),
                        },
                        Err(error) => visit(&Value::Error(error), ArgumentKind::Array),
                    }
                }
                Expr::Empty => {}
                other if contains_array_operand(other) => match self.evaluate_array(other) {
                    Ok(array) => array
                        .aggregate_values()
                        .for_each(|value| visit(value, ArgumentKind::Array)),
                    Err(error) => visit(&Value::Error(error), ArgumentKind::Array),
                },
                Expr::Reference(_) => visit(&self.evaluate(argument), ArgumentKind::Reference),
                Expr::Error(error) => {
                    visit(&Value::Error(error.clone()), ArgumentKind::ErrorLiteral)
                }
                other => visit(&self.evaluate(other), ArgumentKind::Scalar),
            }
        }
    }
}

/// Kahan accumulation, preserving Calc's measured cancellation/order behaviour.
/// Never turn overflow into a finite answer.
#[derive(Default)]
pub(super) struct CompensatedSum {
    sum: f64,
    correction: f64,
}
impl CompensatedSum {
    pub(super) fn add(&mut self, value: f64) {
        let corrected = value - self.correction;
        let next = self.sum + corrected;
        self.correction = if next.is_finite() {
            (next - self.sum) - corrected
        } else {
            0.0
        };
        self.sum = next;
    }
    pub(super) fn total(&self) -> f64 {
        self.sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_dependencies_recalculate_with_aggregate_argument_provenance() {
        let mut book = Workbook::default();
        let input = CellId::new(0, 0, 0);
        book.set_formula(input, "=TRUE()").unwrap();
        book.set_formula(CellId::new(0, 1, 0), "=FALSE()").unwrap();
        for (row, formula, before, after) in [
            (0, "=SUM(A1:A2,2)", 3.0, 2.0),
            (1, "=AVERAGE(A1:A2,2)", 1.0, 2.0 / 3.0),
            (2, "=SUMSQ(A1:A2,2)", 5.0, 4.0),
            (3, "=COUNT(A1:A2,2)", 3.0, 3.0),
            (4, "=MEDIAN(A1:A2,2)", 1.0, 0.0),
        ] {
            let output = CellId::new(0, row, 1);
            book.set_formula(output, formula).unwrap();
            assert_eq!(book.value(output), Value::Number(before), "{formula}");
            book.set_formula(input, "=FALSE()").unwrap();
            assert_eq!(book.value(output), Value::Number(after), "{formula}");
            book.set_formula(input, "=TRUE()").unwrap();
        }
    }

    #[test]
    fn empty_paths_text_and_empty_ranges_follow_their_aggregate_contracts() {
        let mut book = Workbook::default();
        let output = CellId::new(0, 0, 1);
        for (formula, expected) in [
            ("=PRODUCT(A1:A2)", Value::Number(0.0)),
            ("=MEDIAN(A1:A2)", Value::Error(CalcError::InvalidValue)),
            ("=SUMSQ(TRUE(),2)", Value::Number(5.0)),
            ("=SUMSQ(\"3\",2)", Value::Error(CalcError::InvalidValue)),
            ("=VAR(\"3\",2)", Value::Error(CalcError::IllegalParameter)),
            ("=VAR({TRUE,2})", Value::Number(0.5)),
            (
                "=VAR(IF({0,1},{10,20}))",
                Value::Error(CalcError::DivisionByZero),
            ),
            ("=AVERAGEA(IF({0,1},{10,20}))", Value::Number(20.0)),
            ("=COUNTBLANK(IF({0,1},{10,20}))", Value::Number(1.0)),
            ("=GCD(\"3\",2)", Value::Number(1.0)),
            ("=LCM(\"3\",2)", Value::Number(6.0)),
            ("=LCM(A1:A1)", Value::Number(1.0)),
            ("=LCM(A1)", Value::Number(0.0)),
            ("=GCD({2,FALSE})", Value::Number(0.0)),
            ("=LCM({2,TRUE})", Value::Number(1.0)),
            ("=LCM({TRUE,2})", Value::Number(2.0)),
            (
                "=ERROR.TYPE(GCD(-1,2))",
                Value::Error(CalcError::NotAvailable),
            ),
            ("=GCD({\"3\",2})", Value::Error(CalcError::IllegalArgument)),
            (
                "=LCM(IF({0,1},{10,20}))",
                Value::Error(CalcError::IllegalArgument),
            ),
        ] {
            book.set_formula(output, formula).unwrap();
            assert_eq!(book.value(output), expected, "{formula}");
        }
    }

    #[test]
    fn calc_summation_order_and_logical_inspection_remain_observable() {
        let mut book = Workbook::default();
        book.set_formula(CellId::new(0, 0, 0), "=TRUE()").unwrap();
        let output = CellId::new(0, 0, 1);
        for (formula, expected) in [
            ("=SUM(1E16,1,-1E16)", Value::Number(0.0)),
            ("=SUM(1E16,-1E16,1)", Value::Number(0.0)),
            ("=SUM({1E16,-1E16,1})", Value::Number(1.0)),
            ("=AVERAGE({1E16,1,-1E16})", Value::Number(0.0)),
            ("=MIN(A1,2)", Value::Boolean(true)),
            ("=MIN(2,A1)", Value::Number(1.0)),
            ("=ISNUMBER(MIN(A1,2))", Value::Boolean(true)),
            ("=ISLOGICAL(MIN(A1,2))", Value::Boolean(true)),
        ] {
            book.set_formula(output, formula).unwrap();
            assert_eq!(book.value(output), expected, "{formula}");
        }
    }
}
