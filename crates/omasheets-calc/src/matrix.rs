//! Bounded matrix operations with explicit shape and numeric-input checks.
use super::*;

impl Workbook {
    pub(super) fn matrix_array(
        &self,
        function: Function,
        arguments: &[Expr<usize>],
    ) -> Result<ArrayValue, CalcError> {
        if function == Function::Transpose {
            let [input] = arguments else {
                return Err(CalcError::InvalidArguments);
            };
            let input = self.evaluate_array(input)?;
            let mut values = Vec::with_capacity(input.values.len());
            for column in 0..input.columns {
                for row in 0..input.rows {
                    values.push(input.values[row * input.columns + column].clone());
                }
            }
            return Ok(ArrayValue {
                rows: input.columns,
                columns: input.rows,
                values,
            });
        }
        let [left, right] = arguments else {
            return Err(CalcError::InvalidArguments);
        };
        let left = self.evaluate_array(left)?;
        let right = self.evaluate_array(right)?;
        if left.columns != right.rows {
            return Err(CalcError::InvalidValue);
        }
        let count = left
            .rows
            .checked_mul(right.columns)
            .filter(|count| *count <= MAX_RANGE_CELLS)
            .ok_or(CalcError::InvalidNumber)?;
        if count
            .checked_mul(left.columns)
            .is_none_or(|terms| terms > 50_000_000)
        {
            return Err(CalcError::InvalidNumber);
        }
        let numbers = |array: ArrayValue| {
            array
                .values
                .into_iter()
                .map(|value| match value {
                    Value::Number(value) if value.is_finite() => Ok(value),
                    Value::Error(error) => Err(error),
                    _ => Err(CalcError::InvalidValue),
                })
                .collect::<Result<Vec<_>, _>>()
        };
        let (rows, inner, columns) = (left.rows, left.columns, right.columns);
        let left = numbers(left)?;
        let right = numbers(right)?;
        let mut values = Vec::with_capacity(count);
        for row in 0..rows {
            for column in 0..columns {
                let mut sum = 0.0;
                for k in 0..inner {
                    sum += left[row * inner + k] * right[k * columns + column];
                }
                values.push(if sum.is_finite() {
                    Value::Number(sum)
                } else {
                    Value::Error(CalcError::InvalidNumber)
                });
            }
        }
        Ok(ArrayValue {
            rows,
            columns,
            values,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matrix_composition_preserves_dimensions_types_and_dependencies() {
        let mut workbook = Workbook::default();
        for (formula, expected) in [
            ("=SUM(MMULT({1,2;3,4},{5,6;7,8}))", 134.0),
            ("=INDEX(TRANSPOSE({1,2,3;4,5,6}),3,2)", 6.0),
            ("=MMULT({1,2,3},TRANSPOSE({4,5,6}))", 32.0),
            ("=SUM(TRANSPOSE({1,2;3,4})*{1,2;3,4})", 29.0),
            ("=IFERROR(MMULT({1,2},{3,4}),17)", 17.0),
            ("=IFERROR(MMULT({1,TRUE},{3;4}),18)", 18.0),
            ("=IFERROR(MMULT({1,\"2\"},{3;4}),19)", 19.0),
            ("=IFERROR(MMULT({1,#N/A},{3;4}),20)", 20.0),
            ("=INDEX(TRANSPOSE({1,\"text\";TRUE,4}),2,2)", 4.0),
        ] {
            let cell = CellId::new(0, 9, 9);
            workbook.set_formula(cell, formula).unwrap();
            assert_eq!(workbook.value(cell), Value::Number(expected), "{formula}");
        }
        workbook.set_number(CellId::new(0, 0, 0), 2.0);
        workbook.set_number(CellId::new(0, 1, 0), 3.0);
        workbook
            .set_formula(CellId::new(0, 0, 2), "=MMULT(TRANSPOSE(A1:A2),A1:A2)")
            .unwrap();
        assert_eq!(workbook.value(CellId::new(0, 0, 2)), Value::Number(13.0));
        workbook.set_number(CellId::new(0, 1, 0), 4.0);
        assert_eq!(workbook.value(CellId::new(0, 0, 2)), Value::Number(20.0));
        workbook.clear(CellId::new(0, 1, 0));
        assert_eq!(
            workbook.value(CellId::new(0, 0, 2)),
            Value::Error(CalcError::InvalidValue)
        );
    }
}
