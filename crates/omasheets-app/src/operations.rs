//! Typed, bounded compatibility proposals. Unknown fields never confer authority.
use crate::Result;
use regex::Regex;
use serde_json::{Map, Value};
use std::{collections::HashSet, sync::LazyLock};
static A1: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\$?([A-Z]{1,3})\$?([1-9][0-9]{0,6})(?::\$?([A-Z]{1,3})\$?([1-9][0-9]{0,6}))?$")
        .unwrap()
});
static UNSAFE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:\b(?:WEBSERVICE|DDE)\s*\(|(?:https?|ftp|file)://|(?:^|[;(])\s*['"]?[^'"]+\.(?:ods|xlsx?|xlsm)['"]?[#.])"#).unwrap()
});
pub const SUPPORTED: &[&str] = &[
    "set_value",
    "set_formula",
    "clear_range",
    "rename_sheet",
    "add_sheet",
    "delete_sheet",
    "set_range_values",
    "set_range_formulas",
    "format_cells",
    "insert_rows",
    "delete_rows",
    "insert_columns",
    "delete_columns",
    "fill_down",
    "fill_right",
    "sort_range",
    "upsert_chart",
    "upsert_pivot",
    "refresh_pivot",
];
pub fn column_number(name: &str) -> Result<u32> {
    if name.is_empty() || name.len() > 3 || !name.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err("invalid column".into());
    }
    Ok(name.bytes().fold(0, |n, b| n * 26 + (b - b'A' + 1) as u32))
}
pub fn range_shape(value: &str) -> Result<(u32, u32)> {
    let value = value.to_ascii_uppercase();
    let c = A1.captures(&value).ok_or("invalid A1 range")?;
    let sc = column_number(&c[1])?;
    let sr = c[2].parse::<u32>().map_err(|e| e.to_string())?;
    let ec = column_number(c.get(3).map_or(&c[1], |m| m.as_str()))?;
    let er = c
        .get(4)
        .map_or(&c[2], |m| m.as_str())
        .parse::<u32>()
        .map_err(|e| e.to_string())?;
    if ec > 16384 || er > 1048576 || ec < sc || er < sr {
        return Err("A1 range exceeds bounds or runs backwards".into());
    }
    Ok((er - sr + 1, ec - sc + 1))
}
fn text<'a>(m: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    m.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("invalid {key}"))
}
fn name(v: &str, sheet: bool) -> bool {
    (1..=128).contains(&v.chars().count())
        && !v
            .chars()
            .any(|c| c < ' ' || (sheet && "\\/?*[]:".contains(c)))
}
fn formula(v: &Value) -> Result<()> {
    let s = v.as_str().ok_or("invalid formula")?;
    if !s.starts_with('=') || s.chars().count() > 8192 || UNSAFE.is_match(s) {
        Err("invalid, external or network-capable formula".into())
    } else {
        Ok(())
    }
}
fn scalar(v: &Value) -> Result<()> {
    if v.is_array() || v.is_object() || v.as_str().is_some_and(|s| s.chars().count() > 32767) {
        Err("invalid scalar value".into())
    } else {
        Ok(())
    }
}
fn bounded_int(m: &Map<String, Value>, key: &str, maximum: u64) -> Result<u64> {
    let n = m
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("invalid {key}"))?;
    if n == 0 || n > maximum {
        Err(format!("invalid {key} bounds"))
    } else {
        Ok(n)
    }
}
fn boolean(m: &Map<String, Value>, key: &str) -> Result<()> {
    if m.get(key).is_some_and(Value::is_boolean) {
        Ok(())
    } else {
        Err(format!("invalid {key}"))
    }
}
fn bounded_range(m: &Map<String, Value>, key: &str) -> Result<(u32, u32)> {
    let shape = range_shape(text(m, key)?)?;
    if shape.0 as u64 * shape.1 as u64 > 10000 {
        return Err("range exceeds the 10000-cell limit".into());
    }
    Ok(shape)
}
fn fields(kind: &str) -> Result<(Vec<&str>, Vec<&str>)> {
    let extra = match kind {
        "set_value" => vec!["range", "value"],
        "set_formula" => vec!["range", "formula"],
        "clear_range" => vec!["range"],
        "rename_sheet" => vec!["new_name"],
        "add_sheet" | "delete_sheet" => vec![],
        "set_range_values" => vec!["range", "values"],
        "set_range_formulas" => vec!["range", "formulas"],
        "format_cells" => vec![
            "range",
            "number_format",
            "bold",
            "text_color",
            "background_color",
            "wrap_text",
        ],
        "insert_rows" | "delete_rows" => vec!["row", "count"],
        "insert_columns" | "delete_columns" => vec!["column", "count"],
        "fill_down" => vec!["range", "source_rows"],
        "fill_right" => vec!["range", "source_columns"],
        "sort_range" => vec!["range", "key_column", "ascending", "has_header"],
        "upsert_chart" => vec![
            "name",
            "source_sheet",
            "source_range",
            "anchor_range",
            "chart_type",
            "title",
            "has_column_headers",
            "has_row_headers",
            "legend",
        ],
        "upsert_pivot" => vec![
            "name",
            "source_sheet",
            "source_range",
            "output_cell",
            "rows",
            "columns",
            "filters",
            "values",
        ],
        "refresh_pivot" => vec!["name"],
        _ => return Err("unsupported operation type".into()),
    };
    let mut allowed = vec!["type", "sheet"];
    allowed.extend(extra);
    let required = if kind == "format_cells" {
        vec!["type", "sheet", "range"]
    } else {
        allowed.clone()
    };
    Ok((allowed, required))
}
pub fn validate(input: &Value) -> Result<Vec<Value>> {
    let ops = input.as_array().ok_or("operations must be an array")?;
    if !(1..=100).contains(&ops.len()) {
        return Err("a plan requires between 1 and 100 operations".into());
    }
    ops.iter()
        .enumerate()
        .map(|(i, v)| validate_one(v).map_err(|e| format!("operation {i}: {e}")))
        .collect()
}
fn validate_one(v: &Value) -> Result<Value> {
    let mut m = v.as_object().ok_or("operation must be an object")?.clone();
    let kind = text(&m, "type")?.to_owned();
    let (allowed, required) = fields(&kind)?;
    if m.keys().any(|k| !allowed.contains(&k.as_str()))
        || required.iter().any(|k| !m.contains_key(*k))
    {
        return Err("invalid fields".into());
    }
    for k in ["sheet", "new_name", "source_sheet", "name"] {
        if m.contains_key(k) && !name(text(&m, k)?, k != "name") {
            return Err(format!("invalid {k}"));
        }
    }
    let shape = if m.contains_key("range") {
        Some(bounded_range(&m, "range")?)
    } else {
        None
    };
    if matches!(kind.as_str(), "set_value" | "set_formula") && text(&m, "range")?.contains(':') {
        return Err("must target one cell".into());
    }
    if let Some(v) = m.get("formula") {
        formula(v)?;
    }
    if let Some(v) = m.get("value") {
        scalar(v)?;
    }
    match kind.as_str() {
        "set_range_values" | "set_range_formulas" => {
            let (rows, cols) = shape.unwrap();
            let formulas = kind == "set_range_formulas";
            let key = if formulas { "formulas" } else { "values" };
            let a = m[key].as_array().ok_or("matrix must be an array")?;
            if a.len() != rows as usize {
                return Err("matrix row count does not match range".into());
            }
            for row in a {
                let row = row.as_array().ok_or("matrix row must be an array")?;
                if row.len() != cols as usize {
                    return Err("matrix column count does not match range".into());
                }
                for v in row {
                    if formulas {
                        formula(v)?;
                    } else {
                        scalar(v)?;
                    }
                }
            }
        }
        "format_cells" => {
            if ![
                "number_format",
                "bold",
                "text_color",
                "background_color",
                "wrap_text",
            ]
            .iter()
            .any(|k| m.contains_key(*k))
            {
                return Err("must specify a format change".into());
            }
            if m.contains_key("number_format") {
                let s = text(&m, "number_format")?;
                if !(1..=128).contains(&s.chars().count()) || s.chars().any(|c| c < ' ') {
                    return Err("invalid number format".into());
                }
            }
            for k in ["bold", "wrap_text"] {
                if m.contains_key(k) {
                    boolean(&m, k)?;
                }
            }
            for k in ["text_color", "background_color"] {
                if m.contains_key(k) {
                    let s = text(&m, k)?;
                    if s.len() != 7
                        || !s.starts_with('#')
                        || !s[1..].bytes().all(|b| b.is_ascii_hexdigit())
                    {
                        return Err(format!("invalid {k}"));
                    }
                }
            }
        }
        "insert_rows" | "delete_rows" => {
            let row = bounded_int(&m, "row", 1048576)?;
            let count = bounded_int(&m, "count", 10000)?;
            if row + count - 1 > 1048576 {
                return Err("invalid row bounds".into());
            }
        }
        "insert_columns" | "delete_columns" => {
            let col = column_number(&text(&m, "column")?.to_ascii_uppercase())? as u64;
            let count = bounded_int(&m, "count", 1000)?;
            if col + count - 1 > 16384 {
                return Err("invalid column bounds".into());
            }
        }
        "fill_down" => {
            bounded_int(&m, "source_rows", shape.unwrap().0 as u64 - 1)?;
        }
        "fill_right" => {
            bounded_int(&m, "source_columns", shape.unwrap().1 as u64 - 1)?;
        }
        "sort_range" => {
            bounded_int(&m, "key_column", shape.unwrap().1 as u64)?;
            boolean(&m, "ascending")?;
            boolean(&m, "has_header")?;
        }
        "upsert_chart" => {
            bounded_range(&m, "source_range")?;
            let (r, c) = range_shape(text(&m, "anchor_range")?)?;
            if r < 2 || c < 2 {
                return Err("chart anchor must span two rows and columns".into());
            }
            if !["column", "bar", "line", "pie", "scatter"].contains(&text(&m, "chart_type")?) {
                return Err("invalid chart type".into());
            }
            if !(1..=256).contains(&text(&m, "title")?.chars().count()) {
                return Err("invalid chart title".into());
            }
            for k in ["has_column_headers", "has_row_headers", "legend"] {
                boolean(&m, k)?;
            }
        }
        "upsert_pivot" => {
            bounded_range(&m, "source_range")?;
            let cell = text(&m, "output_cell")?;
            if cell.contains(':') {
                return Err("output must be one cell".into());
            }
            range_shape(cell)?;
            let mut used = HashSet::new();
            for (k, max) in [("rows", 8), ("columns", 4), ("filters", 4)] {
                let a = m[k].as_array().ok_or("invalid pivot layout")?;
                if a.len() > max {
                    return Err("pivot layout exceeds limit".into());
                }
                for v in a {
                    let s = v.as_str().ok_or("invalid pivot field")?;
                    if !name(s, false) || !used.insert(s.to_lowercase()) {
                        return Err("invalid or reused pivot field".into());
                    }
                }
            }
            let values = m["values"].as_array().ok_or("invalid pivot values")?;
            if !(1..=8).contains(&values.len()) {
                return Err("invalid pivot value count".into());
            }
            for v in values {
                let v = v.as_object().ok_or("invalid pivot value")?;
                if v.keys()
                    .any(|k| !["field", "function", "label"].contains(&k.as_str()))
                {
                    return Err("unknown pivot value fields".into());
                }
                let f = text(v, "field")?;
                if !name(f, false) || !used.insert(f.to_lowercase()) {
                    return Err("invalid or reused pivot value field".into());
                }
                if !["sum", "count", "average", "min", "max"].contains(&text(v, "function")?) {
                    return Err("invalid pivot function".into());
                }
                if v.contains_key("label") && !name(text(v, "label")?, false) {
                    return Err("invalid pivot label".into());
                }
            }
        }
        _ => {}
    }
    for k in [
        "range",
        "source_range",
        "anchor_range",
        "output_cell",
        "column",
        "text_color",
        "background_color",
    ] {
        if let Some(s) = m.get(k).and_then(Value::as_str) {
            m.insert(k.into(), Value::String(s.to_ascii_uppercase()));
        }
    }
    Ok(Value::Object(m))
}
pub fn destructive(ops: &[Value]) -> Vec<usize> {
    ops.iter()
        .enumerate()
        .filter_map(|(i, v)| {
            [
                "clear_range",
                "delete_sheet",
                "delete_rows",
                "delete_columns",
                "sort_range",
            ]
            .contains(&v["type"].as_str().unwrap_or(""))
            .then_some(i)
        })
        .collect()
}
