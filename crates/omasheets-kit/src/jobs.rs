//! Bounded workbook jobs for the existing local service. Python owns only the
//! sandbox and publication boundary; all workbook semantics live here.

mod audit;
mod preview;
mod stage;

use crate::{KitError, WorkbookSession};
use omasheets_core::CellValue;
use omasheets_core::presentation::CellStyle;
use omasheets_service::spreadsheet::Rect;
use omasheets_service::{GridCell, SheetSummary};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

const MAX_REQUEST_BYTES: u64 = 1024 * 1024;
const MAX_RESULT_BYTES: usize = 4 * 1024 * 1024;
const MAX_RANGE_CELLS: usize = 10_000;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Limits {
    max_cells: usize,
    max_formulas: usize,
    max_sheets: usize,
    max_results: usize,
}

impl Limits {
    fn validate(self) -> Result<Self, KitError> {
        if !(1..=250_000).contains(&self.max_cells)
            || !(1..=20_000).contains(&self.max_formulas)
            || !(1..=64).contains(&self.max_sheets)
            || !(1..=200).contains(&self.max_results)
        {
            return invalid("Job limits exceed the owned workbook contract");
        }
        Ok(self)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    action: String,
    source: PathBuf,
    arguments: Value,
    limits: Limits,
}

fn invalid<T>(message: impl Into<String>) -> Result<T, KitError> {
    Err(KitError::Invalid(message.into()))
}

fn text<'a>(object: &'a Value, field: &str) -> Result<&'a str, KitError> {
    object[field]
        .as_str()
        .ok_or_else(|| KitError::Invalid(format!("{field} must be a string")))
}

fn boolean(object: &Value, field: &str, default: bool) -> Result<bool, KitError> {
    match object.get(field) {
        None => Ok(default),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| KitError::Invalid(format!("{field} must be a boolean"))),
    }
}

fn integer(object: &Value, field: &str, default: usize, maximum: usize) -> Result<usize, KitError> {
    let value = match object.get(field) {
        None => default,
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| KitError::Invalid(format!("{field} must be a positive integer")))?,
    };
    if value == 0 || value > maximum {
        return invalid(format!("{field} exceeds its supported bounds"));
    }
    Ok(value)
}

fn fields(arguments: &Value, allowed: &[&str], required: &[&str]) -> Result<(), KitError> {
    let Some(object) = arguments.as_object() else {
        return invalid("Job arguments must be an object");
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str()))
        || required.iter().any(|key| !object.contains_key(*key))
    {
        return invalid("Job has missing or unsupported argument fields");
    }
    Ok(())
}

fn column_name(mut column: usize) -> String {
    let mut letters = Vec::new();
    loop {
        letters.push((b'A' + (column % 26) as u8) as char);
        column /= 26;
        if column == 0 {
            break;
        }
        column -= 1;
    }
    letters.into_iter().rev().collect()
}

fn cell_name(row: usize, column: usize) -> String {
    format!("{}{}", column_name(column), row + 1)
}

fn coordinate(value: &str) -> Result<(usize, usize), KitError> {
    let cleaned = value.replace('$', "").to_ascii_uppercase();
    let split = cleaned.bytes().take_while(u8::is_ascii_alphabetic).count();
    if split == 0 || split > 3 || split == cleaned.len() {
        return invalid("Choose a bounded A1 cell or rectangle");
    }
    let column = cleaned[..split].bytes().fold(0_usize, |column, byte| {
        column * 26 + (byte - b'A' + 1) as usize
    });
    let row = cleaned[split..]
        .parse::<usize>()
        .map_err(|_| KitError::Invalid("Invalid A1 row".into()))?;
    if !(1..=16_384).contains(&column) || !(1..=1_048_576).contains(&row) {
        return invalid("A1 range exceeds spreadsheet bounds");
    }
    Ok((row - 1, column - 1))
}

fn rectangle(value: &str, maximum: usize) -> Result<Rect, KitError> {
    let mut parts = value.split(':');
    let first = parts.next().unwrap_or("");
    let (row, column) = coordinate(first)?;
    let (end_row, end_column) = coordinate(parts.next().unwrap_or(first))?;
    if parts.next().is_some() || end_row < row || end_column < column {
        return invalid("A1 range must run from top-left to bottom-right");
    }
    let rows = end_row - row + 1;
    let columns = end_column - column + 1;
    if rows
        .checked_mul(columns)
        .is_none_or(|count| count > maximum)
    {
        return invalid("Requested range exceeds the bounded read/edit limit");
    }
    Ok(Rect {
        row,
        column,
        rows,
        columns,
    })
}

fn plain(value: &CellValue) -> String {
    match value {
        CellValue::Blank => String::new(),
        CellValue::Number(number) => number.to_string(),
        CellValue::Text(value) | CellValue::Error(value) => value.clone(),
        CellValue::Boolean(value) => if *value { "TRUE" } else { "FALSE" }.into(),
    }
}

fn scalar(value: &CellValue) -> Value {
    match value {
        CellValue::Blank => json!(""),
        CellValue::Number(value) => json!(value),
        CellValue::Text(value) | CellValue::Error(value) => json!(value),
        CellValue::Boolean(value) => json!(value),
    }
}

struct Sheet {
    summary: SheetSummary,
    rows: usize,
    columns: usize,
    cells: BTreeMap<(usize, usize), GridCell>,
    view: Value,
}

fn canonical_style(mut style: CellStyle) -> CellStyle {
    // The owned XLSX exporter writes the native default as an explicit 11-point
    // font. These two representations have identical presentation; every other
    // property, including an explicit non-default font size, stays distinct.
    style.font_size = Some(style.font_size.unwrap_or(11.0));
    style
}

impl Sheet {
    fn value(&self, row: usize, column: usize) -> CellValue {
        self.cells
            .get(&(row, column))
            .map(|cell| cell.value.clone())
            .unwrap_or(CellValue::Blank)
    }
    fn formula(&self, row: usize, column: usize) -> String {
        self.cells
            .get(&(row, column))
            .map_or_else(String::new, |cell| {
                cell.formula.clone().unwrap_or_else(|| plain(&cell.value))
            })
    }
    fn style(&self, row: usize, column: usize) -> CellStyle {
        canonical_style(
            self.cells
                .get(&(row, column))
                .map(|cell| cell.style.clone())
                .unwrap_or_default(),
        )
    }
}

struct Workbook {
    sheets: Vec<Sheet>,
    inspected_cells: usize,
    formulas: Vec<Value>,
    formula_errors: Vec<Value>,
}

impl Workbook {
    fn capture(session: &mut WorkbookSession, limits: Limits) -> Result<Self, KitError> {
        let summaries = session.sheets()?;
        if summaries.len() > limits.max_sheets {
            return invalid("Workbook exceeds the sheet limit");
        }
        // Check all axes before requesting even the first rectangular page.
        let total = summaries.iter().try_fold(0_usize, |total, sheet| {
            sheet.rows.checked_mul(sheet.columns)?.checked_add(total)
        });
        if total.is_none_or(|count| count > limits.max_cells) {
            return invalid("Workbook exceeds the inspected-cell limit");
        }
        let mut workbook = Self {
            sheets: Vec::new(),
            inspected_cells: 0,
            formulas: Vec::new(),
            formula_errors: Vec::new(),
        };
        for summary in summaries {
            let mut cells = BTreeMap::new();
            let mut rows = 1;
            let mut columns = 1;
            for row in (0..summary.rows).step_by(64) {
                for column in (0..summary.columns).step_by(128) {
                    let page = session.viewport(
                        &summary.id.to_string(),
                        row,
                        column,
                        (summary.rows - row).min(64),
                        (summary.columns - column).min(128),
                    )?;
                    for mut cell in page.cells {
                        if let Some(formula) = &mut cell.formula {
                            if !formula.starts_with('=') {
                                formula.insert(0, '=');
                            }
                        }
                        rows = rows.max(cell.row + 1);
                        columns = columns.max(cell.column + 1);
                        if let Some(formula) = &cell.formula {
                            workbook
                                .formulas
                                .push(json!({"sheet": summary.name, "row": cell.row + 1,
                                "column": cell.column + 1, "formula": formula}));
                            if workbook.formulas.len() > limits.max_formulas {
                                return invalid("Workbook exceeds the formula limit");
                            }
                            if matches!(cell.value, CellValue::Error(_))
                                && workbook.formula_errors.len() < limits.max_results
                            {
                                workbook.formula_errors.push(json!({"sheet": summary.name,
                                    "row": cell.row + 1, "column": cell.column + 1,
                                    "formula": formula, "error_code": 0,
                                    "error_engine": "omasheets-calc", "displayed": plain(&cell.value)}));
                            }
                        }
                        cells.insert((cell.row, cell.column), cell);
                    }
                }
            }
            workbook.inspected_cells += rows * columns;
            let view = session.sheet_view(&summary.id.to_string())?;
            workbook.sheets.push(Sheet {
                summary,
                rows,
                columns,
                cells,
                view,
            });
        }
        Ok(workbook)
    }

    fn sheet(&self, name: &str) -> Result<&Sheet, KitError> {
        self.sheets
            .iter()
            .find(|sheet| sheet.summary.name == name)
            .ok_or_else(|| KitError::Invalid(format!("No sheet {name}")))
    }

    fn inspection(&self, include_formulas: bool) -> Value {
        let sheets: Vec<_> = self.sheets.iter().map(|sheet| json!({
            "name": sheet.summary.name, "used_range": {"start_column": 1, "start_row": 1,
                "end_column": sheet.columns, "end_row": sheet.rows},
            "rows": sheet.rows, "columns": sheet.columns,
            "formula_count": sheet.cells.values().filter(|cell| cell.formula.is_some()).count()
        })).collect();
        json!({"sheets": sheets, "sheet_count": sheets.len(), "inspected_cells": self.inspected_cells,
            "formula_count": self.formulas.len(), "formula_errors": self.formula_errors,
            "formulas": if include_formulas { self.formulas.clone() } else { Vec::new() },
            "named_ranges": {"items": [], "total": 0, "truncated": false},
            "engine": {"name": "OmaSheets Kit"}})
    }

    fn read(&self, arguments: &Value, limits: Limits) -> Result<Value, KitError> {
        fields(
            arguments,
            &["sheet", "range", "include_formulas", "include_styles"],
            &["sheet", "range"],
        )?;
        let name = text(arguments, "sheet")?;
        let address = text(arguments, "range")?;
        let range = rectangle(address, limits.max_cells.min(MAX_RANGE_CELLS))?;
        let styles = boolean(arguments, "include_styles", false)?;
        if styles && range.rows * range.columns > 1000 {
            return invalid("Styled range reads are limited to 1000 cells");
        }
        self.read_rectangle(
            name,
            address,
            range,
            boolean(arguments, "include_formulas", true)?,
            styles,
        )
    }

    fn read_rectangle(
        &self,
        name: &str,
        address: &str,
        range: Rect,
        include_formulas: bool,
        include_styles: bool,
    ) -> Result<Value, KitError> {
        let sheet = self.sheet(name)?;
        let values: Vec<Vec<_>> = (range.row..range.row + range.rows)
            .map(|row| {
                (range.column..range.column + range.columns)
                    .map(|column| scalar(&sheet.value(row, column)))
                    .collect()
            })
            .collect();
        let mut result = json!({"sheet": name, "range": address, "values": values});
        if include_formulas {
            let formulas: Vec<Vec<_>> = (range.row..range.row + range.rows)
                .map(|row| {
                    (range.column..range.column + range.columns)
                        .map(|column| sheet.formula(row, column))
                        .collect()
                })
                .collect();
            result["formulas"] = json!(formulas);
        }
        if include_styles {
            let mut styles = Vec::<Value>::new();
            let mut style_ids = Vec::new();
            for row in range.row..range.row + range.rows {
                let mut ids = Vec::new();
                for column in range.column..range.column + range.columns {
                    let style = sheet.style(row, column);
                    let style = json!({"style_name": "native", "number_format": style.number_format,
                        "bold": style.bold, "italic": style.italic, "underline": style.underline,
                        "font_size": style.font_size, "alignment": style.alignment, "border": style.border,
                        "text_color": style.foreground.unwrap_or_else(|| "automatic".into()),
                        "background_color": style.background.unwrap_or_else(|| "automatic".into()), "wrap_text": style.wrap});
                    let id = styles
                        .iter()
                        .position(|prior| *prior == style)
                        .unwrap_or_else(|| {
                            styles.push(style);
                            styles.len() - 1
                        });
                    ids.push(id);
                }
                style_ids.push(ids);
            }
            result["style_table"] = json!({"styles": styles, "style_ids": style_ids});
        }
        Ok(result)
    }

    fn search(&self, arguments: &Value, limits: Limits) -> Result<Value, KitError> {
        fields(arguments, &["query", "scope", "max_results"], &["query"])?;
        let query = text(arguments, "query")?;
        if query.is_empty() || query.chars().count() > 256 {
            return invalid("Search text must contain 1 to 256 characters");
        }
        let query = query.to_lowercase();
        let scope = arguments
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or("both");
        if !["values", "formulas", "both"].contains(&scope) {
            return invalid("Search scope must be values, formulas or both");
        }
        let maximum = integer(
            arguments,
            "max_results",
            50.min(limits.max_results),
            limits.max_results,
        )?;
        let mut matches = Vec::new();
        for sheet in &self.sheets {
            for ((row, column), cell) in &sheet.cells {
                let value = plain(&cell.value);
                let formula = cell.formula.clone();
                let found = (scope != "formulas" && value.to_lowercase().contains(&query))
                    || (scope != "values"
                        && formula
                            .as_ref()
                            .is_some_and(|formula| formula.to_lowercase().contains(&query)));
                if found {
                    if matches.len() == maximum {
                        return Ok(json!({"matches": matches, "truncated": true}));
                    }
                    matches.push(json!({"sheet": sheet.summary.name, "row": row + 1,
                        "column": column + 1, "value": value, "formula": formula}));
                }
            }
        }
        Ok(json!({"matches": matches, "truncated": false}))
    }
}

fn trace(
    session: &mut WorkbookSession,
    workbook: &Workbook,
    arguments: &Value,
    limits: Limits,
) -> Result<Value, KitError> {
    fields(
        arguments,
        &["sheet", "cell", "direction", "max_depth"],
        &["sheet", "cell"],
    )?;
    let name = text(arguments, "sheet")?;
    let address = text(arguments, "cell")?;
    let range = rectangle(address, 1)?;
    let sheet = workbook.sheet(name)?;
    let direction = arguments
        .get("direction")
        .and_then(Value::as_str)
        .unwrap_or("both");
    if !["precedents", "dependents", "both"].contains(&direction) {
        return invalid("Unsupported trace direction");
    }
    let requested_depth = integer(arguments, "max_depth", 5, 10)?;
    let lineage = session.lineage(name, address)?;
    if lineage["truncated"] == true {
        return invalid("Formula inputs exceed the owned tracing limit");
    }
    let root = lineage["cell"]["cell"].clone();
    let mut precedents = Vec::new();
    if direction != "dependents" {
        for input in lineage["inputs"].as_array().into_iter().flatten() {
            precedents.push(json!({"sheet": input["sheet"], "range": input["a1"]}));
            if precedents.len() > limits.max_results {
                return invalid("Formula trace exceeds the result limit");
            }
        }
    }
    let mut dependents = Vec::new();
    if direction != "precedents" {
        for formula in &workbook.formulas {
            let candidate_sheet = text(formula, "sheet")?;
            let candidate_cell = cell_name(
                formula["row"].as_u64().unwrap() as usize - 1,
                formula["column"].as_u64().unwrap() as usize - 1,
            );
            let candidate = session.lineage(candidate_sheet, &candidate_cell)?;
            if candidate["truncated"] == true {
                return invalid("Dependent tracing encountered a formula above the input limit");
            }
            if candidate["inputs"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|input| input["cell"] == root)
            {
                dependents.push(json!({"sheet": candidate_sheet, "range": candidate_cell}));
                if dependents.len() > limits.max_results {
                    return invalid("Formula trace exceeds the result limit");
                }
            }
        }
    }
    Ok(
        json!({"root": {"sheet": name, "cell": address, "formula": sheet.formula(range.row, range.column)},
        "precedents": precedents, "dependents": dependents, "max_depth": requested_depth,
        "resolved_depth": 1, "warnings": ["Tracing reports immediate resolved dependencies; deeper traversal is not performed"]}),
    )
}

fn validate_queries(arguments: &Value) -> Result<Vec<Value>, KitError> {
    fields(arguments, &["queries"], &["queries"])?;
    let queries = arguments["queries"]
        .as_array()
        .ok_or_else(|| KitError::Invalid("Queries must be an array".into()))?;
    if !(1..=8).contains(&queries.len()) {
        return invalid("Query batch must contain between 1 and 8 items");
    }
    let mut identifiers = BTreeSet::new();
    for query in queries {
        fields(
            query,
            &["id", "tool", "arguments"],
            &["id", "tool", "arguments"],
        )?;
        let id = text(query, "id")?;
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            || !identifiers.insert(id)
        {
            return invalid("Query id is invalid or duplicated");
        }
        let action = match text(query, "tool")? {
            "describe_workbook" => "describe",
            "read_range" => "read_range",
            "search_workbook" => "search",
            "trace_formula" => "trace",
            _ => return invalid("Query uses an unsupported read tool"),
        };
        validate_arguments(action, &query["arguments"])?;
    }
    Ok(queries.clone())
}

fn validate_arguments(action: &str, arguments: &Value) -> Result<(), KitError> {
    match action {
        "describe" => {
            fields(arguments, &["include_formulas"], &[])?;
            boolean(arguments, "include_formulas", false)?;
        }
        "read_range" => {
            fields(
                arguments,
                &["sheet", "range", "include_formulas", "include_styles"],
                &["sheet", "range"],
            )?;
            text(arguments, "sheet")?;
            rectangle(text(arguments, "range")?, MAX_RANGE_CELLS)?;
            boolean(arguments, "include_formulas", true)?;
            boolean(arguments, "include_styles", false)?;
        }
        "search" => {
            fields(arguments, &["query", "scope", "max_results"], &["query"])?;
            text(arguments, "query")?;
            integer(arguments, "max_results", 50, 200)?;
            if let Some(scope) = arguments.get("scope") {
                if !["values", "formulas", "both"].contains(&scope.as_str().unwrap_or("")) {
                    return invalid("Unsupported search scope");
                }
            }
        }
        "trace" => {
            fields(
                arguments,
                &["sheet", "cell", "direction", "max_depth"],
                &["sheet", "cell"],
            )?;
            text(arguments, "sheet")?;
            rectangle(text(arguments, "cell")?, 1)?;
            integer(arguments, "max_depth", 5, 10)?;
            if let Some(direction) = arguments.get("direction") {
                if !["precedents", "dependents", "both"].contains(&direction.as_str().unwrap_or(""))
                {
                    return invalid("Unsupported trace direction");
                }
            }
        }
        "analyze" => {
            fields(arguments, &["focus", "max_findings"], &[])?;
            integer(arguments, "max_findings", 50, 100)?;
            if let Some(focus) = arguments.get("focus") {
                if !["all", "quality", "management", "formulas"]
                    .contains(&focus.as_str().unwrap_or(""))
                {
                    return invalid("Unsupported audit focus");
                }
            }
        }
        "render" => fields(arguments, &[], &[])?,
        "stage" => stage::validate(arguments)?,
        "query" => {
            validate_queries(arguments)?;
        }
        _ => return invalid(format!("Unsupported owned workbook action: {action}")),
    }
    Ok(())
}

fn copy_native(source: &Path, destination: &Path) -> Result<(), KitError> {
    let wal = source.with_file_name(format!(
        "{}-wal",
        source.file_name().unwrap().to_string_lossy()
    ));
    let has_wal = || wal.metadata().is_ok_and(|metadata| metadata.len() != 0);
    if has_wal() {
        return invalid("Active native workbook requires the native service tools");
    }
    let before = source.metadata()?;
    let mut incoming = File::open(source)?;
    let mut outgoing = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)?;
    std::io::copy(&mut incoming, &mut outgoing)?;
    outgoing.sync_all()?;
    let after = source.metadata()?;
    if has_wal()
        || (
            before.dev(),
            before.ino(),
            before.len(),
            before.mtime(),
            before.mtime_nsec(),
            before.ctime(),
            before.ctime_nsec(),
        ) != (
            after.dev(),
            after.ino(),
            after.len(),
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec(),
        )
    {
        return invalid("Native workbook changed while preparing the private job");
    }
    Ok(())
}

fn run(root: &Path, job: Job) -> Result<Value, KitError> {
    let limits = job.limits.validate()?;
    validate_arguments(&job.action, &job.arguments)?;
    if job
        .source
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
        || job.source.components().count() != 2
        || job.source.parent() != Some(Path::new("input"))
    {
        return invalid("Job source must be one private input file");
    }
    let source = root.join(&job.source);
    if !fs::symlink_metadata(&source)?.is_file() {
        return invalid("Job input must be a regular file");
    }
    let source = source.canonicalize()?;
    if !source.starts_with(root.join("input")) {
        return invalid("Job input escapes its private directory");
    }
    let out = root.join("out");
    if !out.exists() {
        fs::DirBuilder::new().mode(0o700).create(&out)?;
    }
    if !fs::symlink_metadata(&out)?.is_dir() {
        return invalid("Job output must be a regular directory");
    }
    let working = out.join("session.omasheets");
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut session = match extension.as_str() {
        "xlsx" => WorkbookSession::open_xlsx(&source, &working)?,
        "omasheets" => {
            copy_native(&source, &working)?;
            WorkbookSession::open_native(&working)?
        }
        _ => return invalid("Only XLSX and native .omasheets file jobs are supported"),
    };
    let workbook = Workbook::capture(&mut session, limits)?;
    if job.action == "stage" {
        return stage::run(session, workbook, &job.arguments, limits, &out, &extension);
    }
    let result = match job.action.as_str() {
        "describe" => workbook.inspection(boolean(&job.arguments, "include_formulas", false)?),
        "read_range" => workbook.read(&job.arguments, limits)?,
        "search" => workbook.search(&job.arguments, limits)?,
        "trace" => trace(&mut session, &workbook, &job.arguments, limits)?,
        "analyze" => audit::analyze(&workbook, &job.arguments, limits)?,
        "render" => {
            let result = preview::write(&workbook, &out.join("preview.pdf"))?;
            session.close()?;
            return Ok(json!({"result": result, "artifacts": {"preview": "out/preview.pdf"}}));
        }
        "query" => {
            let mut items = Vec::new();
            for query in validate_queries(&job.arguments)? {
                let arguments = &query["arguments"];
                let result = match text(&query, "tool")? {
                    "describe_workbook" => {
                        workbook.inspection(boolean(arguments, "include_formulas", false)?)
                    }
                    "read_range" => workbook.read(arguments, limits)?,
                    "search_workbook" => workbook.search(arguments, limits)?,
                    "trace_formula" => trace(&mut session, &workbook, arguments, limits)?,
                    _ => unreachable!("validated query"),
                };
                items.push(json!({"id": query["id"], "tool": query["tool"], "result": result}));
            }
            json!({"items": items})
        }
        _ => unreachable!("validated action"),
    };
    session.close()?;
    Ok(json!({"result": result, "artifacts": {}}))
}

/// Execute one job and write one bounded structured response. Failure details
/// are returned in the response file rather than process stderr.
pub fn run_files(request: impl AsRef<Path>, result: impl AsRef<Path>) -> Result<bool, KitError> {
    let request = request.as_ref().canonicalize()?;
    let root = request
        .parent()
        .ok_or_else(|| KitError::Invalid("Job has no private directory".into()))?;
    let response = (|| {
        let file = File::open(&request)?;
        if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_REQUEST_BYTES {
            return invalid("Job request exceeds its size limit");
        }
        let mut bytes = Vec::new();
        file.take(MAX_REQUEST_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_REQUEST_BYTES {
            return invalid("Job request grew beyond its size limit");
        }
        let job: Job =
            serde_json::from_slice(&bytes).map_err(|error| KitError::Invalid(error.to_string()))?;
        run(root, job)
    })();
    let mut payload = match response {
        Ok(mut response) => {
            response["ok"] = json!(true);
            response
        }
        Err(error) => {
            json!({"ok": false, "error": error.to_string().chars().take(512).collect::<String>(), "engine": "omasheets-kit"})
        }
    };
    let mut bytes =
        serde_json::to_vec(&payload).map_err(|error| KitError::Invalid(error.to_string()))?;
    if bytes.len() > MAX_RESULT_BYTES {
        payload = json!({"ok": false, "error": "Job result exceeds the bounded response limit", "engine": "omasheets-kit"});
        bytes = serde_json::to_vec(&payload).expect("small response is serializable");
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(result)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(payload["ok"] == true)
}
