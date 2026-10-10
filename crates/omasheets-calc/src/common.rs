//! Common scalar, statistical and financial functions, sharing the engine's
//! reference, array, coercion and error rules rather than a second evaluator.
use super::*;

pub(super) fn elementwise(function: Function) -> bool {
    matches!(
        function,
        Function::Sin
            | Function::Cos
            | Function::Tan
            | Function::Asin
            | Function::Acos
            | Function::Atan
            | Function::Atan2
            | Function::Sinh
            | Function::Cosh
            | Function::Tanh
            | Function::Asinh
            | Function::Acosh
            | Function::Atanh
            | Function::Degrees
            | Function::Radians
            | Function::Quotient
            | Function::MRound
            | Function::Even
            | Function::Odd
            | Function::Fact
            | Function::FactDouble
            | Function::Combin
            | Function::Combina
            | Function::IsEven
            | Function::IsOdd
            | Function::Search
            | Function::Substitute
            | Function::Replace
            | Function::Clean
            | Function::Proper
            | Function::Time
            | Function::Hour
            | Function::Minute
            | Function::Second
            | Function::Days
            | Function::Fv
            | Function::Nper
            | Function::Ipmt
            | Function::Ppmt
            | Function::Sln
            | Function::Syd
    )
}

impl Workbook {
    pub(super) fn evaluate_common_function(
        &self,
        function: Function,
        arguments: &[Expr<usize>],
    ) -> Option<Value> {
        if matches!(
            function,
            Function::Large
                | Function::Small
                | Function::PercentileInc
                | Function::PercentileExc
                | Function::QuartileInc
                | Function::QuartileExc
                | Function::RankAvg
        ) {
            return Some(self.common_statistic(function, arguments));
        }
        if matches!(
            function,
            Function::SumSq | Function::CountBlank | Function::Gcd | Function::Lcm | Function::Xor
        ) {
            let mut values = Vec::new();
            for arg in arguments {
                self.flatten_values(arg, &mut values);
            }
            return Some(common_aggregate(function, &values, arguments.len()));
        }
        if !elementwise(function) {
            return None;
        }
        let values: Vec<_> = arguments.iter().map(|arg| self.evaluate(arg)).collect();
        Some(match first_error(&values) {
            Some(error) => Value::Error(error),
            None => common_scalar(function, &values),
        })
    }

    fn common_statistic(&self, function: Function, arguments: &[Expr<usize>]) -> Value {
        let rank = function == Function::RankAvg;
        if !(arguments.len() == 2 || (rank && arguments.len() == 3)) {
            return Value::Error(CalcError::InvalidArguments);
        }
        let data_arg = if rank { &arguments[1] } else { &arguments[0] };
        let mut values = Vec::new();
        self.flatten_values(data_arg, &mut values);
        if let Some(error) = first_error(&values) {
            return Value::Error(error);
        }
        let mut data: Vec<_> = values
            .iter()
            .filter_map(|v| match v {
                Value::Number(n) => Some(*n),
                _ => None,
            })
            .collect();
        if data.is_empty() {
            return Value::Error(CalcError::InvalidNumber);
        }
        data.sort_by(f64::total_cmp);
        let argument = if rank { &arguments[0] } else { &arguments[1] };
        let n = match number(self.evaluate(argument)) {
            Ok(n) => n,
            Err(e) => return Value::Error(e),
        };
        let result = match function {
            Function::Large | Function::Small => {
                // Calc floors SMALL ranks and rounds LARGE ranks upward.
                let k = if function == Function::Small {
                    n.floor()
                } else {
                    n.ceil()
                };
                if k < 1.0 || k > data.len() as f64 {
                    return Value::Error(CalcError::InvalidNumber);
                }
                data[if function == Function::Large {
                    data.len() - k as usize
                } else {
                    k as usize - 1
                }]
            }
            Function::RankAvg => {
                if !data.contains(&n) {
                    return Value::Error(CalcError::NotAvailable);
                }
                let ascending = match arguments.get(2) {
                    None => false,
                    Some(a) => match number(self.evaluate(a)) {
                        Ok(n) => n != 0.0,
                        Err(e) => return Value::Error(e),
                    },
                };
                let before = data
                    .iter()
                    .filter(|&&v| if ascending { v < n } else { v > n })
                    .count();
                let equal = data.iter().filter(|&&v| v == n).count();
                before as f64 + (equal as f64 + 1.0) / 2.0
            }
            _ => {
                let quartile = matches!(function, Function::QuartileInc | Function::QuartileExc);
                let fraction = if quartile { n.trunc() / 4.0 } else { n };
                if !(0.0..=1.0).contains(&fraction) {
                    return Value::Error(CalcError::InvalidNumber);
                }
                let exclusive = matches!(function, Function::PercentileExc | Function::QuartileExc);
                let position = if exclusive {
                    fraction * (data.len() + 1) as f64 - 1.0
                } else {
                    fraction * (data.len() - 1) as f64
                };
                if position < 0.0 || position > (data.len() - 1) as f64 {
                    return Value::Error(CalcError::InvalidNumber);
                }
                let lo = position.floor() as usize;
                let hi = position.ceil() as usize;
                data[lo] + (data[hi] - data[lo]) * (position - lo as f64)
            }
        };
        finite(result)
    }
}

fn finite(n: f64) -> Value {
    if n.is_finite() {
        Value::Number(n)
    } else {
        Value::Error(CalcError::InvalidNumber)
    }
}
fn nums(values: &[Value], count: std::ops::RangeInclusive<usize>) -> Result<Vec<f64>, CalcError> {
    if !count.contains(&values.len()) {
        return Err(CalcError::InvalidArguments);
    }
    values.iter().cloned().map(number).collect()
}
fn common_scalar(function: Function, values: &[Value]) -> Value {
    match function {
        Function::Sin => unary_number(values, f64::sin),
        Function::Cos => unary_number(values, f64::cos),
        Function::Tan => unary_number(values, f64::tan),
        Function::Asin => unary_number(values, f64::asin),
        Function::Acos => unary_number(values, f64::acos),
        Function::Atan => unary_number(values, f64::atan),
        Function::Sinh => unary_number(values, f64::sinh),
        Function::Cosh => unary_number(values, f64::cosh),
        Function::Tanh => unary_number(values, f64::tanh),
        Function::Asinh => unary_number(values, f64::asinh),
        Function::Acosh => unary_number(values, f64::acosh),
        Function::Atanh => unary_number(values, f64::atanh),
        Function::Degrees => unary_number(values, f64::to_degrees),
        Function::Radians => unary_number(values, f64::to_radians),
        Function::Atan2 | Function::Quotient | Function::MRound => {
            let n = match nums(values, 2..=2) {
                Ok(n) => n,
                Err(e) => return Value::Error(e),
            };
            match function {
                Function::Atan2 => finite(n[1].atan2(n[0])),
                Function::Quotient => {
                    if n[1] == 0.0 {
                        Value::Error(CalcError::DivisionByZero)
                    } else {
                        finite((n[0] / n[1]).trunc())
                    }
                }
                _ => {
                    if n[0] * n[1] < 0.0 {
                        Value::Error(CalcError::InvalidNumber)
                    } else if n[1] == 0.0 {
                        Value::Number(0.0)
                    } else {
                        finite((n[0] / n[1]).round() * n[1])
                    }
                }
            }
        }
        Function::Even | Function::Odd => unary_number(values, |n| {
            let v = n.abs().ceil();
            let rounded = if function == Function::Even {
                (v / 2.0).ceil() * 2.0
            } else if v % 2.0 == 0.0 {
                v + 1.0
            } else {
                v
            };
            if n < 0.0 { -rounded } else { rounded }
        }),
        Function::IsEven | Function::IsOdd => {
            let n = match nums(values, 1..=1) {
                Ok(n) => n[0],
                Err(e) => return Value::Error(e),
            };
            Value::Boolean((n.abs().trunc() % 2.0 == 0.0) == (function == Function::IsEven))
        }
        Function::Fact | Function::FactDouble => {
            let n = match nums(values, 1..=1) {
                Ok(n) => n[0],
                Err(e) => return Value::Error(e),
            };
            if !(0.0..=300.0).contains(&n) {
                return Value::Error(CalcError::InvalidNumber);
            }
            let mut result = 1.0;
            let mut k = n.trunc() as u32;
            while k > 1 {
                result *= f64::from(k);
                k -= if function == Function::Fact { 1 } else { 2 };
            }
            finite(result)
        }
        Function::Combin | Function::Combina => {
            let n = match nums(values, 2..=2) {
                Ok(n) => n,
                Err(e) => return Value::Error(e),
            };
            if n.iter().any(|n| *n < 0.0 || *n > 1e9) {
                return Value::Error(CalcError::InvalidNumber);
            }
            let mut a = n[0].trunc() as u64;
            let mut b = n[1].trunc() as u64;
            if function == Function::Combina && b > a {
                return Value::Error(CalcError::InvalidValue);
            }
            if function == Function::Combina {
                if b == 0 {
                    return Value::Number(1.0);
                }
                if a == 0 {
                    return Value::Error(CalcError::InvalidNumber);
                }
                a += b - 1;
            }
            if b > a {
                return Value::Error(CalcError::InvalidNumber);
            }
            b = b.min(a - b);
            // No unbounded loop for near-centre combinations of huge inputs.
            if b > 1024 {
                return Value::Error(CalcError::InvalidNumber);
            }
            let mut result = 1.0;
            for i in 1..=b {
                result *= (a - b + i) as f64 / i as f64;
                if !result.is_finite() {
                    break;
                }
            }
            finite(result.round())
        }
        Function::Clean => text_unary(values, |s| s.chars().filter(|c| *c as u32 >= 32).collect()),
        Function::Proper => text_unary(values, |s| {
            let mut start = true;
            let mut result = String::new();
            for ch in s.chars() {
                if start {
                    result.extend(ch.to_uppercase());
                } else {
                    result.extend(ch.to_lowercase());
                }
                start = !ch.is_alphabetic();
            }
            result
        }),
        Function::Search | Function::Substitute | Function::Replace => {
            common_text(function, values)
        }
        Function::Time | Function::Hour | Function::Minute | Function::Second | Function::Days => {
            common_time(function, values)
        }
        Function::Fv
        | Function::Nper
        | Function::Ipmt
        | Function::Ppmt
        | Function::Sln
        | Function::Syd => common_finance(function, values),
        _ => unreachable!("common scalar dispatch"),
    }
}

fn common_aggregate(function: Function, values: &[Value], arguments: usize) -> Value {
    if arguments == 0 || (function == Function::CountBlank && arguments != 1) {
        return Value::Error(CalcError::InvalidArguments);
    }
    if function == Function::CountBlank {
        return Value::Number(
            values
                .iter()
                .filter(|v| {
                    matches!(v, Value::Blank) || matches!(v, Value::Text(s) if s.is_empty())
                })
                .count() as f64,
        );
    }
    if let Some(e) = first_error(values) {
        return Value::Error(e);
    }
    if function == Function::SumSq {
        return finite(
            values
                .iter()
                .filter_map(|v| match v {
                    Value::Number(n) => Some(n * n),
                    _ => None,
                })
                .sum(),
        );
    }
    if function == Function::Xor {
        let mut count = 0;
        let mut result = false;
        for value in values {
            if matches!(value, Value::Text(_) | Value::Blank) {
                continue;
            }
            match truthy(value.clone()) {
                Ok(b) => {
                    result ^= b;
                    count += 1;
                }
                Err(e) => return Value::Error(e),
            }
        }
        return if count == 0 {
            Value::Error(CalcError::InvalidValue)
        } else {
            Value::Boolean(result)
        };
    }
    let mut result = if function == Function::Gcd {
        0u64
    } else {
        1u64
    };
    for value in values {
        if matches!(value, Value::Blank | Value::Text(_)) {
            continue;
        }
        let n = match number(value.clone()) {
            Ok(n) if (0.0..=9_007_199_254_740_991.0).contains(&n) => n.trunc() as u64,
            _ => return Value::Error(CalcError::InvalidNumber),
        };
        let (mut a, mut b) = (result, n);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        result = if function == Function::Gcd {
            a
        } else if result == 0 || n == 0 {
            0
        } else {
            match (result / a).checked_mul(n) {
                Some(n) if n <= 9_007_199_254_740_991 => n,
                _ => return Value::Error(CalcError::InvalidNumber),
            }
        };
    }
    Value::Number(result as f64)
}

fn common_text(function: Function, values: &[Value]) -> Value {
    let expected = if function == Function::Replace {
        4..=4
    } else if function == Function::Substitute {
        3..=4
    } else {
        2..=3
    };
    if !expected.contains(&values.len()) {
        return Value::Error(CalcError::InvalidArguments);
    }
    let text = match text_value(&values[0]) {
        Ok(t) => t,
        Err(e) => return Value::Error(e),
    };
    if function == Function::Replace {
        let (start, count) = match (text_count(&values[1]), text_count(&values[2])) {
            (Ok(a), Ok(b)) if a > 0 => (a - 1, b),
            _ => return Value::Error(CalcError::InvalidValue),
        };
        let replacement = match text_value(&values[3]) {
            Ok(t) => t,
            Err(e) => return Value::Error(e),
        };
        let chars: Vec<_> = text.chars().collect();
        let start = start.min(chars.len());
        let end = start.saturating_add(count).min(chars.len());
        let result = chars[..start].iter().collect::<String>()
            + &replacement
            + &chars[end..].iter().collect::<String>();
        return bounded_text(result);
    }
    let second = match text_value(&values[1]) {
        Ok(t) => t,
        Err(e) => return Value::Error(e),
    };
    if function == Function::Substitute {
        let replacement = match text_value(&values[2]) {
            Ok(t) => t,
            Err(e) => return Value::Error(e),
        };
        let instance = match values.get(3) {
            None => None,
            Some(v) => match text_count(v) {
                Ok(n) if n > 0 => Some(n),
                _ => return Value::Error(CalcError::InvalidValue),
            },
        };
        if second.is_empty() {
            return Value::Text(text);
        }
        let mut result = String::new();
        let mut tail = text.as_str();
        let mut count = 0;
        while let Some(pos) = tail.find(&second) {
            count += 1;
            result.push_str(&tail[..pos]);
            result.push_str(if instance.is_none() || instance == Some(count) {
                &replacement
            } else {
                &second
            });
            if result.len() > 131_068 {
                return Value::Error(CalcError::InvalidValue);
            }
            tail = &tail[pos + second.len()..];
        }
        result.push_str(tail);
        return bounded_text(result);
    }
    let start = match values.get(2) {
        None => 1,
        Some(v) => match text_count(v) {
            Ok(n) if n > 0 => n,
            _ => return Value::Error(CalcError::InvalidValue),
        },
    };
    let chars: Vec<_> = second.chars().collect();
    if text.is_empty() || start > chars.len() + 1 {
        return Value::Error(CalcError::InvalidValue);
    }
    // SEARCH is case-insensitive and accepts ?, * and ~ escapes. The
    // existing wildcard matcher also supplies the criteria functions.
    let pattern = format!("{}*", text.to_lowercase());
    for index in start - 1..=chars.len() {
        if wildcard_matches(
            &pattern,
            &chars[index..].iter().collect::<String>().to_lowercase(),
        ) {
            return Value::Number((index + 1) as f64);
        }
    }
    Value::Error(CalcError::InvalidValue)
}
fn bounded_text(text: String) -> Value {
    if text.chars().count() <= 32_767 {
        Value::Text(text)
    } else {
        Value::Error(CalcError::InvalidValue)
    }
}
fn common_time(function: Function, values: &[Value]) -> Value {
    let count = match function {
        Function::Time => 3,
        Function::Days => 2,
        _ => 1,
    };
    let n = match nums(values, count..=count) {
        Ok(n) => n,
        Err(e) => return Value::Error(e),
    };
    if function == Function::Time {
        if n.iter().any(|n| n.abs() > 1e9) {
            return Value::Error(CalcError::InvalidNumber);
        }
        let seconds = n[0] * 3600.0 + n[1] * 60.0 + n[2];
        return if seconds < 0.0 {
            Value::Error(CalcError::InvalidNumber)
        } else {
            finite(seconds.rem_euclid(86_400.0) / 86_400.0)
        };
    }
    if function == Function::Days {
        return finite(n[0] - n[1]);
    }
    if n[0] < 0.0 {
        return Value::Error(CalcError::InvalidNumber);
    }
    let seconds = (n[0].fract() * 86_400.0).round() as u64 % 86_400;
    Value::Number(match function {
        Function::Hour => seconds / 3600,
        Function::Minute => seconds / 60 % 60,
        _ => seconds % 60,
    } as f64)
}
fn common_finance(function: Function, values: &[Value]) -> Value {
    if matches!(function, Function::Sln | Function::Syd) {
        let count = if function == Function::Sln { 3 } else { 4 };
        let n = match nums(values, count..=count) {
            Ok(n) => n,
            Err(e) => return Value::Error(e),
        };
        if n[2] <= 0.0 {
            return Value::Error(CalcError::DivisionByZero);
        }
        if function == Function::Syd && (n[3] <= 0.0 || n[3] > n[2]) {
            return Value::Error(CalcError::InvalidNumber);
        }
        return finite(if function == Function::Sln {
            (n[0] - n[1]) / n[2]
        } else {
            (n[0] - n[1]) * (n[2] - n[3] + 1.0) * 2.0 / (n[2] * (n[2] + 1.0))
        });
    }
    if matches!(function, Function::Ipmt | Function::Ppmt) {
        let n = match nums(values, 4..=6) {
            Ok(n) => n,
            Err(e) => return Value::Error(e),
        };
        let (rate, period, periods, pv) = (n[0], n[1], n[2], n[3]);
        let fv = n.get(4).copied().unwrap_or(0.0);
        let due = n.get(5).copied().unwrap_or(0.0) != 0.0;
        if period < 1.0 || period > periods {
            return Value::Error(CalcError::InvalidNumber);
        }
        let pmt = match payment(rate, periods, pv, fv, due) {
            Ok(n) => n,
            Err(e) => return Value::Error(e),
        };
        let interest = if rate == 0.0 || (due && period == 1.0) {
            0.0
        } else {
            let growth = (1.0 + rate).powf(period - 1.0);
            let balance =
                pv * growth + pmt * (if due { 1.0 + rate } else { 1.0 }) * (growth - 1.0) / rate;
            -balance * rate / if due { 1.0 + rate } else { 1.0 }
        };
        return finite(if function == Function::Ipmt {
            interest
        } else {
            pmt - interest
        });
    }
    let n = match nums(values, 3..=5) {
        Ok(n) => n,
        Err(e) => return Value::Error(e),
    };
    let rate = n[0];
    let due = n.get(4).copied().unwrap_or(0.0) != 0.0;
    let timing = if due { 1.0 + rate } else { 1.0 };
    if function == Function::Fv {
        let (periods, pmt, pv) = (n[1], n[2], n.get(3).copied().unwrap_or(0.0));
        return finite(if rate == 0.0 {
            -pv - pmt * periods
        } else {
            let growth = (1.0 + rate).powf(periods);
            -pv * growth - pmt * timing * (growth - 1.0) / rate
        });
    }
    let (pmt, pv, fv) = (n[1], n[2], n.get(3).copied().unwrap_or(0.0));
    if rate == 0.0 {
        return if pmt == 0.0 {
            Value::Error(CalcError::DivisionByZero)
        } else {
            finite(-(pv + fv) / pmt)
        };
    }
    let ratio = (pmt * timing - fv * rate) / (pv * rate + pmt * timing);
    finite(ratio.ln() / rate.ln_1p())
}
