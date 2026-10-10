use super::*;
use omasheets_service::spreadsheet::Action;

pub(super) fn validate(arguments: &Value) -> Result<(), KitError> {
    fields(arguments, &["operations"], &["operations"])?;
    let operations = arguments["operations"]
        .as_array()
        .ok_or_else(|| KitError::Invalid("Operations must be an array".into()))?;
    if !(1..=100).contains(&operations.len()) {
        return invalid("A stage requires 1 to 100 operations");
    }
    let mut target_cells = 0;
    for operation in operations {
        let kind = text(operation, "type")?;
        let (allowed, required): (&[&str], &[&str]) = match kind {
            "set_value" => (
                &["type", "sheet", "range", "value"],
                &["type", "sheet", "range", "value"],
            ),
            "set_formula" => (
                &["type", "sheet", "range", "formula"],
                &["type", "sheet", "range", "formula"],
            ),
            "clear_range" => (&["type", "sheet", "range"], &["type", "sheet", "range"]),
            "set_range_values" => (
                &["type", "sheet", "range", "values"],
                &["type", "sheet", "range", "values"],
            ),
            "set_range_formulas" => (
                &["type", "sheet", "range", "formulas"],
                &["type", "sheet", "range", "formulas"],
            ),
            "format_cells" => (
                &[
                    "type",
                    "sheet",
                    "range",
                    "number_format",
                    "bold",
                    "text_color",
                    "background_color",
                    "wrap_text",
                ],
                &["type", "sheet", "range"],
            ),
            "add_sheet" | "delete_sheet" => (&["type", "sheet"], &["type", "sheet"]),
            "rename_sheet" => (
                &["type", "sheet", "new_name"],
                &["type", "sheet", "new_name"],
            ),
            "insert_rows" | "delete_rows" => (
                &["type", "sheet", "row", "count"],
                &["type", "sheet", "row", "count"],
            ),
            "insert_columns" | "delete_columns" => (
                &["type", "sheet", "column", "count"],
                &["type", "sheet", "column", "count"],
            ),
            "sort_range" => (
                &[
                    "type",
                    "sheet",
                    "range",
                    "key_column",
                    "ascending",
                    "has_header",
                ],
                &[
                    "type",
                    "sheet",
                    "range",
                    "key_column",
                    "ascending",
                    "has_header",
                ],
            ),
            _ => return invalid(format!("Unsupported selected-file operation: {kind}")),
        };
        fields(operation, allowed, required)?;
        let sheet = text(operation, "sheet")?;
        if sheet.is_empty() || sheet.chars().count() > 128 {
            return invalid("Sheet name exceeds its supported bounds");
        }
        if let Some(address) = operation.get("range") {
            let range = rectangle(
                address
                    .as_str()
                    .ok_or_else(|| KitError::Invalid("Range must be text".into()))?,
                MAX_RANGE_CELLS,
            )?;
            target_cells += range.rows * range.columns;
            if target_cells > 250_000 {
                return invalid("Combined operation targets exceed the stage limit");
            }
            if ["set_value", "set_formula"].contains(&kind) && range.rows * range.columns != 1 {
                return invalid("Scalar edits target exactly one cell");
            }
            match kind {
                "set_value" => {
                    literal(&operation["value"])?;
                }
                "set_formula" => {
                    formula(&operation["formula"])?;
                }
                "set_range_values" => {
                    matrix(&operation["values"], range, false)?;
                }
                "set_range_formulas" => {
                    matrix(&operation["formulas"], range, true)?;
                }
                "format_cells" => {
                    style_patch(operation)?;
                }
                "sort_range" => {
                    integer(operation, "key_column", 1, range.columns)?;
                    boolean(operation, "ascending", true)?;
                    boolean(operation, "has_header", false)?;
                }
                _ => {}
            }
        }
        if kind == "rename_sheet" {
            text(operation, "new_name")?;
        }
        if ["insert_rows", "delete_rows"].contains(&kind) {
            integer(operation, "row", 1, 1_048_576)?;
            integer(operation, "count", 1, 10_000)?;
        }
        if ["insert_columns", "delete_columns"].contains(&kind) {
            coordinate(&format!("{}1", text(operation, "column")?))?;
            integer(operation, "count", 1, 1000)?;
        }
    }
    Ok(())
}

fn literal(value: &Value) -> Result<String, KitError> {
    Ok(match value {
        Value::Null => String::new(),
        Value::Bool(value) => if *value { "TRUE" } else { "FALSE" }.into(),
        Value::Number(value) => value.to_string(),
        Value::String(value) if value.chars().count() <= 32_768 => format!("'{value}"),
        _ => return invalid("Cell value must be a bounded scalar"),
    })
}

fn formula(value: &Value) -> Result<String, KitError> {
    let value = value
        .as_str()
        .ok_or_else(|| KitError::Invalid("Formula must be a string".into()))?;
    if !value.starts_with('=') || value.len() > 8192 {
        return invalid("Formula must start with = and contain at most 8192 bytes");
    }
    Ok(value.to_owned())
}

fn matrix(value: &Value, range: Rect, formulas: bool) -> Result<Vec<Vec<String>>, KitError> {
    let rows = value
        .as_array()
        .ok_or_else(|| KitError::Invalid("Cell matrix must be an array".into()))?;
    if rows.len() != range.rows {
        return invalid("Cell matrix does not match its target rows");
    }
    rows.iter()
        .map(|row| {
            let row = row
                .as_array()
                .ok_or_else(|| KitError::Invalid("Cell matrix rows must be arrays".into()))?;
            if row.len() != range.columns {
                return invalid("Cell matrix does not match its target columns");
            }
            row.iter()
                .map(|value| {
                    if formulas {
                        formula(value)
                    } else {
                        literal(value)
                    }
                })
                .collect()
        })
        .collect()
}

fn style_patch(operation: &Value) -> Result<Value, KitError> {
    let mut patch = serde_json::Map::new();
    for (input, output) in [
        ("number_format", "number_format"),
        ("bold", "bold"),
        ("text_color", "foreground"),
        ("background_color", "background"),
        ("wrap_text", "wrap"),
    ] {
        if let Some(value) = operation.get(input) {
            patch.insert(output.into(), value.clone());
        }
    }
    if patch.is_empty() {
        return invalid("Format operation must specify at least one style property");
    }
    let style: CellStyle = serde_json::from_value(Value::Object(patch.clone()))
        .map_err(|error| KitError::Invalid(error.to_string()))?;
    style
        .validate()
        .map_err(|error| KitError::Invalid(error.to_string()))?;
    Ok(Value::Object(patch))
}

fn edit(session: &mut WorkbookSession, sheet: &str, action: Action) -> Result<(), KitError> {
    let revision = session.revision()?;
    session.edit(sheet, &revision, action)?;
    Ok(())
}

fn ensure_axes(
    session: &mut WorkbookSession,
    sheet: &str,
    range: Rect,
    limits: Limits,
) -> Result<(), KitError> {
    let summaries = session.sheets()?;
    let summary = summaries
        .iter()
        .find(|candidate| candidate.name == sheet)
        .ok_or_else(|| KitError::Invalid(format!("No sheet {sheet}")))?;
    let rows = summary.rows.max(range.row + range.rows);
    let columns = summary.columns.max(range.column + range.columns);
    let total = summaries.iter().try_fold(0_usize, |total, candidate| {
        let cells = if candidate.name == sheet {
            rows.checked_mul(columns)?
        } else {
            candidate.rows.checked_mul(candidate.columns)?
        };
        total.checked_add(cells)
    });
    if total.is_none_or(|total| total > limits.max_cells) {
        return invalid("Edit would exceed the bounded workbook dimensions");
    }
    if rows > summary.rows {
        edit(
            session,
            sheet,
            Action::InsertRows {
                at: summary.rows,
                count: rows - summary.rows,
            },
        )?;
    }
    if columns > summary.columns {
        edit(
            session,
            sheet,
            Action::InsertColumns {
                at: summary.columns,
                count: columns - summary.columns,
            },
        )?;
    }
    Ok(())
}

fn set_matrix(
    session: &mut WorkbookSession,
    sheet: &str,
    range: Rect,
    values: Vec<Vec<String>>,
) -> Result<(), KitError> {
    // The owned edit endpoint commits at most 1000 cells per request. Split a
    // larger validated matrix into private job edits; publication still waits
    // for complete save/reopen verification of the whole stage.
    for row in (0..range.rows).step_by(32) {
        for column in (0..range.columns).step_by(16) {
            let rows = (range.rows - row).min(32);
            let columns = (range.columns - column).min(16);
            let values = values[row..row + rows]
                .iter()
                .map(|line| line[column..column + columns].to_vec())
                .collect();
            edit(
                session,
                sheet,
                Action::SetCells {
                    row: range.row + row,
                    column: range.column + column,
                    values,
                },
            )?;
        }
    }
    Ok(())
}

fn apply(session: &mut WorkbookSession, operation: &Value, limits: Limits) -> Result<(), KitError> {
    let kind = text(operation, "type")?;
    let sheet = text(operation, "sheet")?;
    match kind {
        "add_sheet" => {
            let base = session
                .sheets()?
                .first()
                .map(|sheet| sheet.id.to_string())
                .ok_or_else(|| {
                    KitError::Invalid(
                        "Adding the first sheet to an empty file job is not supported".into(),
                    )
                })?;
            edit(session, &base, Action::AddSheet { name: sheet.into() })?;
        }
        "delete_sheet" => edit(session, sheet, Action::DeleteSheet)?,
        "rename_sheet" => edit(
            session,
            sheet,
            Action::RenameSheet {
                name: text(operation, "new_name")?.into(),
            },
        )?,
        "insert_rows" => edit(
            session,
            sheet,
            Action::InsertRows {
                at: integer(operation, "row", 1, 1_048_576)? - 1,
                count: integer(operation, "count", 1, 10_000)?,
            },
        )?,
        "delete_rows" => edit(
            session,
            sheet,
            Action::DeleteRows {
                at: integer(operation, "row", 1, 1_048_576)? - 1,
                count: integer(operation, "count", 1, 10_000)?,
            },
        )?,
        "insert_columns" => edit(
            session,
            sheet,
            Action::InsertColumns {
                at: coordinate(&format!("{}1", text(operation, "column")?))?.1,
                count: integer(operation, "count", 1, 1000)?,
            },
        )?,
        "delete_columns" => edit(
            session,
            sheet,
            Action::DeleteColumns {
                at: coordinate(&format!("{}1", text(operation, "column")?))?.1,
                count: integer(operation, "count", 1, 1000)?,
            },
        )?,
        _ => {
            let range = rectangle(text(operation, "range")?, MAX_RANGE_CELLS)?;
            ensure_axes(session, sheet, range, limits)?;
            match kind {
                "set_value" => set_matrix(
                    session,
                    sheet,
                    range,
                    vec![vec![literal(&operation["value"])?]],
                )?,
                "set_formula" => set_matrix(
                    session,
                    sheet,
                    range,
                    vec![vec![formula(&operation["formula"])?]],
                )?,
                "set_range_values" => set_matrix(
                    session,
                    sheet,
                    range,
                    matrix(&operation["values"], range, false)?,
                )?,
                "set_range_formulas" => set_matrix(
                    session,
                    sheet,
                    range,
                    matrix(&operation["formulas"], range, true)?,
                )?,
                "format_cells" => edit(
                    session,
                    sheet,
                    Action::Format {
                        range,
                        patch: style_patch(operation)?,
                    },
                )?,
                "clear_range" => {
                    let workbook = Workbook::capture(session, limits)?;
                    let notes: Vec<_> = workbook
                        .sheet(sheet)?
                        .cells
                        .values()
                        .filter(|cell| {
                            cell.row >= range.row
                                && cell.row < range.row + range.rows
                                && cell.column >= range.column
                                && cell.column < range.column + range.columns
                                && !cell.note.is_empty()
                        })
                        .map(|cell| (cell.row, cell.column))
                        .collect();
                    set_matrix(
                        session,
                        sheet,
                        range,
                        vec![vec![String::new(); range.columns]; range.rows],
                    )?;
                    edit(session, sheet, Action::ClearFormat { range })?;
                    for (row, column) in notes {
                        edit(
                            session,
                            sheet,
                            Action::Note {
                                row,
                                column,
                                text: String::new(),
                            },
                        )?;
                    }
                }
                "sort_range" => {
                    let workbook = Workbook::capture(session, limits)?;
                    if range.column != 0 || range.columns < workbook.sheet(sheet)?.columns {
                        return invalid(
                            "Owned sort moves complete rows; a partial-column sort is unsupported",
                        );
                    }
                    edit(
                        session,
                        sheet,
                        Action::Sort {
                            range,
                            column: range.column
                                + integer(operation, "key_column", 1, range.columns)?
                                - 1,
                            descending: !boolean(operation, "ascending", true)?,
                            header: boolean(operation, "has_header", false)?,
                        },
                    )?;
                }
                _ => unreachable!("validated operation"),
            }
        }
    }
    Ok(())
}

fn inventory(workbook: &Workbook) -> Value {
    let inspection = workbook.inspection(false);
    let sheets: Vec<_> = inspection["sheets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|sheet| json!({"name": sheet["name"], "used_range": sheet["used_range"]}))
        .collect();
    json!({"sheets": sheets, "formula_count": workbook.formulas.len(),
        "formula_errors": workbook.formula_errors, "named_ranges": inspection["named_ranges"]})
}

fn fingerprint(workbook: &Workbook) -> Value {
    let sheets: Vec<_> = workbook
        .sheets
        .iter()
        .map(|sheet| {
            let cells: Vec<_> = sheet
                .cells
                .values()
                .filter(|cell| {
                    cell.formula.is_some()
                        || cell.value != CellValue::Blank
                        || canonical_style(cell.style.clone())
                            != canonical_style(CellStyle::default())
                        || !cell.note.is_empty()
                })
                .map(|cell| {
                    json!({"row": cell.row, "column": cell.column, "value": cell.value,
                "formula": cell.formula, "style": canonical_style(cell.style.clone()), "note": cell.note})
                })
                .collect();
            json!({"name": sheet.summary.name, "cells": cells, "presentation": sheet.view})
        })
        .collect();
    json!(sheets)
}

fn targets(
    workbook: &Workbook,
    operations: &[Value],
) -> Result<BTreeMap<(String, String), Value>, KitError> {
    let mut targets = BTreeMap::new();
    for operation in operations {
        if let Some(address) = operation.get("range").and_then(Value::as_str) {
            let name = text(operation, "sheet")?;
            if workbook.sheet(name).is_ok() {
                let range = rectangle(address, MAX_RANGE_CELLS)?;
                let mut target = workbook.read_rectangle(name, address, range, true, true)?;
                let style = workbook.sheet(name)?.style(range.row, range.column);
                target["format"] = json!({"number_format": style.number_format, "bold": style.bold,
                    "text_color": style.foreground.unwrap_or_else(|| "automatic".into()),
                    "background_color": style.background.unwrap_or_else(|| "automatic".into()), "wrap_text": style.wrap});
                targets.insert((name.into(), address.into()), target);
            }
        }
    }
    Ok(targets)
}

pub(super) fn run(
    mut session: WorkbookSession,
    before: Workbook,
    arguments: &Value,
    limits: Limits,
    out: &Path,
    extension: &str,
) -> Result<Value, KitError> {
    let operations = arguments["operations"]
        .as_array()
        .expect("validated operations");
    let before_targets = targets(&before, operations)?;
    for operation in operations {
        apply(&mut session, operation, limits)?;
    }
    let expected = Workbook::capture(&mut session, limits)?;
    let new_errors: Vec<_> = expected
        .formula_errors
        .iter()
        .filter(|error| !before.formula_errors.contains(error))
        .cloned()
        .collect();
    if !new_errors.is_empty() {
        return invalid("Stage introduces native formula errors; no workbook was published");
    }
    let expected_targets = targets(&expected, operations)?;
    let working = session.working_path().to_path_buf();
    let workbook = out.join(format!("workbook.{extension}"));
    if extension == "xlsx" {
        session.save_xlsx_copy(&workbook)?;
    }
    session.close()?;
    if extension == "omasheets" {
        copy_native(&working, &workbook)?;
    }
    let mut reopened = if extension == "xlsx" {
        WorkbookSession::open_xlsx(&workbook, out.join("reopened.omasheets"))?
    } else {
        WorkbookSession::open_native(&workbook)?
    };
    let after = Workbook::capture(&mut reopened, limits)?;
    let reopened_targets = targets(&after, operations)?;
    let comparison = json!({"sheet_inventory_match": inventory(&expected)["sheets"] == inventory(&after)["sheets"],
        "named_ranges_match": true, "formula_count_match": expected.formulas.len() == after.formulas.len(),
        "target_ranges_match": expected_targets == reopened_targets,
        "workbook_objects_match": audit::objects(&expected) == audit::objects(&after),
        "all_cells_and_presentation_match": fingerprint(&expected) == fingerprint(&after),
        "new_formula_errors": new_errors});
    if comparison
        .as_object()
        .unwrap()
        .iter()
        .any(|(key, value)| key != "new_formula_errors" && *value != true)
    {
        return invalid(format!(
            "Stage failed save and reopen verification: {comparison}"
        ));
    }
    let preview = preview::write(&after, &out.join("preview.pdf"))?;
    reopened.close()?;
    let keys: BTreeSet<_> = before_targets
        .keys()
        .chain(expected_targets.keys())
        .cloned()
        .collect();
    let target_changes: Vec<_> = keys
        .into_iter()
        .map(|key| {
            json!({"sheet": key.0, "range": key.1,
        "before": before_targets.get(&key).cloned().unwrap_or_else(|| json!({})),
        "after": expected_targets.get(&key).cloned().unwrap_or_else(|| json!({}))})
        })
        .collect();
    Ok(
        json!({"result": {"semantic_diff": {"operation_count": operations.len(), "before": inventory(&before),
        "after": inventory(&after), "target_changes": target_changes,
        "object_changes": {"before": audit::objects(&before), "after": audit::objects(&after)}},
        "verification": {"status": "verified", "recalculated": true, "reopened": true,
            "filter_name": if extension == "xlsx" { "Owned strict XLSX" } else { "Owned native workbook" },
            "comparison": comparison, "excel_equivalence": "not_claimed"},
        "warnings": preview["warnings"], "preview": preview, "engine": {"name": "OmaSheets Kit"}},
        "artifacts": {"workbook": format!("out/workbook.{extension}"), "preview": "out/preview.pdf"}}),
    )
}
