//! Replayable sheet presentation, attached to row/column identities.
use crate::{ApplyError, CellRef, ColumnId, Document, RowId, SheetId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    #[default]
    General,
    Left,
    Center,
    Right,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Border {
    #[default]
    None,
    All,
    Bottom,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub font_size: Option<f64>,
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub alignment: Alignment,
    pub wrap: bool,
    pub border: Border,
    pub number_format: String,
}

impl CellStyle {
    pub fn validate(&self) -> Result<(), ApplyError> {
        if self
            .font_size
            .is_some_and(|size| !size.is_finite() || !(6.0..=72.0).contains(&size))
            || self.number_format.len() > 128
            || self.number_format.chars().any(char::is_control)
            || [&self.foreground, &self.background]
                .into_iter()
                .flatten()
                .any(|color| {
                    color.len() != 7
                        || !color.starts_with('#')
                        || !color[1..].bytes().all(|c| c.is_ascii_hexdigit())
                })
        {
            return Err(invalid("Invalid cell style"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresentedCell {
    pub row: RowId,
    pub column: ColumnId,
    #[serde(default)]
    pub style: CellStyle,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub rows: Vec<RowId>,
    pub columns: Vec<ColumnId>,
}

impl Region {
    pub fn contains(&self, row: RowId, column: ColumnId) -> bool {
        self.rows.contains(&row) && self.columns.contains(&column)
    }

    pub fn validate(
        &self,
        rows: &[RowId],
        columns: &[ColumnId],
        limit: usize,
    ) -> Result<(), ApplyError> {
        if self
            .rows
            .len()
            .checked_mul(self.columns.len())
            .is_none_or(|count| count > limit)
            || !contiguous(&self.rows, rows)
            || !contiguous(&self.columns, columns)
        {
            return Err(invalid(
                "Presentation range must be a bounded rectangle in current view order",
            ));
        }
        Ok(())
    }
}

fn contiguous<T: PartialEq>(items: &[T], order: &[T]) -> bool {
    items
        .first()
        .and_then(|first| order.iter().position(|item| item == first))
        .and_then(|start| order.get(start..start + items.len()))
        .is_some_and(|slice| slice == items)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub range: Region,
    pub column: ColumnId,
    pub text: String,
    pub case_sensitive: bool,
    pub header: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Greater,
    Less,
    Equal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionalRule {
    pub range: Region,
    pub comparison: Comparison,
    pub value: f64,
    pub style: CellStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartKind {
    Bar,
    Line,
    Pie,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chart {
    pub id: String,
    pub title: String,
    pub kind: ChartKind,
    pub range: Region,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SheetPresentation {
    pub cells: Vec<PresentedCell>,
    pub row_heights: BTreeMap<RowId, f64>,
    pub column_widths: BTreeMap<ColumnId, f64>,
    pub merges: Vec<Region>,
    pub frozen_rows: usize,
    pub frozen_columns: usize,
    pub show_grid_lines: bool,
    pub filter: Option<Filter>,
    pub conditional: Vec<ConditionalRule>,
    pub charts: Vec<Chart>,
}

impl Default for SheetPresentation {
    fn default() -> Self {
        Self {
            cells: Vec::new(),
            row_heights: BTreeMap::new(),
            column_widths: BTreeMap::new(),
            merges: Vec::new(),
            frozen_rows: 0,
            frozen_columns: 0,
            show_grid_lines: true,
            filter: None,
            conditional: Vec::new(),
            charts: Vec::new(),
        }
    }
}

pub(crate) fn invalid(message: &str) -> ApplyError {
    ApplyError::InvalidPresentation(message.into())
}

impl SheetPresentation {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self, document: &Document, sheet: SheetId) -> Result<(), ApplyError> {
        let state = document.sheet(sheet)?;
        if self.is_default() {
            return Ok(());
        }
        self.validate_axes(&state.rows, &state.columns)?;
        for cell in document.cells_in_view(sheet) {
            if state
                .cells
                .get(&(cell.row, cell.column))
                .is_some_and(|state| {
                    !matches!(
                        state.input,
                        crate::CellInput::Value {
                            value: crate::Literal::Blank
                        }
                    )
                })
            {
                self.check_anchor(cell.row, cell.column)?;
            }
        }
        Ok(())
    }

    fn validate_axes(&self, rows: &[RowId], columns: &[ColumnId]) -> Result<(), ApplyError> {
        if self.cells.len() > 10_000
            || self.row_heights.len() > 10_000
            || self.column_widths.len() > 1_000
            || self.merges.len() > 1000
            || self.conditional.len() > 32
            || self.charts.len() > 16
            || self.frozen_rows > rows.len()
            || self.frozen_columns > columns.len()
            || serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > 2 * 1024 * 1024)
        {
            return Err(invalid("Sheet presentation exceeds its bounds"));
        }
        let mut seen = BTreeSet::new();
        for cell in &self.cells {
            cell.style.validate()?;
            if cell.note.len() > 8192
                || !rows.contains(&cell.row)
                || !columns.contains(&cell.column)
                || !seen.insert((cell.row, cell.column))
            {
                return Err(invalid("Invalid or repeated presentation cell"));
            }
        }
        if self.row_heights.iter().any(|(row, size)| {
            !rows.contains(row) || !size.is_finite() || !(16.0..=600.0).contains(size)
        }) || self.column_widths.iter().any(|(column, size)| {
            !columns.contains(column) || !size.is_finite() || !(24.0..=1200.0).contains(size)
        }) {
            return Err(invalid("Invalid row height or column width"));
        }
        for (index, region) in self.merges.iter().enumerate() {
            region.validate(rows, columns, 10_000)?;
            if self.merges[..index].iter().any(|other| {
                region.rows.iter().any(|row| other.rows.contains(row))
                    && region
                        .columns
                        .iter()
                        .any(|column| other.columns.contains(column))
            }) {
                return Err(invalid("Merged ranges cannot overlap"));
            }
        }
        if let Some(filter) = &self.filter {
            filter.range.validate(rows, columns, 100_000)?;
            if !filter.range.columns.contains(&filter.column) || filter.text.len() > 1024 {
                return Err(invalid("Invalid sheet filter"));
            }
        }
        for rule in &self.conditional {
            rule.range.validate(rows, columns, 100_000)?;
            rule.style.validate()?;
            if !rule.value.is_finite() {
                return Err(invalid("Conditional thresholds must be finite"));
            }
        }
        let mut identifiers = BTreeSet::new();
        for chart in &self.charts {
            chart.range.validate(rows, columns, 1000)?;
            if chart.range.rows.len() < 2
                || chart.range.columns.len() < 2
                || chart.range.columns.len() > 9
                || chart.id.is_empty()
                || chart.id.len() > 128
                || !identifiers.insert(&chart.id)
                || chart.title.trim().is_empty()
                || chart.title.len() > 255
            {
                return Err(invalid(
                    "A chart needs a unique name, header row, category column and 1–8 value columns",
                ));
            }
        }
        Ok(())
    }

    pub fn check_anchor(&self, row: RowId, column: ColumnId) -> Result<(), ApplyError> {
        if self.merges.iter().any(|range| {
            range.contains(row, column) && (row != range.rows[0] || column != range.columns[0])
        }) {
            return Err(invalid(
                "Only the top-left cell of a merged range can hold a value",
            ));
        }
        Ok(())
    }

    pub fn cell(&self, cell: CellRef) -> Option<&PresentedCell> {
        self.cells
            .iter()
            .find(|entry| entry.row == cell.row && entry.column == cell.column)
    }

    pub fn cell_mut(&mut self, cell: CellRef) -> &mut PresentedCell {
        if let Some(index) = self
            .cells
            .iter()
            .position(|entry| entry.row == cell.row && entry.column == cell.column)
        {
            return &mut self.cells[index];
        }
        self.cells.push(PresentedCell {
            row: cell.row,
            column: cell.column,
            style: CellStyle::default(),
            note: String::new(),
        });
        self.cells.last_mut().expect("inserted")
    }

    pub fn remap(&mut self, rows: &BTreeMap<RowId, RowId>, columns: &BTreeMap<ColumnId, ColumnId>) {
        for cell in &mut self.cells {
            cell.row = rows[&cell.row];
            cell.column = columns[&cell.column];
        }
        self.row_heights = self
            .row_heights
            .iter()
            .map(|(id, size)| (rows[id], *size))
            .collect();
        self.column_widths = self
            .column_widths
            .iter()
            .map(|(id, size)| (columns[id], *size))
            .collect();
        for region in self.regions_mut() {
            for row in &mut region.rows {
                *row = rows[row];
            }
            for column in &mut region.columns {
                *column = columns[column];
            }
        }
        if let Some(filter) = &mut self.filter {
            filter.column = columns[&filter.column];
        }
        self.canonicalize();
    }

    pub fn canonicalize(&mut self) {
        self.cells
            .retain(|cell| cell.style != CellStyle::default() || !cell.note.is_empty());
        self.cells.sort_by_key(|cell| (cell.row, cell.column));
    }

    fn regions_mut(&mut self) -> impl Iterator<Item = &mut Region> {
        self.merges
            .iter_mut()
            .chain(self.filter.iter_mut().map(|filter| &mut filter.range))
            .chain(self.conditional.iter_mut().map(|rule| &mut rule.range))
            .chain(self.charts.iter_mut().map(|chart| &mut chart.range))
    }

    /// Keep existing identities and include insertions inside each rectangular range.
    pub(crate) fn inserted(
        &mut self,
        old_rows: &[RowId],
        old_columns: &[ColumnId],
        rows: &[RowId],
        columns: &[ColumnId],
    ) -> Result<(), ApplyError> {
        if self.is_default() {
            return Ok(());
        }
        self.frozen_rows = retained_freeze(self.frozen_rows, old_rows, rows);
        self.frozen_columns = retained_freeze(self.frozen_columns, old_columns, columns);
        for region in self.regions_mut() {
            expand(&mut region.rows, rows);
            expand(&mut region.columns, columns);
        }
        self.validate_axes(rows, columns)
    }

    pub(crate) fn deleted(
        &mut self,
        old_rows: &[RowId],
        old_columns: &[ColumnId],
        rows: &[RowId],
        columns: &[ColumnId],
    ) {
        if self.is_default() {
            return;
        }
        self.frozen_rows = old_rows
            .iter()
            .take(self.frozen_rows)
            .filter(|row| rows.contains(row))
            .count();
        self.frozen_columns = old_columns
            .iter()
            .take(self.frozen_columns)
            .filter(|column| columns.contains(column))
            .count();
        self.cells
            .retain(|cell| rows.contains(&cell.row) && columns.contains(&cell.column));
        self.row_heights.retain(|row, _| rows.contains(row));
        self.column_widths
            .retain(|column, _| columns.contains(column));
        let intact = |region: &Region| {
            region.rows.iter().all(|row| rows.contains(row))
                && region.columns.iter().all(|column| columns.contains(column))
        };
        self.merges.retain(intact);
        if self
            .filter
            .as_ref()
            .is_some_and(|filter| !intact(&filter.range))
        {
            self.filter = None;
        }
        self.conditional.retain(|rule| intact(&rule.range));
        self.charts.retain(|chart| intact(&chart.range));
    }

    pub(crate) fn reordered(
        &mut self,
        rows: &[RowId],
        columns: &[ColumnId],
    ) -> Result<(), ApplyError> {
        if self.is_default() {
            return Ok(());
        }
        for region in self.regions_mut() {
            region
                .rows
                .sort_by_key(|row| rows.iter().position(|id| id == row));
        }
        self.validate_axes(rows, columns)
    }
}

fn expand<T: Clone + PartialEq>(items: &mut Vec<T>, order: &[T]) {
    if let (Some(start), Some(end)) = (
        items
            .first()
            .and_then(|id| order.iter().position(|item| item == id)),
        items
            .last()
            .and_then(|id| order.iter().position(|item| item == id)),
    ) {
        *items = order[start..=end].to_vec();
    }
}

fn retained_freeze<T: PartialEq>(count: usize, old: &[T], current: &[T]) -> usize {
    count
        .checked_sub(1)
        .and_then(|last| old.get(last))
        .and_then(|id| current.iter().position(|item| item == id))
        .map_or(0, |index| index + 1)
}
