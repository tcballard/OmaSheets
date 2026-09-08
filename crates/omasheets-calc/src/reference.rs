//! Reference-valued selections retain a bounded, stable dependency envelope.
use super::*;

/// Narrow only provably constant axes, after stable bindings have been applied.
/// The persisted parsed shape remains unchanged, including legacy INDEX bindings.
pub(super) fn narrow_reference_dependencies(expression: Expr) -> Expr {
    match expression {
        Expr::UnaryMinus(inner) => {
            Expr::UnaryMinus(Box::new(narrow_reference_dependencies(*inner)))
        }
        Expr::Percent(inner) => Expr::Percent(Box::new(narrow_reference_dependencies(*inner))),
        Expr::Binary(op, left, right) => Expr::Binary(
            op,
            Box::new(narrow_reference_dependencies(*left)),
            Box::new(narrow_reference_dependencies(*right)),
        ),
        Expr::Function(function, arguments) => {
            let mut arguments: Vec<_> = arguments
                .into_iter()
                .map(narrow_reference_dependencies)
                .collect();
            if function == Function::Index && matches!(arguments.len(), 2 | 3) {
                narrow_index(&mut arguments);
            } else if function == Function::ReferenceSpan
                && arguments.len() == 3
                && matches!(arguments[2], Expr::Range { members: None, .. })
                && let (Some((a, b)), Some((c, d))) = (
                    reference_bounds(&arguments[0]),
                    reference_bounds(&arguments[1]),
                )
                && a.sheet == c.sheet
                && let Ok(envelope) = expand_range(
                    CellId::new(a.sheet, a.row.min(c.row), a.column.min(c.column)),
                    CellId::new(a.sheet, b.row.max(d.row), b.column.max(d.column)),
                )
            {
                arguments[2] = envelope;
            }
            Expr::Function(function, arguments)
        }
        other => other,
    }
}

fn narrow_index(arguments: &mut Vec<Expr>) {
    let Expr::Range {
        anchor,
        members,
        rows,
        columns,
    } = &arguments[0]
    else {
        return;
    };
    let (anchor, members, rows, columns) = (*anchor, members.clone(), *rows, *columns);
    let literal = |expression: &Expr, bound: usize| match expression {
        Expr::Number(value)
            if value.is_finite() && *value >= 1.0 && value.trunc() <= bound as f64 =>
        {
            Some(value.trunc() as usize - 1)
        }
        _ => None,
    };
    let horizontal = rows == 1 && arguments.len() == 2;
    let selected_row = if horizontal {
        None
    } else {
        literal(&arguments[1], rows)
    };
    let selected_column = if horizontal {
        literal(&arguments[1], columns)
    } else {
        arguments.get(2).and_then(|arg| literal(arg, columns))
    };
    if selected_row.is_none() && selected_column.is_none() {
        return;
    }
    // Make the original omitted-column semantics explicit before changing shape.
    if arguments.len() == 2 && !horizontal {
        arguments.push(Expr::Number(if columns == 1 { 1.0 } else { 0.0 }));
    }
    let first_row = selected_row.unwrap_or(0);
    let first_column = selected_column.unwrap_or(0);
    let new_rows = if selected_row.is_some() { 1 } else { rows };
    let new_columns = if selected_column.is_some() {
        1
    } else {
        columns
    };
    let (new_anchor, new_members) = if let Some(members) = members {
        let selected: Vec<_> = (first_row..first_row + new_rows)
            .flat_map(|row| {
                let members = &members;
                (first_column..first_column + new_columns)
                    .map(move |column| members[row * columns + column])
            })
            .collect();
        (selected[0], Some(selected))
    } else {
        (
            CellId::new(
                anchor.sheet,
                anchor.row + first_row as u32,
                anchor.column + first_column as u32,
            ),
            None,
        )
    };
    arguments[0] = Expr::Range {
        anchor: new_anchor,
        members: new_members,
        rows: new_rows,
        columns: new_columns,
    };
    if selected_row.is_some() || (horizontal && selected_column.is_some()) {
        arguments[1] = Expr::Number(1.0);
    }
    if !horizontal && selected_column.is_some() {
        arguments[2] = Expr::Number(1.0);
    }
}

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
    fn reference_endpoints_keep_function_selectors_on_the_origin_sheet() {
        let mut workbook = Workbook::default();
        workbook.define_sheet(0, "Data");
        workbook.define_sheet(1, "Report");
        for row in 0..3 {
            workbook.set_number(CellId::new(0, row, 0), (row + 1) as f64 * 10.0);
        }
        workbook.set_number(CellId::new(0, 0, 1), 99.0);
        workbook.set_number(CellId::new(1, 0, 1), 2.0);
        for formula in [
            "=SUM(Data!A1:INDEX(Data!A1:A3,B1))",
            "=SUM(INDEX(Data!A1:A3,1):INDEX(Data!A1:A3,B1))",
            "=SUM(Data!A1:(INDEX(Data!A1:A3,B1)))",
            "=SUM(Data!A1:A2)",
        ] {
            let target = CellId::new(1, 0, 2);
            workbook.set_formula(target, formula).unwrap();
            assert_eq!(workbook.value(target), Value::Number(30.0), "{formula}");
        }
    }

    #[test]
    fn constant_index_axes_do_not_create_false_cycles() {
        let mut workbook = Workbook::default();
        workbook.set_number(cell(0, 0), 10.0);
        workbook.set_number(cell(1, 0), 20.0);
        workbook.set_number(cell(2, 0), 30.0);
        workbook.set_number(cell(0, 3), 2.0);
        workbook
            .set_formula(cell(0, 1), "=SUM(A1:INDEX(A1:B3,D1,1))")
            .unwrap();
        assert_eq!(workbook.value(cell(0, 1)), Value::Number(30.0));
        workbook.set_number(cell(0, 3), 3.0);
        assert_eq!(workbook.value(cell(0, 1)), Value::Number(60.0));
        workbook.set_number(cell(2, 0), 40.0);
        assert_eq!(workbook.value(cell(0, 1)), Value::Number(70.0));
        for (formula, expected) in [
            ("=SUM(INDEX(A1:B3,2))", 20.0),
            ("=INDEX(A1:B1,1)", 10.0),
            ("=INDEX(A1:B3,2,1)", 20.0),
            ("=SUM(INDEX(A1:B3,0,1))", 70.0),
            ("=IFERROR(INDEX(A1:B3,4,1),99)", 99.0),
        ] {
            workbook.set_formula(cell(6, 6), formula).unwrap();
            assert_eq!(
                workbook.value(cell(6, 6)),
                Value::Number(expected),
                "{formula}"
            );
        }
        assert!(matches!(
            workbook.set_formula(cell(0, 0), "=INDEX(A1:B3,1,1)"),
            Err(FormulaError::Cycle(_))
        ));
        let parsed = ParsedFormula::parse("=INDEX(A1:B3,2,1)", 0, &HashMap::new()).unwrap();
        assert_eq!(parsed.reference_count(), 6);
        let mapped = parsed.map_references(|id| CellId::new(id.sheet, 2 - id.row, id.column));
        workbook.set_parsed_formula(cell(6, 6), mapped).unwrap();
        assert_eq!(workbook.value(cell(6, 6)), Value::Number(20.0));
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
