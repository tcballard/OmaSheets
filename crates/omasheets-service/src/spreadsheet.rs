//! Atomic human spreadsheet controls, projected through stable identities.
use crate::{Service, ServiceError, revision};
use omasheets_core::presentation::{
    CellStyle, Chart, ChartKind, Comparison, ConditionalRule, Filter, Region,
};
use omasheets_core::{
    Actor, ActorKind, CellInput, CellRef, CellValue, Command, Document, Literal, Operation, SheetId,
};
use omasheets_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub row: usize,
    pub column: usize,
    pub rows: usize,
    pub columns: usize,
}

impl Rect {
    pub fn region(&self, document: &Document, sheet: SheetId) -> Result<Region, ServiceError> {
        let rows = document
            .rows(sheet)
            .ok_or_else(|| invalid("Unknown sheet"))?;
        let columns = document
            .columns(sheet)
            .ok_or_else(|| invalid("Unknown sheet"))?;
        if self.rows == 0
            || self.columns == 0
            || self
                .rows
                .checked_mul(self.columns)
                .is_none_or(|count| count > 10_000)
            || self
                .row
                .checked_add(self.rows)
                .is_none_or(|end| end > rows.len())
            || self
                .column
                .checked_add(self.columns)
                .is_none_or(|end| end > columns.len())
        {
            return Err(invalid(
                "Select a rectangle inside the sheet containing at most 10,000 cells",
            ));
        }
        Ok(Region {
            rows: rows[self.row..self.row + self.rows].to_vec(),
            columns: columns[self.column..self.column + self.columns].to_vec(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    SetCells {
        row: usize,
        column: usize,
        values: Vec<Vec<String>>,
    },
    Format {
        range: Rect,
        patch: Value,
    },
    ClearFormat {
        range: Rect,
    },
    Note {
        row: usize,
        column: usize,
        text: String,
    },
    Dimensions {
        range: Rect,
        width: Option<f64>,
        height: Option<f64>,
        #[serde(default)]
        autofit: bool,
    },
    Freeze {
        rows: usize,
        columns: usize,
    },
    GridLines {
        visible: bool,
    },
    Merge {
        range: Rect,
        #[serde(default)]
        unmerge: bool,
    },
    Sort {
        range: Rect,
        column: usize,
        #[serde(default)]
        descending: bool,
        #[serde(default)]
        header: bool,
    },
    Filter {
        range: Rect,
        column: usize,
        text: String,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        header: bool,
    },
    ClearFilter,
    Deduplicate {
        range: Rect,
        #[serde(default)]
        header: bool,
    },
    Conditional {
        range: Rect,
        comparison: Comparison,
        value: f64,
        patch: Value,
    },
    ClearConditional,
    Chart {
        range: Rect,
        title: String,
        kind: ChartKind,
    },
    RemoveChart {
        id: String,
    },
    AddSheet {
        name: String,
    },
    RenameSheet {
        name: String,
    },
    DuplicateSheet {
        name: String,
    },
    DeleteSheet,
    InsertRows {
        at: usize,
        count: usize,
    },
    DeleteRows {
        at: usize,
        count: usize,
    },
    InsertColumns {
        at: usize,
        count: usize,
    },
    DeleteColumns {
        at: usize,
        count: usize,
    },
    Replace {
        range: Rect,
        find: String,
        replacement: String,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        whole_cell: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditResult {
    pub revision: String,
    pub undo: Vec<Command>,
    pub redo: Vec<Command>,
    pub structural: bool,
    pub message: String,
}

fn invalid(message: &str) -> ServiceError {
    ServiceError::new("invalid_sheet_action", message)
}
fn human() -> Actor {
    Actor::new(ActorKind::Human, "omasheets-desktop")
}
fn refs(sheet: SheetId, range: &Region) -> impl Iterator<Item = CellRef> + '_ {
    range.rows.iter().flat_map(move |row| {
        range.columns.iter().map(move |column| CellRef {
            sheet,
            row: *row,
            column: *column,
        })
    })
}

fn patch_style(style: &CellStyle, patch: &Value) -> Result<CellStyle, ServiceError> {
    let mut value = serde_json::to_value(style).map_err(|_| invalid("Invalid style"))?;
    let object = patch
        .as_object()
        .ok_or_else(|| invalid("A style patch must be an object"))?;
    for (key, field) in object {
        value[key] = field.clone();
    }
    let style: CellStyle =
        serde_json::from_value(value).map_err(|_| invalid("Invalid or unknown style property"))?;
    style.validate()?;
    Ok(style)
}

fn input_command(document: &Document, cell: CellRef) -> Result<Command, ServiceError> {
    let a1 = document
        .project_a1(cell)
        .ok_or_else(|| invalid("Cell is outside the current view"))?;
    Ok(match document.cell(cell).map(|state| &state.input) {
        Some(CellInput::Value { value }) => Command::SetValue {
            sheet: cell.sheet,
            a1,
            value: value.clone(),
        },
        Some(CellInput::Formula { formula }) => {
            let current = document.compile_formula(cell.sheet, &formula.source)?;
            if current.references() != formula.references()
                || current.sheet_bindings != formula.sheet_bindings
            {
                return Err(invalid(
                    "A formula's stable references have moved; re-enter that formula before duplicating it",
                ));
            }
            Command::SetFormula {
                sheet: cell.sheet,
                a1,
                source: formula.source.clone(),
            }
        }
        None => Command::ClearCell {
            sheet: cell.sheet,
            a1,
        },
    })
}

pub fn edit(
    store: &mut Store,
    sheet_name: &str,
    expected: &str,
    now: i64,
    action: Action,
) -> Result<EditResult, ServiceError> {
    let main = store.branch_id("main")?;
    let document = store.document(main)?;
    if revision(document) != expected {
        return Err(ServiceError::new(
            "document_changed",
            "The workbook changed; reopen it before applying this action",
        ));
    }
    let sheet = Service::sheet(document, sheet_name)?;
    let before = document.presentation(sheet)?.clone();
    let mut presentation = before.clone();
    let mut commands = Vec::new();
    let mut undo = Vec::new();
    let mut structural = false;
    let mut message = "Saved locally — Ctrl+Z to undo".to_string();
    let duplicate = matches!(&action, Action::DuplicateSheet { .. });
    match action {
        Action::SetCells {
            row,
            column,
            values,
        } => {
            let width = values.first().map_or(0, Vec::len);
            if width == 0
                || values
                    .len()
                    .checked_mul(width)
                    .is_none_or(|count| count > 1000)
                || values.iter().any(|line| line.len() != width)
            {
                return Err(invalid(
                    "Edit a rectangular matrix containing at most 1,000 cells",
                ));
            }
            let region = Rect {
                row,
                column,
                rows: values.len(),
                columns: width,
            }
            .region(document, sheet)?;
            for (cell, text) in refs(sheet, &region).zip(values.into_iter().flatten()) {
                undo.push(Command::RestoreCell {
                    cell,
                    input: document.cell(cell).map(|state| state.input.clone()),
                });
                let a1 = document.project_a1(cell).expect("cell");
                let command = if text.is_empty() {
                    Command::ClearCell { sheet, a1 }
                } else if text.starts_with('=') {
                    Command::SetFormula {
                        sheet,
                        a1,
                        source: text,
                    }
                } else {
                    let value = if let Some(text) = text.strip_prefix('\'') {
                        Literal::Text(text.into())
                    } else if text.eq_ignore_ascii_case("true") {
                        Literal::Boolean(true)
                    } else if text.eq_ignore_ascii_case("false") {
                        Literal::Boolean(false)
                    } else if let Ok(value) = text.parse::<f64>()
                        && value.is_finite()
                    {
                        Literal::Number(value)
                    } else {
                        Literal::Text(text)
                    };
                    Command::SetValue { sheet, a1, value }
                };
                commands.push(command);
            }
        }

        Action::Format { range, patch } => {
            for cell in refs(sheet, &range.region(document, sheet)?) {
                let entry = presentation.cell_mut(cell);
                entry.style = patch_style(&entry.style, &patch)?;
            }
        }
        Action::ClearFormat { range } => {
            for cell in refs(sheet, &range.region(document, sheet)?) {
                presentation.cell_mut(cell).style = CellStyle::default();
            }
        }
        Action::Note { row, column, text } => {
            let range = Rect {
                row,
                column,
                rows: 1,
                columns: 1,
            }
            .region(document, sheet)?;
            presentation
                .cell_mut(refs(sheet, &range).next().expect("nonempty"))
                .note = text;
        }
        Action::Dimensions {
            range,
            width,
            height,
            autofit,
        } => {
            let range = range.region(document, sheet)?;
            for row in &range.rows {
                if let Some(height) = height {
                    presentation.row_heights.insert(*row, height);
                }
            }
            for column in &range.columns {
                if autofit {
                    let mut longest = 3;
                    let mut count = 0;
                    for cell in document.cells_in_view(sheet) {
                        if cell.column != *column {
                            continue;
                        }
                        count += 1;
                        if count > 10_000 {
                            return Err(invalid(
                                "Autofit is limited to 10,000 occupied cells per column",
                            ));
                        }
                        longest = longest.max(
                            display(document.value(cell), &effective_style(document, cell))
                                .chars()
                                .count(),
                        );
                    }
                    presentation
                        .column_widths
                        .insert(*column, (longest as f64 * 8.0 + 20.0).clamp(40.0, 600.0));
                } else if let Some(width) = width {
                    presentation.column_widths.insert(*column, width);
                }
            }
        }
        Action::Freeze { rows, columns } => {
            presentation.frozen_rows = rows;
            presentation.frozen_columns = columns;
        }
        Action::GridLines { visible } => presentation.show_grid_lines = visible,
        Action::Merge { range, unmerge } => {
            let range = range.region(document, sheet)?;
            if unmerge {
                presentation.merges.retain(|merge| {
                    !range.rows.iter().any(|row| merge.rows.contains(row))
                        || !range
                            .columns
                            .iter()
                            .any(|column| merge.columns.contains(column))
                });
            } else {
                presentation.merges.push(range);
            }
        }
        Action::Sort {
            range,
            column,
            descending,
            header,
        } => {
            let region = range.region(document, sheet)?;
            if !(range.column..range.column + range.columns).contains(&column) {
                return Err(invalid("Choose a sort column inside the selection"));
            }
            if !presentation.merges.is_empty() {
                return Err(invalid("Unmerge cells before sorting rows"));
            }
            let key_column = document.columns(sheet).expect("sheet")[column];
            let mut rows = document.rows(sheet).expect("sheet").to_vec();
            let mut sorted = region.rows[usize::from(header)..].to_vec();
            sorted.sort_by(|left, right| {
                let order = compare(
                    &document.value(CellRef {
                        sheet,
                        row: *left,
                        column: key_column,
                    }),
                    &document.value(CellRef {
                        sheet,
                        row: *right,
                        column: key_column,
                    }),
                );
                if descending { order.reverse() } else { order }
            });
            rows.splice(
                range.row + usize::from(header)..range.row + range.rows,
                sorted,
            );
            undo.push(Command::ReorderRows {
                sheet,
                rows: document.rows(sheet).expect("sheet").to_vec(),
            });
            commands.push(Command::ReorderRows { sheet, rows });
            message = "Entire rows sorted; formulas and formatting follow their identities — Ctrl+Z to undo".into();
        }
        Action::Filter {
            range,
            column,
            text,
            case_sensitive,
            header,
        } => {
            let region = range.region(document, sheet)?;
            let column = *document
                .columns(sheet)
                .and_then(|columns| columns.get(column))
                .ok_or_else(|| invalid("Invalid filter column"))?;
            presentation.filter = Some(Filter {
                range: region,
                column,
                text,
                case_sensitive,
                header,
            });
        }
        Action::ClearFilter => presentation.filter = None,
        Action::Deduplicate { range, header } => {
            let region = range.region(document, sheet)?;
            let mut seen = BTreeSet::new();
            let mut doomed = Vec::new();
            for row in region.rows.iter().skip(usize::from(header)) {
                let values: Vec<_> = region
                    .columns
                    .iter()
                    .map(|column| {
                        document.value(CellRef {
                            sheet,
                            row: *row,
                            column: *column,
                        })
                    })
                    .collect();
                let key =
                    serde_json::to_string(&values).map_err(|_| invalid("Invalid cell value"))?;
                if !seen.insert(key) {
                    doomed.push(*row);
                }
            }
            if !doomed.is_empty() {
                commands.push(Command::DeleteRows {
                    sheet,
                    rows: doomed,
                });
                structural = true;
            }
            message = "Duplicate rows removed. Structural changes start a new undo history.".into();
        }
        Action::Conditional {
            range,
            comparison,
            value,
            patch,
        } => presentation.conditional.push(ConditionalRule {
            range: range.region(document, sheet)?,
            comparison,
            value,
            style: patch_style(&CellStyle::default(), &patch)?,
        }),
        Action::ClearConditional => presentation.conditional.clear(),
        Action::Chart { range, title, kind } => {
            let id = omasheets_core::ObjectId::from_seed(&format!("{expected}:{now}:{title}"))
                .to_string();
            presentation.charts.push(Chart {
                id,
                title,
                kind,
                range: range.region(document, sheet)?,
            });
        }
        Action::RemoveChart { id } => presentation.charts.retain(|chart| chart.id != id),
        Action::RenameSheet { name } => {
            undo.push(Command::RenameSheet {
                sheet,
                name: document.sheet_name(sheet).expect("sheet").into(),
            });
            commands.push(Command::RenameSheet { sheet, name });
            structural = true;
        }
        Action::AddSheet { name } | Action::DuplicateSheet { name } => {
            let mut staged = document.clone();
            let add = Command::AddSheet { name };
            let event = staged.command(human(), now, add.clone())?;
            let Operation::AddSheet {
                sheet: destination, ..
            } = event.operation
            else {
                unreachable!()
            };
            commands.push(add);
            let (rows, columns) = if duplicate {
                (
                    document.rows(sheet).expect("sheet").len(),
                    document.columns(sheet).expect("sheet").len(),
                )
            } else {
                (1000, 26)
            };
            if rows == 0
                || columns == 0
                || rows > 10_000
                || columns > 1000
                || (duplicate && document.cell_count(sheet) > 900)
            {
                return Err(invalid(
                    "Sheet duplication supports up to 900 occupied cells, 10,000 rows and 1,000 columns",
                ));
            }
            for command in [
                Command::AddColumns {
                    sheet: destination,
                    count: columns,
                    at: 0,
                },
                Command::AddRows {
                    sheet: destination,
                    count: rows,
                    at: 0,
                    table: None,
                },
            ] {
                staged.command(human(), now, command.clone())?;
                commands.push(command);
            }
            if duplicate {
                if document.tables().values().any(|table| table.sheet == sheet) {
                    return Err(invalid(
                        "Duplicate sheets containing native tables through an explicit copy of their cells",
                    ));
                }
                for cell in document.cells_in_view(sheet) {
                    let command = match input_command(document, cell)? {
                        Command::SetValue { a1, value, .. } => Command::SetValue {
                            sheet: destination,
                            a1,
                            value,
                        },
                        Command::SetFormula { a1, source, .. } => Command::SetFormula {
                            sheet: destination,
                            a1,
                            source,
                        },
                        Command::ClearCell { a1, .. } => Command::ClearCell {
                            sheet: destination,
                            a1,
                        },
                        _ => unreachable!(),
                    };
                    commands.push(command);
                }
                let row_map: BTreeMap<_, _> = document
                    .rows(sheet)
                    .expect("sheet")
                    .iter()
                    .zip(staged.rows(destination).expect("sheet"))
                    .map(|(a, b)| (*a, *b))
                    .collect();
                let column_map: BTreeMap<_, _> = document
                    .columns(sheet)
                    .expect("sheet")
                    .iter()
                    .zip(staged.columns(destination).expect("sheet"))
                    .map(|(a, b)| (*a, *b))
                    .collect();
                let mut copied = before.clone();
                copied.remap(&row_map, &column_map);
                commands.push(Command::SetPresentation {
                    sheet: destination,
                    presentation: copied,
                });
                for check in document
                    .checks()
                    .values()
                    .filter(|check| check.cell.sheet == sheet)
                {
                    commands.push(Command::AddCheck {
                        sheet: destination,
                        a1: document.project_a1(check.cell).expect("cell"),
                        name: check.name.clone(),
                        severity: check.severity,
                        message: check.message.clone(),
                    });
                }
                for watch in document
                    .watches()
                    .values()
                    .filter(|watch| watch.cell.sheet == sheet)
                {
                    commands.push(Command::WatchOutput {
                        sheet: destination,
                        a1: document.project_a1(watch.cell).expect("cell"),
                        name: watch.name.clone(),
                    });
                }
            }
            structural = true;
        }
        Action::DeleteSheet => {
            if document.sheets().len() <= 1 {
                return Err(invalid("Keep at least one sheet in the workbook"));
            }
            commands.push(Command::DeleteSheet { sheet });
            structural = true;
        }
        Action::InsertRows { at, count } => {
            commands.push(Command::AddRows {
                sheet,
                at,
                count,
                table: None,
            });
            structural = true;
        }
        Action::InsertColumns { at, count } => {
            commands.push(Command::AddColumns { sheet, at, count });
            structural = true;
        }
        Action::DeleteRows { at, count } => {
            let rows = document.rows(sheet).expect("sheet");
            if count == 0
                || count >= rows.len()
                || at.checked_add(count).is_none_or(|end| end > rows.len())
            {
                return Err(invalid(
                    "Delete a valid selection while keeping at least one row",
                ));
            }
            commands.push(Command::DeleteRows {
                sheet,
                rows: rows[at..at + count].to_vec(),
            });
            structural = true;
        }
        Action::DeleteColumns { at, count } => {
            let columns = document.columns(sheet).expect("sheet");
            if count == 0
                || count >= columns.len()
                || at.checked_add(count).is_none_or(|end| end > columns.len())
            {
                return Err(invalid(
                    "Delete a valid selection while keeping at least one column",
                ));
            }
            commands.push(Command::DeleteColumns {
                sheet,
                columns: columns[at..at + count].to_vec(),
            });
            structural = true;
        }
        Action::Replace {
            range,
            find,
            replacement,
            case_sensitive,
            whole_cell,
        } => {
            if find.is_empty() || find.len() > 1024 || replacement.len() > 8192 {
                return Err(invalid("Enter bounded find and replacement text"));
            }
            for cell in refs(sheet, &range.region(document, sheet)?) {
                let Some(CellInput::Value {
                    value: Literal::Text(text),
                }) = document.cell(cell).map(|state| &state.input)
                else {
                    continue;
                };
                let next = replace_text(text, &find, &replacement, case_sensitive, whole_cell);
                if next != *text {
                    undo.push(input_command(document, cell)?);
                    commands.push(Command::SetValue {
                        sheet,
                        a1: document.project_a1(cell).expect("cell"),
                        value: Literal::Text(next),
                    });
                }
            }
            message = format!("Replaced text in {} cells — Ctrl+Z to undo", commands.len());
        }
    }
    presentation.canonicalize();
    if presentation != before {
        presentation.validate(document, sheet)?;
        undo.push(Command::SetPresentation {
            sheet,
            presentation: before,
        });
        commands.push(Command::SetPresentation {
            sheet,
            presentation,
        });
    }
    if commands.len() > 1000 {
        return Err(invalid("This action exceeds 1,000 atomic commands"));
    }
    if serde_json::to_vec(&(&commands, &undo)).map_or(true, |bytes| bytes.len() > 3 * 1024 * 1024) {
        return Err(invalid("This action exceeds the bounded undo payload"));
    }
    if !commands.is_empty() {
        store.append_batch(main, human(), now, commands.clone())?;
    }
    if structural {
        undo.clear();
        message = "Saved locally. Structural changes start a new undo history.".into();
    }
    Ok(EditResult {
        revision: revision(store.document(main)?),
        undo,
        redo: commands,
        structural,
        message,
    })
}

fn compare(left: &CellValue, right: &CellValue) -> std::cmp::Ordering {
    match (left, right) {
        (CellValue::Number(a), CellValue::Number(b)) => a.total_cmp(b),
        (CellValue::Blank, CellValue::Blank) => std::cmp::Ordering::Equal,
        (CellValue::Blank, _) => std::cmp::Ordering::Greater,
        (_, CellValue::Blank) => std::cmp::Ordering::Less,
        _ => plain(left).to_lowercase().cmp(&plain(right).to_lowercase()),
    }
}

pub fn plain(value: &CellValue) -> String {
    match value {
        CellValue::Blank => String::new(),
        CellValue::Number(value) => value.to_string(),
        CellValue::Text(value) | CellValue::Error(value) => value.clone(),
        CellValue::Boolean(value) => if *value { "TRUE" } else { "FALSE" }.into(),
    }
}

fn replace_text(
    text: &str,
    find: &str,
    replacement: &str,
    case_sensitive: bool,
    whole_cell: bool,
) -> String {
    if whole_cell {
        return if matches_text(text, find, case_sensitive, true) {
            replacement.into()
        } else {
            text.into()
        };
    }
    if case_sensitive {
        return text.replace(find, replacement);
    }
    // Map lowercase byte boundaries back to the original Unicode text, including expanding lowercase mappings.
    let mut folded = String::new();
    let mut boundaries = BTreeMap::new();
    for (offset, ch) in text.char_indices() {
        boundaries.insert(folded.len(), offset);
        folded.extend(ch.to_lowercase());
    }
    boundaries.insert(folded.len(), text.len());
    let needle = find.to_lowercase();
    let mut output = String::new();
    let mut cursor = 0;
    for (start, _) in folded.match_indices(&needle) {
        if let (Some(&a), Some(&b)) = (
            boundaries.get(&start),
            boundaries.get(&(start + needle.len())),
        ) {
            if a >= cursor {
                output.push_str(&text[cursor..a]);
                output.push_str(replacement);
                cursor = b;
            }
        }
    }
    output.push_str(&text[cursor..]);
    output
}

fn matches_text(text: &str, find: &str, case_sensitive: bool, whole: bool) -> bool {
    let (text, find) = if case_sensitive {
        (text.into(), find.into())
    } else {
        (text.to_lowercase(), find.to_lowercase())
    };
    if whole {
        text == find
    } else {
        text.contains(&find)
    }
}

pub fn effective_style(document: &Document, cell: CellRef) -> CellStyle {
    let Ok(presentation) = document.presentation(cell.sheet) else {
        return CellStyle::default();
    };
    let mut style = presentation
        .cell(cell)
        .map(|entry| entry.style.clone())
        .unwrap_or_default();
    let CellValue::Number(value) = document.value(cell) else {
        return style;
    };
    for rule in &presentation.conditional {
        let matches = match rule.comparison {
            Comparison::Greater => value > rule.value,
            Comparison::Less => value < rule.value,
            Comparison::Equal => value == rule.value,
        };
        if rule.range.contains(cell.row, cell.column) && matches {
            if rule.style.background.is_some() {
                style.background = rule.style.background.clone();
            }
            if rule.style.foreground.is_some() {
                style.foreground = rule.style.foreground.clone();
            }
            style.bold |= rule.style.bold;
            style.italic |= rule.style.italic;
            style.underline |= rule.style.underline;
        }
    }
    style
}

pub fn display(value: CellValue, style: &CellStyle) -> String {
    let CellValue::Number(number) = value else {
        return plain(&value);
    };
    let format = style.number_format.as_str();
    if matches!(
        format,
        "yyyy-mm-dd" | "dd/mm/yyyy" | "mm/dd/yyyy" | "m/d/yy"
    ) && let Ok(serial) = omasheets_calc::serial_date::serial_from_number(number)
        && let Ok(date) = omasheets_calc::serial_date::civil_from_serial(serial)
    {
        return match format {
            "yyyy-mm-dd" => format!("{:04}-{:02}-{:02}", date.year, date.month, date.day),
            "dd/mm/yyyy" => format!("{:02}/{:02}/{:04}", date.day, date.month, date.year),
            "m/d/yy" => format!(
                "{}/{}/{:02}",
                date.month,
                date.day,
                date.year.rem_euclid(100)
            ),
            _ => format!("{:02}/{:02}/{:04}", date.month, date.day, date.year),
        };
    }
    let percentage = format.ends_with('%');
    let core = format
        .trim_start_matches(['$', '£', '€'])
        .trim_end_matches('%');
    let decimals = match core {
        "0" | "#,##0" => Some(0),
        "0.0" | "#,##0.0" => Some(1),
        "0.00" | "#,##0.00" => Some(2),
        "0.000" | "#,##0.000" => Some(3),
        _ => None,
    };
    if let Some(decimals) = decimals {
        let value = if percentage { number * 100.0 } else { number };
        if !value.is_finite() {
            return plain(&CellValue::Number(number));
        }
        let mut text = format!("{value:.decimals$}");
        if core.starts_with("#,##") {
            let end = text.find('.').unwrap_or(text.len());
            let start = usize::from(text.starts_with('-'));
            let mut position = end.saturating_sub(3);
            while position > start {
                text.insert(position, ',');
                position = position.saturating_sub(3);
            }
        }
        if percentage {
            text.push('%');
        }
        if let Some(currency) = format
            .chars()
            .next()
            .filter(|ch| ['$', '£', '€'].contains(ch))
        {
            text.insert(0, currency);
        }
        return text;
    }
    plain(&CellValue::Number(number))
}

pub fn inspect(
    document: &Document,
    sheet: SheetId,
    range: Rect,
    find: Option<&str>,
) -> Result<Value, ServiceError> {
    let region = range.region(document, sheet)?;
    let mut count = 0;
    let mut numbers = Vec::new();
    let mut matches = Vec::new();
    let mut truncated = false;
    for cell in refs(sheet, &region) {
        let value = document.value(cell);
        if value != CellValue::Blank {
            count += 1;
        }
        if let CellValue::Number(value) = value {
            numbers.push(value);
        }
        if find.is_some_and(|find| {
            !find.is_empty() && matches_text(&plain(&value), find, false, false)
        }) {
            if matches.len() < 200 {
                matches.push(json!({"a1":document.project_a1(cell),"row":document.rows(sheet).expect("sheet").iter().position(|row| *row==cell.row),"column":document.columns(sheet).expect("sheet").iter().position(|column| *column==cell.column),"value":value}));
            } else {
                truncated = true;
            }
        }
    }
    let sum: f64 = numbers.iter().sum();
    let average = sum / numbers.len() as f64;
    Ok(
        json!({"count":count,"numeric_count":numbers.len(),"sum":sum.is_finite().then_some(sum),"average":average.is_finite().then_some(average),
        "min":numbers.iter().copied().reduce(f64::min),"max":numbers.iter().copied().reduce(f64::max),"matches":matches,"truncated":truncated}),
    )
}

pub fn find(document: &Document, sheet: SheetId, query: &str) -> Result<Value, ServiceError> {
    if query.is_empty() || query.len() > 1024 {
        return Err(invalid("Enter search text within 1,024 bytes"));
    }
    let mut matches = Vec::new();
    let mut truncated = false;
    for (index, cell) in document.cells_in_view(sheet).enumerate() {
        if index >= 100_000 {
            truncated = true;
            break;
        }
        let value = document.value(cell);
        let formula = match document.cell(cell).map(|state| &state.input) {
            Some(CellInput::Formula { formula }) => formula.source.as_str(),
            _ => "",
        };
        if matches_text(&plain(&value), query, false, false)
            || matches_text(formula, query, false, false)
        {
            if matches.len() == 200 {
                truncated = true;
                break;
            }
            matches.push(json!({"a1":document.project_a1(cell),"row":document.rows(sheet).expect("sheet").iter().position(|row| *row==cell.row),"column":document.columns(sheet).expect("sheet").iter().position(|column| *column==cell.column),"value":value}));
        }
    }
    Ok(json!({"matches":matches,"truncated":truncated}))
}

pub fn view(document: &Document, sheet: SheetId) -> Result<Value, ServiceError> {
    let presentation = document.presentation(sheet)?;
    let rows = document.rows(sheet).expect("sheet");
    let columns = document.columns(sheet).expect("sheet");
    let row_positions: BTreeMap<_, _> = rows
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index))
        .collect();
    let column_positions: BTreeMap<_, _> = columns
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index))
        .collect();
    let rect = |range: &Region| json!({"row":row_positions[&range.rows[0]],"column":column_positions[&range.columns[0]],"rows":range.rows.len(),"columns":range.columns.len()});
    let mut hidden = Vec::new();
    if let Some(filter) = &presentation.filter {
        for row in filter.range.rows.iter().skip(usize::from(filter.header)) {
            if !matches_text(
                &plain(&document.value(CellRef {
                    sheet,
                    row: *row,
                    column: filter.column,
                })),
                &filter.text,
                filter.case_sensitive,
                false,
            ) {
                hidden.push(row_positions[row]);
            }
        }
    }
    let charts:Vec<_>=presentation.charts.iter().map(|chart| {
        let categories:Vec<_>=chart.range.rows.iter().skip(1).map(|row| plain(&document.value(CellRef {sheet,row:*row,column:chart.range.columns[0]}))).collect();
        let series:Vec<_>=chart.range.columns.iter().skip(1).map(|column| {
            let values:Vec<_>=chart.range.rows.iter().skip(1).map(|row| match document.value(CellRef {sheet,row:*row,column:*column}) {CellValue::Number(value)=>Some(value),_=>None}).collect();
            json!({"name":plain(&document.value(CellRef {sheet,row:chart.range.rows[0],column:*column})),"values":values})
        }).collect();
        json!({"id":chart.id,"title":chart.title,"kind":chart.kind,"range":rect(&chart.range),"categories":categories,"series":series})
    }).collect();
    Ok(
        json!({"row_heights":presentation.row_heights.iter().map(|(row,height)|json!([row_positions[row],height])).collect::<Vec<_>>(),
        "column_widths":presentation.column_widths.iter().map(|(column,width)|json!([column_positions[column],width])).collect::<Vec<_>>(),
        "merges":presentation.merges.iter().map(rect).collect::<Vec<_>>(),"frozen_rows":presentation.frozen_rows,"frozen_columns":presentation.frozen_columns,
        "show_grid_lines":presentation.show_grid_lines,"hidden_rows":hidden,"filter_active":presentation.filter.is_some(),"charts":charts}),
    )
}
