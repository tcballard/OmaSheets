//! Reference-valued selections retain a bounded, stable dependency envelope.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct ReferenceView {
    node: Option<usize>,
    anchor: CellId,
    row: usize,
    column: usize,
    pub(super) rows: usize,
    pub(super) columns: usize,
}

impl ReferenceView {
    fn cell(self, workbook: &Workbook, row: usize, column: usize) -> CellId {
        if let Some(node) = self.node {
            match workbook.range_shape(node) {
                RangeShape::Rectangle { anchor, .. } => CellId::new(
                    anchor.sheet,
                    anchor.row + (self.row + row) as u32,
                    anchor.column + (self.column + column) as u32,
                ),
                RangeShape::Members { columns, .. } => {
                    let index = (self.row + row) * columns + self.column + column;
                    workbook.cells[workbook.cells[node].dependencies[index]].id
                }
            }
        } else {
            CellId::new(
                self.anchor.sheet,
                self.anchor.row + (self.row + row) as u32,
                self.anchor.column + (self.column + column) as u32,
            )
        }
    }

    fn position(self, workbook: &Workbook, cell: CellId) -> Option<(usize, usize)> {
        let (row, column) = match self.node.map(|node| (node, workbook.range_shape(node))) {
            Some((node, RangeShape::Members { columns, .. })) => {
                let index = workbook.cells[node]
                    .dependencies
                    .iter()
                    .position(|index| workbook.cells[*index].id == cell)?;
                (index / columns, index % columns)
            }
            _ => {
                if cell.sheet != self.anchor.sheet {
                    return None;
                }
                (
                    cell.row.checked_sub(self.anchor.row)? as usize,
                    cell.column.checked_sub(self.anchor.column)? as usize,
                )
            }
        };
        let row = row.checked_sub(self.row)?;
        let column = column.checked_sub(self.column)?;
        (row < self.rows && column < self.columns).then_some((row, column))
    }

    fn select(self, row: usize, column: usize, rows: usize, columns: usize) -> Self {
        Self {
            row: self.row + row,
            column: self.column + column,
            rows,
            columns,
            ..self
        }
    }

    pub(super) fn value(self, workbook: &Workbook, index: usize) -> Value {
        workbook.value(self.cell(workbook, index / self.columns, index % self.columns))
    }

    pub(super) fn array(self, workbook: &Workbook) -> ArrayValue {
        ArrayValue {
            rows: self.rows,
            columns: self.columns,
            values: (0..self.rows * self.columns)
                .map(|index| self.value(workbook, index))
                .collect(),
        }
    }

    pub(super) fn scalar(self, workbook: &Workbook) -> Value {
        if self.rows == 1 && self.columns == 1 {
            return self.value(workbook, 0);
        }
        let origin = workbook.evaluating.get();
        for index in 0..self.rows * self.columns {
            let cell = self.cell(workbook, index / self.columns, index % self.columns);
            if (self.columns == 1 || cell.column == origin.column)
                && (self.rows == 1 || cell.row == origin.row)
            {
                return workbook.value(cell);
            }
        }
        Value::Error(CalcError::InvalidValue)
    }

    pub(super) fn visit(self, workbook: &Workbook, mut visit: impl FnMut(&Value)) {
        if self
            .node
            .is_none_or(|node| matches!(workbook.range_shape(node), RangeShape::Rectangle { .. }))
        {
            workbook.for_each_rectangle_cell(
                self.cell(workbook, 0, 0),
                self.rows,
                self.columns,
                |_, index| visit(&workbook.cells[index].value),
            );
        } else {
            for index in 0..self.rows * self.columns {
                visit(&self.value(workbook, index));
            }
        }
    }
}

impl Workbook {
    pub(super) fn reference_view(
        &self,
        expression: &Expr<usize>,
    ) -> Result<ReferenceView, CalcError> {
        match expression {
            Expr::Reference(index) => Ok(ReferenceView {
                node: None,
                anchor: self.cells[*index].id,
                row: 0,
                column: 0,
                rows: 1,
                columns: 1,
            }),
            Expr::RangeNode {
                node,
                rows,
                columns,
            } => Ok(ReferenceView {
                node: Some(*node),
                anchor: self.cells[*node].id,
                row: 0,
                column: 0,
                rows: *rows,
                columns: *columns,
            }),
            Expr::Function(Function::Index, arguments) if matches!(arguments.len(), 2 | 3) => {
                let input = self.reference_view(&arguments[0])?;
                let (row, column, rows, columns) =
                    self.index_selection(arguments, input.rows, input.columns)?;
                Ok(input.select(row, column, rows, columns))
            }
            Expr::Function(Function::ReferenceSpan, arguments) => self.reference_span(arguments),
            Expr::Error(error) => Err(error.clone()),
            _ => Err(CalcError::InvalidArguments),
        }
    }

    pub(super) fn reference_span(
        &self,
        arguments: &[Expr<usize>],
    ) -> Result<ReferenceView, CalcError> {
        let [first, last, envelope] = arguments else {
            return Err(CalcError::InvalidArguments);
        };
        let first = self.reference_view(first)?;
        let last = self.reference_view(last)?;
        let envelope = self.reference_view(envelope)?;
        let endpoints = [
            first.cell(self, 0, 0),
            first.cell(self, first.rows - 1, first.columns - 1),
            last.cell(self, 0, 0),
            last.cell(self, last.rows - 1, last.columns - 1),
        ];
        let mut min_row = usize::MAX;
        let mut min_column = usize::MAX;
        let mut max_row = 0;
        let mut max_column = 0;
        for cell in endpoints {
            let (row, column) = envelope
                .position(self, cell)
                .ok_or(CalcError::InvalidReference)?;
            min_row = min_row.min(row);
            min_column = min_column.min(column);
            max_row = max_row.max(row);
            max_column = max_column.max(column);
        }
        Ok(envelope.select(
            min_row,
            min_column,
            max_row - min_row + 1,
            max_column - min_column + 1,
        ))
    }

    fn index_selection(
        &self,
        arguments: &[Expr<usize>],
        rows: usize,
        columns: usize,
    ) -> Result<(usize, usize, usize, usize), CalcError> {
        if !matches!(arguments.len(), 2 | 3) {
            return Err(CalcError::InvalidArguments);
        }
        let index = |expression: &Expr<usize>| -> Result<usize, CalcError> {
            let value = number(self.evaluate(expression))?;
            if !value.is_finite() || value < 0.0 || value > usize::MAX as f64 {
                return Err(CalcError::InvalidReference);
            }
            Ok(value.trunc() as usize)
        };
        let mut row = index(&arguments[1])?;
        let column = if let Some(column) = arguments.get(2) {
            index(column)?
        } else if rows == 1 {
            let column = row;
            row = 1;
            column
        } else if columns == 1 {
            1
        } else {
            0
        };
        if row > rows || column > columns {
            return Err(CalcError::InvalidReference);
        }
        Ok((
            row.saturating_sub(1),
            column.saturating_sub(1),
            if row == 0 { rows } else { 1 },
            if column == 0 { columns } else { 1 },
        ))
    }

    pub(super) fn index_array(&self, arguments: &[Expr<usize>]) -> Result<ArrayValue, CalcError> {
        let Some(first) = arguments.first() else {
            return Err(CalcError::InvalidArguments);
        };
        let input = ArrayInput::new(first, self)?;
        let (input_rows, input_columns) = input.shape();
        let (row, column, rows, columns) =
            self.index_selection(arguments, input_rows, input_columns)?;
        let mut values = Vec::with_capacity(rows * columns);
        for r in row..row + rows {
            for c in column..column + columns {
                values.push(input.value(self, r * input_columns + c));
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

    fn cell(row: u32, column: u32) -> CellId {
        CellId::new(0, row, column)
    }

    #[test]
    fn index_references_supply_dynamic_endpoints_and_whole_axes() {
        let mut workbook = Workbook::default();
        for row in 0..3 {
            workbook.set_number(cell(row, 0), (row + 1) as f64 * 10.0);
            workbook.set_number(cell(row, 1), (row + 1) as f64);
        }
        workbook.set_number(cell(0, 3), 2.0);
        for (source, expected) in [
            ("=SUM(A1:INDEX(A1:A3,D1))", 30.0),
            ("=SUM(INDEX(A1:A3,2):INDEX(A1:A3,3))", 50.0),
            ("=SUM(INDEX(A1:B3,0,2))", 6.0),
            ("=SUM(INDEX(A1:B3,2,0))", 22.0),
            ("=SUM(INDEX(A1:B3,0,0))", 66.0),
            ("=SUM(INDEX(A1:B1,0))", 11.0),
            ("=MATCH(20,INDEX(A1:B3,0,1),0)", 2.0),
            ("=VLOOKUP(20,INDEX(A1:B3,0,0),2,FALSE)", 2.0),
            ("=SUM(A1:INDEX(INDEX(A1:A3,0),D1))", 30.0),
            ("=SUM(INDEX(A1:B3,0,1):INDEX(A1:B3,0,2))", 66.0),
            ("=SUM(INDEX({1,2;3,4},0,2))", 6.0),
            ("=SUM(INDEX({1,2;3,4},2,0))", 7.0),
            ("=SUM(INDEX(A1:A3,0)*B1:B3)", 140.0),
            ("=IFERROR(SUM(A1:INDEX(A1:A3,4)),17)", 17.0),
        ] {
            workbook.set_formula(cell(5, 6), source).unwrap();
            assert_eq!(
                workbook.value(cell(5, 6)),
                Value::Number(expected),
                "{source}"
            );
        }
        workbook.set_formula(cell(1, 2), "=INDEX(A1:A3,0)").unwrap();
        assert_eq!(workbook.value(cell(1, 2)), Value::Number(20.0));
        workbook
            .set_formula(cell(5, 6), "=SUM(A1:INDEX(A1:A3,D1))")
            .unwrap();
        workbook.set_number(cell(0, 3), 3.0);
        assert_eq!(workbook.value(cell(5, 6)), Value::Number(60.0));
        workbook.set_number(cell(2, 0), 40.0);
        assert_eq!(workbook.value(cell(5, 6)), Value::Number(70.0));
    }

    #[test]
    fn reference_envelope_rebinds_and_remains_sparse() {
        let parsed = ParsedFormula::parse("=SUM(A1:INDEX(A1:A3,2))", 0, &HashMap::new()).unwrap();
        let mapped = parsed.map_references(|id| CellId::new(id.sheet, 2 - id.row, id.column));
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 30.0);
        workbook.set_number(cell(1, 0), 20.0);
        workbook.set_number(cell(2, 0), 10.0);
        workbook.set_parsed_formula(cell(5, 1), mapped).unwrap();
        assert_eq!(workbook.value(cell(5, 1)), Value::Number(30.0));
        workbook.set_number(cell(2, 0), 50.0);
        assert_eq!(workbook.value(cell(5, 1)), Value::Number(70.0));

        let mut sparse = Workbook::default();
        sparse.set_number(cell(0, 0), 2.0);
        sparse.set_number(cell(499_999, 0), 3.0);
        sparse
            .set_formula(cell(0, 5), "=SUM(A1:INDEX(A1:A500000,500000))")
            .unwrap();
        assert_eq!(sparse.value(cell(0, 5)), Value::Number(5.0));
        assert!(sparse.statistics().dependency_edges < 20);
        sparse.set_number(cell(123_456, 0), 7.0);
        assert_eq!(sparse.value(cell(0, 5)), Value::Number(12.0));
        assert!(matches!(
            sparse.set_formula(cell(0, 0), "=SUM(A1:INDEX(A1:A3,2))"),
            Err(FormulaError::Cycle(_))
        ));
        assert_eq!(sparse.value(cell(0, 0)), Value::Number(2.0));
    }
}
