//! Statistical collections preserve reference provenance, empty paths and pairs.
//! Contracts are checked against the pinned Calc reference, independently of
//! these implementations of the standard statistical formulae.
use super::aggregate::{ArgumentKind, CompensatedSum};
use super::*;

impl Workbook {
    pub(super) fn evaluate_statistics(
        &self,
        function: Function,
        arguments: &[Expr<usize>],
    ) -> Option<Value> {
        match function {
            Function::ModeMulti => Some(
                self.mode_array(arguments)
                    .map_or_else(Value::Error, |array| array.at(0, 0)),
            ),
            Function::Large | Function::Small
                if arguments.len() == 2 && contains_array_operand(&arguments[1]) =>
            {
                Some(
                    self.order_selection_array(function, arguments)
                        .map_or_else(Value::Error, |array| array.at(0, 0)),
                )
            }
            Function::MinA | Function::MaxA => {
                Some(self.evaluate_simple_aggregate(function, arguments, false))
            }
            Function::StDevA
            | Function::StDevPA
            | Function::VarA
            | Function::VarPA
            | Function::AveDev
            | Function::DevSq
            | Function::GeoMean
            | Function::HarMean
            | Function::Mode
            | Function::ModeSingle
            | Function::Skew
            | Function::SkewP
            | Function::Kurt => Some(self.extended_statistic(function, arguments)),
            Function::TrimMean | Function::PercentRankInc | Function::PercentRankExc => {
                Some(self.order_statistic(function, arguments))
            }
            Function::Slope
            | Function::Intercept
            | Function::RSq
            | Function::StEyx
            | Function::Forecast
            | Function::Correl
            | Function::CovarianceP
            | Function::CovarianceS => Some(
                self.regression(function, arguments)
                    .map_or_else(Value::Error, number_value),
            ),
            _ => None,
        }
    }

    pub(super) fn visit_statistical_arguments(
        &self,
        arguments: &[Expr<usize>],
        text_as_zero: bool,
        reverse: bool,
        visit: &mut impl FnMut(&Value, ArgumentKind),
    ) {
        let mut argument = |arg: &Expr<usize>| {
            if matches!(
                arg,
                Expr::Function(Function::Index | Function::ReferenceSpan, _)
            ) {
                match self.reference_view(arg) {
                    Ok(reference) => {
                        reference.visit(self, &mut |value: &Value| {
                            visit(value, ArgumentKind::Reference)
                        });
                        return;
                    }
                    Err(CalcError::InvalidArguments) => {}
                    Err(error) => {
                        visit(&Value::Error(error), ArgumentKind::Reference);
                        return;
                    }
                }
            }
            // Calc's statistical matrix collection traverses columns first.
            if contains_array_operand(arg) && !matches!(arg, Expr::RangeNode { .. }) {
                match self.evaluate_array(arg) {
                    Ok(array) => {
                        for column in 0..array.columns {
                            for row in 0..array.rows {
                                if text_as_zero || !array.empty_path_at(row, column) {
                                    visit(&array.at(row, column), ArgumentKind::Array);
                                }
                            }
                        }
                    }
                    Err(error) => visit(&Value::Error(error), ArgumentKind::Array),
                }
            } else {
                self.visit_aggregate_arguments(std::slice::from_ref(arg), false, visit);
            }
        };
        if reverse {
            for arg in arguments.iter().rev() {
                argument(arg);
            }
        } else {
            for arg in arguments {
                argument(arg);
            }
        }
    }

    fn statistical_numbers(
        &self,
        arguments: &[Expr<usize>],
        text_as_zero: bool,
        reverse: bool,
        retain_intermediate_error: bool,
    ) -> Result<Vec<f64>, CalcError> {
        let mut numbers = Vec::new();
        let mut error = None;
        self.visit_statistical_arguments(arguments, text_as_zero, reverse, &mut |value, kind| {
            match value {
                Value::Number(n) => numbers.push(*n),
                Value::Boolean(b) => numbers.push(f64::from(*b)),
                Value::Text(_) if text_as_zero => numbers.push(0.0),
                Value::Blank if text_as_zero && kind == ArgumentKind::Array => numbers.push(0.0),
                Value::Text(_) if kind == ArgumentKind::Scalar => {
                    error.get_or_insert(CalcError::IllegalParameter);
                }
                Value::Error(e) => {
                    error.get_or_insert_with(|| {
                        if retain_intermediate_error {
                            statistical_error(e, kind)
                        } else {
                            e.clone()
                        }
                    });
                }
                _ => {}
            }
        });
        error.map_or(Ok(numbers), Err)
    }

    pub(super) fn mode_array(&self, arguments: &[Expr<usize>]) -> Result<ArrayValue, CalcError> {
        if arguments.is_empty() {
            return Err(CalcError::InvalidArguments);
        }
        let numbers = self.statistical_numbers(arguments, false, false, false)?;
        let modes = ordered_modes(&numbers)?;
        Ok(ArrayValue::from_parts(
            modes.len(),
            1,
            modes.into_iter().map(Value::Number).collect(),
        ))
    }

    pub(super) fn order_selection_array(
        &self,
        function: Function,
        arguments: &[Expr<usize>],
    ) -> Result<ArrayValue, CalcError> {
        let mut data = self.statistical_numbers(&arguments[..1], false, false, false)?;
        if data.is_empty() {
            return Err(CalcError::InvalidValue);
        }
        data.sort_by(f64::total_cmp);
        let selectors = self.evaluate_array(&arguments[1])?;
        let values = selectors
            .values
            .iter()
            .map(|value| {
                let n = match value {
                    Value::Text(_) | Value::Blank => Err(CalcError::InvalidValue),
                    Value::Error(error) => Err(CalcError::Pending(Box::new(error.clone()))),
                    value => number(value.clone()),
                };
                match n {
                    Err(error) => Value::Error(error),
                    Ok(n) => {
                        let k = if function == Function::Small {
                            n.floor()
                        } else {
                            n.ceil()
                        };
                        if k < 1.0 || k > data.len() as f64 {
                            Value::Error(CalcError::IllegalArgument)
                        } else {
                            Value::Number(
                                data[if function == Function::Small {
                                    k as usize - 1
                                } else {
                                    data.len() - k as usize
                                }],
                            )
                        }
                    }
                }
            })
            .collect();
        Ok(ArrayValue::from_parts(
            selectors.rows,
            selectors.columns,
            values,
        ))
    }

    fn extended_statistic(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        if arguments.is_empty() {
            return Value::Error(CalcError::InvalidArguments);
        }
        if function == Function::GeoMean {
            return self.geometric_mean(arguments);
        }
        let text_as_zero = matches!(
            function,
            Function::StDevA | Function::StDevPA | Function::VarA | Function::VarPA
        );
        let numbers = match self.statistical_numbers(
            arguments,
            text_as_zero,
            !matches!(function, Function::Mode | Function::ModeSingle),
            matches!(
                function,
                Function::AveDev
                    | Function::HarMean
                    | Function::Skew
                    | Function::SkewP
                    | Function::Kurt
            ),
        ) {
            Ok(numbers) => numbers,
            Err(error) => return Value::Error(error),
        };
        match function {
            Function::StDevA => deviation(&numbers, true, true),
            Function::StDevPA => deviation(&numbers, false, true),
            Function::VarA => deviation(&numbers, true, false),
            Function::VarPA => deviation(&numbers, false, false),
            Function::Mode | Function::ModeSingle => {
                mode(&numbers, function == Function::ModeSingle)
            }
            Function::HarMean => {
                if numbers.iter().any(|n| *n <= 0.0) {
                    return Value::Error(CalcError::IllegalArgument);
                }
                if numbers.is_empty() {
                    return Value::Error(CalcError::InvalidNumber);
                }
                number_value(numbers.len() as f64 / compensated(numbers.iter().map(|n| 1.0 / n)))
            }
            Function::AveDev | Function::DevSq => {
                if numbers.is_empty() {
                    return Value::Error(if function == Function::DevSq {
                        CalcError::DivisionByZero
                    } else {
                        CalcError::InvalidNumber
                    });
                }
                let mean = compensated(numbers.iter().copied()) / numbers.len() as f64;
                if function == Function::AveDev {
                    number_value(
                        compensated(numbers.iter().map(|n| (n - mean).abs()))
                            / numbers.len() as f64,
                    )
                } else {
                    number_value(compensated(numbers.iter().map(|n| (n - mean).powi(2))))
                }
            }
            Function::Skew | Function::SkewP | Function::Kurt => moments(&numbers, function),
            _ => unreachable!("extended statistical function"),
        }
    }

    fn geometric_mean(&self, arguments: &[Expr<usize>]) -> Value {
        let mut sum = CompensatedSum::default();
        let mut count = 0usize;
        let mut finished = None;
        self.visit_statistical_arguments(arguments, false, true, &mut |value, kind| {
            // A zero short-circuits numeric work, but errors in remaining
            // arguments still propagate when Calc drains its argument stack.
            if let Value::Error(error) = value {
                if !matches!(finished, Some(Value::Error(_))) {
                    finished = Some(Value::Error(
                        if matches!(finished, Some(Value::Number(0.0)))
                            && kind != ArgumentKind::Array
                        {
                            error.clone()
                        } else {
                            statistical_error(error, kind)
                        },
                    ));
                }
                return;
            }
            if finished.is_some() {
                return;
            }
            let number = match value {
                Value::Number(n) => *n,
                Value::Boolean(b) => f64::from(*b),
                Value::Text(_) if kind == ArgumentKind::Scalar => {
                    finished = Some(Value::Error(CalcError::IllegalParameter));
                    return;
                }
                _ => return,
            };
            if number == 0.0 {
                finished = Some(Value::Number(0.0));
            } else if number < 0.0 {
                finished = Some(Value::Error(CalcError::IllegalArgument));
            } else {
                sum.add(number.ln());
                count += 1;
            }
        });
        finished.unwrap_or_else(|| {
            if count == 0 {
                Value::Error(CalcError::InvalidNumber)
            } else {
                number_value((sum.total() / count as f64).exp())
            }
        })
    }

    pub(super) fn order_statistic(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        self.order_statistic_result(function, arguments)
            .map_or_else(Value::Error, number_value)
    }

    fn order_statistic_result(
        &self,
        function: Function,
        arguments: &[Expr<usize>],
    ) -> Result<f64, CalcError> {
        let rank = matches!(function, Function::Rank | Function::RankAvg);
        let percent_rank = matches!(
            function,
            Function::PercentRankInc | Function::PercentRankExc
        );
        if !(2..=if rank || percent_rank { 3 } else { 2 }).contains(&arguments.len()) {
            return Err(CalcError::InvalidArguments);
        }
        let n = number(self.evaluate(&arguments[usize::from(!rank)]))?;
        let extra = arguments
            .get(2)
            .map(|arg| number(self.evaluate(arg)))
            .transpose()?;
        let quartile = matches!(function, Function::QuartileInc | Function::QuartileExc);
        let exclusive = matches!(function, Function::PercentileExc | Function::QuartileExc);
        let fraction = if quartile { n.floor() / 4.0 } else { n };
        if matches!(
            function,
            Function::PercentileInc
                | Function::PercentileExc
                | Function::QuartileInc
                | Function::QuartileExc
        ) && (!(0.0..=1.0).contains(&fraction)
            || exclusive && (fraction == 0.0 || fraction == 1.0))
        {
            return Err(CalcError::IllegalArgument);
        }
        if function == Function::TrimMean && !(0.0..1.0).contains(&n) {
            return Err(CalcError::IllegalArgument);
        }
        let significance = extra.unwrap_or(3.0).floor();
        if percent_rank && significance < 1.0 {
            return Err(CalcError::IllegalArgument);
        }
        let mut data = self.statistical_numbers(
            std::slice::from_ref(&arguments[usize::from(rank)]),
            false,
            false,
            false,
        )?;
        if data.is_empty() {
            return Err(CalcError::InvalidValue);
        }
        data.sort_by(f64::total_cmp);
        match function {
            Function::Large | Function::Small => {
                let k = if function == Function::Small {
                    n.floor()
                } else {
                    n.ceil()
                };
                if k < 1.0 || k > data.len() as f64 {
                    return Err(CalcError::InvalidValue);
                }
                Ok(data[if function == Function::Small {
                    k as usize - 1
                } else {
                    data.len() - k as usize
                }])
            }
            Function::Rank | Function::RankAvg => {
                // Calc ranks by exact equality; arithmetic rounding is observable.
                let first = data.partition_point(|value| *value < n);
                let end = data.partition_point(|value| *value <= n);
                if first == end {
                    return Err(CalcError::NotAvailable);
                }
                let ahead = if extra.unwrap_or(0.0) != 0.0 {
                    first
                } else {
                    data.len() - end
                };
                Ok(ahead as f64
                    + if function == Function::RankAvg {
                        (end - first + 1) as f64 / 2.0
                    } else {
                        1.0
                    })
            }
            Function::TrimMean => {
                let trim = (n * data.len() as f64).floor() as usize / 2;
                let kept = &data[trim..data.len() - trim];
                Ok(compensated(kept.iter().copied()) / kept.len() as f64)
            }
            Function::PercentRankInc | Function::PercentRankExc => {
                if n < data[0] || n > data[data.len() - 1] {
                    return Err(CalcError::InvalidValue);
                }
                if data.len() == 1 {
                    return Ok(1.0);
                }
                let upper = data.partition_point(|value| *value < n);
                let position = if data[upper] == n {
                    upper as f64
                } else {
                    upper as f64 - 1.0 + (n - data[upper - 1]) / (data[upper] - data[upper - 1])
                };
                let result = if function == Function::PercentRankExc {
                    (position + 1.0) / (data.len() + 1) as f64
                } else {
                    position / (data.len() - 1) as f64
                };
                // Calc rounds to significant digits, not a fixed decimal count.
                if result == 0.0 || significance >= 16.0 {
                    return Ok(result);
                }
                let scale = 10.0f64.powf(significance - 1.0 - result.log10().floor());
                Ok((result * scale).round() / scale)
            }
            _ => {
                let position = if exclusive {
                    fraction * (data.len() + 1) as f64 - 1.0
                } else {
                    fraction * (data.len() - 1) as f64
                };
                if position < 0.0 || position > (data.len() - 1) as f64 {
                    return Err(CalcError::IllegalArgument);
                }
                let lower = position.floor() as usize;
                let upper = position.ceil() as usize;
                Ok(data[lower] + (data[upper] - data[lower]) * (position - lower as f64))
            }
        }
    }

    fn regression(&self, function: Function, arguments: &[Expr<usize>]) -> Result<f64, CalcError> {
        let forecast = usize::from(function == Function::Forecast);
        if arguments.len() != 2 + forecast {
            return Err(CalcError::InvalidArguments);
        }
        let y = self.evaluate_array(&arguments[forecast])?;
        let x = self.evaluate_array(&arguments[forecast + 1])?;
        if x.rows != y.rows || x.columns != y.columns {
            return Err(CalcError::IllegalArgument);
        }
        let mut pairs = Vec::new();
        for column in 0..x.columns {
            for row in 0..x.rows {
                let left = x.at(row, column);
                let right = y.at(row, column);
                if let Value::Error(error) = left {
                    return Err(error.clone());
                }
                if let Value::Error(error) = right {
                    return Err(error.clone());
                }
                if x.empty_path_at(row, column) || y.empty_path_at(row, column) {
                    continue;
                }
                if let (Some(x), Some(y)) =
                    (numeric_observation(&left), numeric_observation(&right))
                {
                    pairs.push((x, y));
                }
            }
        }
        if pairs.len()
            < if function == Function::StEyx {
                3
            } else if function == Function::CovarianceS {
                2
            } else {
                1
            }
        {
            return Err(CalcError::InvalidValue);
        }
        let count = pairs.len() as f64;
        let mx = compensated(pairs.iter().map(|p| p.0)) / count;
        let my = compensated(pairs.iter().map(|p| p.1)) / count;
        let xx = compensated(pairs.iter().map(|p| (p.0 - mx).powi(2)));
        let yy = compensated(pairs.iter().map(|p| (p.1 - my).powi(2)));
        let xy = compensated(pairs.iter().map(|p| (p.0 - mx) * (p.1 - my)));
        match function {
            Function::CovarianceP => Ok(xy / count),
            Function::CovarianceS if count > 1.0 => Ok(xy / (count - 1.0)),
            Function::CovarianceS => Err(CalcError::DivisionByZero),
            Function::Correl | Function::RSq => {
                if xx == 0.0 || yy == 0.0 {
                    return Err(CalcError::DivisionByZero);
                }
                let r = xy / (xx * yy).sqrt();
                Ok(if function == Function::RSq { r * r } else { r })
            }
            _ if xx == 0.0 => Err(CalcError::DivisionByZero),
            Function::Slope => Ok(xy / xx),
            Function::Intercept => Ok(my - xy / xx * mx),
            Function::Forecast => Ok(my + xy / xx * (number(self.evaluate(&arguments[0]))? - mx)),
            Function::StEyx => Ok(((yy - xy * xy / xx) / (count - 2.0)).max(0.0).sqrt()),
            _ => unreachable!("paired statistic"),
        }
    }
}

fn statistical_error(error: &CalcError, kind: ArgumentKind) -> CalcError {
    match kind {
        ArgumentKind::ErrorLiteral => CalcError::IllegalParameter,
        ArgumentKind::Reference => error.clone(),
        _ => match error {
            CalcError::Pending(_) => error.clone(),
            error => CalcError::Pending(Box::new(error.clone())),
        },
    }
}

fn numeric_observation(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => Some(*n),
        Value::Boolean(b) => Some(f64::from(*b)),
        _ => None,
    }
}
fn compensated(values: impl Iterator<Item = f64>) -> f64 {
    let mut sum = CompensatedSum::default();
    for value in values {
        sum.add(value);
    }
    sum.total()
}
fn all_modes(numbers: &[f64]) -> Result<Vec<f64>, CalcError> {
    let mut sorted = numbers.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mut modes = Vec::new();
    let mut maximum = 1;
    let mut start = 0;
    while start < sorted.len() {
        let mut end = start + 1;
        while end < sorted.len() && sorted[end] == sorted[start] {
            end += 1;
        }
        let count = end - start;
        if count > maximum {
            maximum = count;
            modes.clear();
        }
        if count == maximum && count > 1 {
            modes.push(sorted[start]);
        }
        start = end;
    }
    if modes.is_empty() {
        return Err(CalcError::InvalidValue);
    }
    Ok(modes)
}
fn mode(numbers: &[f64], first_encountered: bool) -> Value {
    let modes = match if first_encountered {
        ordered_modes(numbers)
    } else {
        all_modes(numbers)
    } {
        Ok(modes) => modes,
        Err(error) => return Value::Error(error),
    };
    Value::Number(modes[0])
}
fn ordered_modes(numbers: &[f64]) -> Result<Vec<f64>, CalcError> {
    let key = |n: f64| if n == 0.0 { 0 } else { n.to_bits() };
    let mut remaining: std::collections::HashSet<_> =
        all_modes(numbers)?.into_iter().map(key).collect();
    // Preserve first encounter order without a full input scan for every tie.
    Ok(numbers
        .iter()
        .copied()
        .filter(|n| remaining.remove(&key(*n)))
        .collect())
}
fn moments(numbers: &[f64], function: Function) -> Value {
    let kurtosis = function == Function::Kurt;
    if numbers.len() < if kurtosis { 4 } else { 3 } {
        return Value::Error(CalcError::DivisionByZero);
    }
    let n = numbers.len() as f64;
    let mean = compensated(numbers.iter().copied()) / n;
    let variance = compensated(numbers.iter().map(|v| (v - mean).powi(2)))
        / if function == Function::SkewP {
            n
        } else {
            n - 1.0
        };
    if variance == 0.0 {
        return Value::Error(if kurtosis {
            CalcError::DivisionByZero
        } else {
            CalcError::IllegalArgument
        });
    }
    let sd = variance.sqrt();
    let moment = compensated(
        numbers
            .iter()
            .map(|v| ((v - mean) / sd).powi(if kurtosis { 4 } else { 3 })),
    );
    number_value(if kurtosis {
        moment * n * (n + 1.0) / ((n - 1.0) * (n - 2.0) * (n - 3.0))
            - 3.0 * (n - 1.0).powi(2) / ((n - 2.0) * (n - 3.0))
    } else if function == Function::SkewP {
        moment / n
    } else {
        moment * n / ((n - 1.0) * (n - 2.0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_statistical_errors_publish_for_cell_references() {
        let mut book = Workbook::default();
        let input = CellId::new(0, 0, 0);
        let result = CellId::new(0, 0, 1);
        let inline = CellId::new(0, 0, 2);
        let referenced = CellId::new(0, 0, 3);
        book.set_number(input, 0.0);
        book.set_formula(result, "=AVEDEV(1/A1,2)").unwrap();
        book.set_formula(inline, "=ERROR.TYPE(AVEDEV(1/A1,2))")
            .unwrap();
        book.set_formula(referenced, "=ERROR.TYPE(B1)").unwrap();
        assert_eq!(book.value(result), Value::Error(CalcError::DivisionByZero));
        assert_eq!(book.value(inline), Value::Error(CalcError::NotAvailable));
        assert_eq!(book.value(referenced), Value::Number(2.0));
        book.set_number(input, 1.0);
        assert_eq!(book.value(result), Value::Number(0.5));
        assert_eq!(book.value(inline), Value::Error(CalcError::NotAvailable));
        assert_eq!(
            book.value(referenced),
            Value::Error(CalcError::NotAvailable)
        );
    }

    #[test]
    fn statistics_preserve_text_logical_and_empty_path_contracts() {
        let mut book = Workbook::default();
        let output = CellId::new(0, 0, 0);
        for (formula, expected) in [
            ("=MINA(IF({0,1,1},{10,20,30}))", Value::Number(0.0)),
            ("=VARA(\"3\",2,4)", Value::Number(4.0)),
            ("=SMALL({TRUE,FALSE,2,4},2)", Value::Number(1.0)),
            (
                "=RANK(0.1+0.2,{0.3,1})",
                Value::Error(CalcError::NotAvailable),
            ),
            ("=PERCENTRANK({1,2,2,4,8,16},3)", Value::Number(0.5)),
            (
                "=QUARTILE({1,2,3,4},-0.1)",
                Value::Error(CalcError::IllegalArgument),
            ),
            ("=ERROR.TYPE(SMALL({1,2},0))", Value::Number(3.0)),
            (
                "=ERROR.TYPE(PERCENTILE.EXC({1,2},0))",
                Value::Error(CalcError::NotAvailable),
            ),
            ("=GEOMEAN({0,#N/A})", Value::Error(CalcError::NotAvailable)),
            ("=HARMEAN({0,2})", Value::Error(CalcError::IllegalArgument)),
            ("=SKEW({2,2,2})", Value::Error(CalcError::IllegalArgument)),
            ("=KURT({2,2,2,2})", Value::Error(CalcError::DivisionByZero)),
        ] {
            book.set_formula(output, formula).unwrap();
            assert_eq!(book.value(output), expected, "{formula}");
        }
    }

    #[test]
    fn multimode_and_order_selections_remain_arrays_in_consumers() {
        let mut book = Workbook::default();
        let output = CellId::new(0, 0, 0);
        for (formula, expected) in [
            ("=MODE({4,4,2,2})", Value::Number(2.0)),
            ("=MODE.SNGL({4,4,2,2})", Value::Number(4.0)),
            ("=SUM(MODE.MULT({4,4,2,2}))", Value::Number(6.0)),
            ("=INDEX(MODE.MULT({4,4,2,2}),2)", Value::Number(2.0)),
            ("=SUM(LARGE({1,2,3,4},{1,2}))", Value::Number(7.0)),
            ("=SUM(SMALL({1,2,3,4},{1;2}))", Value::Number(3.0)),
            (
                "=SUM(LARGE({1,2,3,4},{0,2}))",
                Value::Error(CalcError::IllegalArgument),
            ),
            (
                "=ERROR.TYPE(SUM(SMALL({1,2,3,4},{0,2})))",
                Value::Error(CalcError::NotAvailable),
            ),
        ] {
            book.set_formula(output, formula).unwrap();
            assert_eq!(book.value(output), expected, "{formula}");
        }
    }

    #[test]
    fn regression_tracks_aligned_pairs_and_recalculates_after_edits() {
        let mut book = Workbook::default();
        for row in 0..3 {
            book.set_number(CellId::new(0, row, 0), (row + 1) as f64);
        }
        let slope = CellId::new(0, 0, 1);
        book.set_formula(slope, "=SLOPE({2;4;6},A1:A3)").unwrap();
        assert_eq!(book.value(slope), Value::Number(2.0));
        book.set_number(CellId::new(0, 2, 0), 4.0);
        let Value::Number(value) = book.value(slope) else {
            panic!("numeric slope");
        };
        assert!((value - 9.0 / 7.0).abs() < 1e-12);
        for (formula, expected) in [
            (
                "=SLOPE({1;2},{3,4})",
                Value::Error(CalcError::IllegalArgument),
            ),
            ("=STEYX({1,2},{3,4})", Value::Error(CalcError::InvalidValue)),
            ("=SLOPE({TRUE,2,4},{1,2,3})", Value::Number(1.5)),
            ("=SLOPE(IF({0,1,1},{10,2,4}),{1,2,3})", Value::Number(2.0)),
            (
                "=COVARIANCE.S({1},{2})",
                Value::Error(CalcError::InvalidValue),
            ),
        ] {
            book.set_formula(slope, formula).unwrap();
            assert_eq!(book.value(slope), expected, "{formula}");
        }
    }
}
