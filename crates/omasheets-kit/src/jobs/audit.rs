use super::*;
use sha2::{Digest, Sha256};

#[derive(Default)]
struct Column {
    populated: usize,
    blanks: usize,
    distinct: BTreeSet<String>,
    numeric: Vec<f64>,
    formulas: usize,
}

struct Findings {
    items: Vec<Value>,
    total: usize,
    maximum: usize,
}

impl Findings {
    fn add(
        &mut self,
        severity: &str,
        category: &str,
        sheet: &str,
        address: &str,
        message: &str,
        metrics: Value,
    ) {
        self.total += 1;
        if self.items.len() < self.maximum {
            self.items
                .push(json!({"id": format!("F{:03}", self.items.len() + 1),
                "severity": severity, "category": category, "sheet": sheet,
                "range": address, "message": message, "metrics": metrics}));
        }
    }
}

pub(super) fn objects(workbook: &Workbook) -> Value {
    let mut charts = Vec::new();
    let mut truncated = false;
    for sheet in &workbook.sheets {
        for chart in sheet.view["charts"].as_array().into_iter().flatten() {
            if charts.len() == 500 {
                truncated = true;
                break;
            }
            charts.push(json!({"sheet": sheet.summary.name, "name": chart["id"],
                "title": chart["title"], "kind": chart["kind"], "source_range": chart["range"]}));
        }
    }
    json!({"charts": charts, "pivots": [], "truncated": truncated})
}

pub(super) fn analyze(
    workbook: &Workbook,
    arguments: &Value,
    limits: Limits,
) -> Result<Value, KitError> {
    let maximum = integer(arguments, "max_findings", 50, 100)?.min(limits.max_results);
    let focus = arguments
        .get("focus")
        .and_then(Value::as_str)
        .unwrap_or("all");
    let mut findings = Findings {
        items: Vec::new(),
        total: 0,
        maximum,
    };
    let mut profiles = Vec::new();
    let mut opportunities = Vec::new();
    let mut data_rows = 0;
    for sheet in &workbook.sheets {
        let name = &sheet.summary.name;
        let table_range = format!("A1:{}", cell_name(sheet.rows - 1, sheet.columns - 1));
        let header_range = format!("A1:{}1", column_name(sheet.columns - 1));
        let headers: Vec<_> = (0..sheet.columns)
            .map(|column| plain(&sheet.value(0, column)).trim().to_owned())
            .collect();
        let blank_headers: Vec<_> = headers
            .iter()
            .enumerate()
            .filter(|(_, header)| header.is_empty())
            .map(|(column, _)| column_name(column))
            .take(20)
            .collect();
        if sheet.rows > 1 && !blank_headers.is_empty() {
            findings.add(
                "warning",
                "missing_header",
                name,
                &header_range,
                "The table has blank header cells.",
                json!({"blank_columns": blank_headers}),
            );
        }
        let mut header_counts = BTreeMap::new();
        for header in headers.iter().filter(|header| !header.is_empty()) {
            *header_counts.entry(header.to_lowercase()).or_insert(0) += 1;
        }
        let duplicates: Vec<_> = header_counts
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(header, _)| header)
            .take(20)
            .collect();
        if !duplicates.is_empty() {
            findings.add(
                "warning",
                "duplicate_header",
                name,
                &header_range,
                "The table has duplicate column names.",
                json!({"headers": duplicates}),
            );
        }
        let mut columns: Vec<_> = (0..sheet.columns).map(|_| Column::default()).collect();
        let mut seen = BTreeMap::new();
        let mut duplicate_count = 0;
        let mut duplicate_examples = Vec::new();
        for row in 1..sheet.rows {
            let mut values = Vec::new();
            let mut has_data = false;
            for (column, statistics) in columns.iter_mut().enumerate() {
                let value = sheet.value(row, column);
                values.push(scalar(&value));
                if sheet
                    .cells
                    .get(&(row, column))
                    .is_some_and(|cell| cell.formula.is_some())
                {
                    statistics.formulas += 1;
                }
                if matches!(&value, CellValue::Blank)
                    || matches!(&value, CellValue::Text(value) if value.is_empty())
                {
                    statistics.blanks += 1;
                    continue;
                }
                has_data = true;
                statistics.populated += 1;
                if statistics.populated <= 10_000 {
                    statistics.distinct.insert(plain(&value));
                }
                if let CellValue::Number(number) = value {
                    statistics.numeric.push(number);
                }
            }
            if has_data && ["all", "quality"].contains(&focus) {
                let digest = Sha256::digest(
                    serde_json::to_vec(&values).expect("cell values are serializable"),
                );
                if let Some(prior) = seen.get(&digest) {
                    duplicate_count += 1;
                    if duplicate_examples.len() < 10 {
                        duplicate_examples.push(json!({"row": row + 1, "matches_row": prior}));
                    }
                } else {
                    seen.insert(digest, row + 1);
                }
            }
        }
        if duplicate_count != 0 {
            findings.add(
                "warning",
                "duplicate_rows",
                name,
                &table_range,
                "Duplicate data rows may distort totals.",
                json!({"duplicate_count": duplicate_count, "examples": duplicate_examples}),
            );
        }
        let mut column_profiles = Vec::new();
        let mut dimensions = Vec::new();
        let mut measures = Vec::new();
        for (column, statistics) in columns.iter().enumerate() {
            let header = if headers[column].is_empty() {
                format!("Column {}", column_name(column))
            } else {
                headers[column].clone()
            };
            let mut profile = json!({"column": column_name(column), "header": header,
                "populated": statistics.populated, "blanks": statistics.blanks,
                "distinct": statistics.distinct.len(), "distinct_truncated": statistics.populated > 10_000,
                "numeric": statistics.numeric.len(), "formula_cells": statistics.formulas});
            let address = format!(
                "{}2:{}{}",
                column_name(column),
                column_name(column),
                sheet.rows
            );
            if !statistics.numeric.is_empty() {
                let sum: f64 = statistics.numeric.iter().sum();
                if !sum.is_finite() {
                    return invalid("Numeric audit aggregate exceeds the supported finite range");
                }
                profile["min"] =
                    json!(statistics.numeric.iter().copied().reduce(f64::min).unwrap());
                profile["max"] =
                    json!(statistics.numeric.iter().copied().reduce(f64::max).unwrap());
                profile["sum"] = json!(sum);
                profile["mean"] = json!(sum / statistics.numeric.len() as f64);
                measures.push(header.clone());
                if statistics.numeric.len() >= 8
                    && ["all", "quality", "management"].contains(&focus)
                {
                    let mut ordered = statistics.numeric.clone();
                    ordered.sort_by(f64::total_cmp);
                    let q1 = ordered[ordered.len() / 4];
                    let q3 = ordered[ordered.len() * 3 / 4];
                    let low = q1 - 1.5 * (q3 - q1);
                    let high = q3 + 1.5 * (q3 - q1);
                    let outliers: Vec<_> = ordered
                        .iter()
                        .filter(|value| **value < low || **value > high)
                        .copied()
                        .collect();
                    if !outliers.is_empty() {
                        findings.add("notice", "numeric_outliers", name, &address,
                            &format!("{header} contains values outside the IQR range."),
                            json!({"count": outliers.len(), "low": low, "high": high, "examples": outliers.into_iter().take(5).collect::<Vec<_>>() }));
                    }
                }
            } else if statistics.populated != 0 {
                dimensions.push(header.clone());
            }
            if sheet.rows > 1
                && statistics.blanks as f64 / (sheet.rows - 1) as f64 >= 0.25
                && ["all", "quality"].contains(&focus)
            {
                findings.add(
                    "notice",
                    "sparse_column",
                    name,
                    &address,
                    &format!("{header} is at least 25% blank."),
                    json!({"blank_count": statistics.blanks, "row_count": sheet.rows - 1}),
                );
            }
            column_profiles.push(profile);
        }
        if !measures.is_empty() && opportunities.len() < 20 {
            opportunities.push(json!({"sheet": name, "source_range": table_range,
                "recommended_chart": if dimensions.is_empty() { "line" } else { "column" },
                "dimensions": dimensions.into_iter().take(4).collect::<Vec<_>>(), "measures": measures.into_iter().take(8).collect::<Vec<_>>() }));
        }
        data_rows += sheet.rows - 1;
        profiles.push(json!({"sheet": name, "table_range": table_range, "headers": headers, "columns": column_profiles}));
    }
    for error in &workbook.formula_errors {
        let address = cell_name(
            error["row"].as_u64().unwrap() as usize - 1,
            error["column"].as_u64().unwrap() as usize - 1,
        );
        findings.add(
            "error",
            "formula_error",
            text(error, "sheet")?,
            &address,
            "A formula currently evaluates to an error.",
            json!({"displayed": error["displayed"], "error_code": error["error_code"]}),
        );
    }
    Ok(
        json!({"focus": focus, "summary": {"sheet_count": workbook.sheets.len(),
        "inspected_cells": workbook.inspected_cells, "data_rows": data_rows,
        "formula_count": workbook.formulas.len(), "formula_error_count": workbook.formula_errors.len(),
        "finding_count": findings.items.len(), "finding_total": findings.total, "truncated": findings.total > findings.items.len()},
        "sheets": profiles, "findings": findings.items, "objects": objects(workbook),
        "management_summary_opportunities": opportunities, "method": "deterministic_bounded_profile_v1",
        "engine": {"name": "OmaSheets Kit"}}),
    )
}
