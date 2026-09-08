//! Database aggregates share field resolution and row-wise criteria semantics.
use super::*;

impl Workbook {
    pub(super) fn database_aggregate(
        &self,
        function: Function,
        arguments: &[Expr<usize>],
    ) -> Result<Value, CalcError> {
        let [database, field, criteria] = arguments else {
            return Err(CalcError::InvalidArguments);
        };
        let database = ArrayInput::new(database, self)?;
        let criteria = ArrayInput::new(criteria, self)?;
        let (rows, columns) = database.shape();
        let (criteria_rows, criteria_columns) = criteria.shape();
        if rows < 2 || criteria_rows < 2 {
            return Err(CalcError::InvalidValue);
        }
        if (rows - 1)
            .checked_mul(criteria_rows - 1)
            .and_then(|count| count.checked_mul(criteria_columns))
            .is_none_or(|count| count > 50_000_000)
        {
            return Err(CalcError::InvalidNumber);
        }
        let header = |name: &str| {
            (0..columns).find(|column| matches!(database.value(self, *column), Value::Text(heading) if heading.eq_ignore_ascii_case(name)))
        };
        let field = match self.evaluate(field) {
            Value::Text(name) => header(&name),
            Value::Number(number)
                if number.is_finite() && number >= 1.0 && number.trunc() <= columns as f64 =>
            {
                Some(number.trunc() as usize - 1)
            }
            Value::Error(error) => return Err(error),
            _ => None,
        }
        .ok_or(CalcError::InvalidValue)?;
        let fields = (0..criteria_columns)
            .map(|column| match criteria.value(self, column) {
                Value::Text(name) => header(&name).ok_or(CalcError::InvalidValue),
                Value::Error(error) => Err(error),
                _ => Err(CalcError::InvalidValue),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut filters = Vec::new();
        for row in 1..criteria_rows {
            let mut filter = Vec::new();
            for (column, field) in fields.iter().enumerate() {
                let mut criterion = criteria.value(self, row * criteria_columns + column);
                if matches!(&criterion, Value::Blank)
                    || matches!(&criterion, Value::Text(text) if text.is_empty())
                {
                    continue;
                }
                // Database bare text is a prefix, unlike COUNTIF's exact text.
                if let Value::Text(text) = &mut criterion
                    && !text.starts_with(['=', '<', '>'])
                    && matches!(parse_criterion(text).1, Value::Text(_))
                {
                    text.push('*');
                }
                filter.push((*field, criterion));
            }
            filters.push(filter);
        }
        let mut values = Vec::new();
        for row in 1..rows {
            let mut accepted = false;
            for filter in &filters {
                let mut matches = true;
                for (column, criterion) in filter {
                    if !criterion_matches(
                        database.value(self, row * columns + column),
                        criterion.clone(),
                    )? {
                        matches = false;
                        break;
                    }
                }
                if matches {
                    accepted = true;
                    break;
                }
            }
            if accepted {
                match database.value(self, row * columns + field) {
                    Value::Number(value) => values.push(value),
                    Value::Error(error) => return Err(error),
                    _ => {}
                }
            }
        }
        Ok(match function {
            Function::DAverage if values.is_empty() => Value::Error(CalcError::DivisionByZero),
            Function::DAverage => Value::Number(
                values
                    .iter()
                    .map(|value| value / values.len() as f64)
                    .sum::<f64>(),
            ),
            Function::DMin => Value::Number(values.iter().copied().reduce(f64::min).unwrap_or(0.0)),
            Function::DMax => Value::Number(values.iter().copied().reduce(f64::max).unwrap_or(0.0)),
            Function::DStDev => deviation(&values, true, true),
            _ => return Err(CalcError::InvalidArguments),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn database_criteria_combine_rows_columns_prefixes_and_errors() {
        let mut workbook = Workbook::default();
        let database = "{\"Tree\",\"Height\",\"Yield\";\"Apple\",18,14;\"Pear\",12,10;\"Cherry\",13,9;\"Apple\",14,10;\"Pear\",9,8;\"Apple\",8,6}";
        for (function, field, criteria, expected) in [
            (
                "DAVERAGE",
                "\"Yield\"",
                "{\"Tree\",\"Height\";\"=Apple\",\">10\"}",
                12.0,
            ),
            ("DMAX", "3", "{\"Tree\";\"Pe\"}", 10.0),
            ("DMIN", "\"yield\"", "{\"Tree\";\"=?p*\"}", 6.0),
            (
                "DAVERAGE",
                "3",
                "{\"Tree\",\"Height\",\"Height\";\"=Apple\",\">10\",\"<16\";\"=Pear\",\"\",\"\"}",
                28.0 / 3.0,
            ),
            ("DMAX", "3", "{\"Tree\";\"\"}", 14.0),
            ("DMIN", "3", "{\"Tree\";\"=Missing\"}", 0.0),
            ("DSTDEV", "3", "{\"Tree\";\"=Pear\"}", 2.0_f64.sqrt()),
        ] {
            let formula = format!("={function}({database},{field},{criteria})");
            let cell = CellId::new(0, 0, 0);
            workbook.set_formula(cell, &formula).unwrap();
            assert_eq!(workbook.value(cell), Value::Number(expected), "{formula}");
        }
        for (formula, expected) in [
            (
                "=DAVERAGE({\"A\";1},1,{\"A\";2})",
                CalcError::DivisionByZero,
            ),
            ("=DSTDEV({\"A\";1},1,{\"A\";1})", CalcError::DivisionByZero),
            (
                "=DMAX({\"A\";1},1,{\"Expression\";TRUE})",
                CalcError::InvalidValue,
            ),
            (
                "=DMAX({\"A\";#N/A},1,{\"A\";\"\"})",
                CalcError::NotAvailable,
            ),
        ] {
            let cell = CellId::new(0, 0, 0);
            workbook.set_formula(cell, formula).unwrap();
            assert_eq!(workbook.value(cell), Value::Error(expected), "{formula}");
        }
    }
}
