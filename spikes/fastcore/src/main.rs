use formualizer_workbook::{
    CalamineAdapter, LoadStrategy, SpreadsheetReader, Workbook, WorkbookConfig,
};
use serde_json::{Value, json};
use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

fn usage() -> &'static str {
    "usage: omasheets-fastcore-spike inspect INPUT.xlsx"
}

fn inspect(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    if path.extension().and_then(|value| value.to_str()) != Some("xlsx") {
        return Err("the first spike intentionally accepts .xlsx only".into());
    }
    let metadata = path.metadata()?;
    if !metadata.is_file() {
        return Err("input must be a regular file".into());
    }

    let started = Instant::now();
    let adapter = CalamineAdapter::open_path(path)?;
    let opened_ms = started.elapsed().as_secs_f64() * 1000.0;
    let loaded_at = Instant::now();
    let (mut workbook, adapter_stats) = Workbook::from_reader_with_adapter_stats(
        adapter,
        LoadStrategy::EagerAll,
        WorkbookConfig::ephemeral().with_span_evaluation(true),
    )?;
    let loaded_ms = loaded_at.elapsed().as_secs_f64() * 1000.0;
    let evaluated_at = Instant::now();
    let evaluation = workbook.evaluate_all()?;
    let evaluated_ms = evaluated_at.elapsed().as_secs_f64() * 1000.0;

    let sheets = workbook
        .sheet_names()
        .into_iter()
        .map(|name| {
            let (rows, columns) = workbook.sheet_dimensions(&name).unwrap_or((0, 0));
            json!({"name": name, "rows": rows, "columns": columns})
        })
        .collect::<Vec<_>>();
    let stats = adapter_stats.unwrap_or_default();

    Ok(json!({
        "schema": 1,
        "engine": "formualizer-calamine",
        "input_bytes": metadata.len(),
        "sheet_count": sheets.len(),
        "sheets": sheets,
        "observed": {
            "value_cells": stats.value_cells_observed,
            "formula_cells": stats.formula_cells_observed,
            "value_slots_loaded": stats.value_slots_handed_to_engine,
            "formula_cells_loaded": stats.formula_cells_handed_to_engine
        },
        "evaluation": format!("{evaluation:?}"),
        "timing_ms": {
            "open": opened_ms,
            "load": loaded_ms,
            "evaluate": evaluated_ms,
            "total": started.elapsed().as_secs_f64() * 1000.0
        }
    }))
}

fn run(arguments: impl IntoIterator<Item = String>) -> Result<Value, Box<dyn std::error::Error>> {
    let mut arguments = arguments.into_iter();
    let command = arguments.next().ok_or_else(|| usage().to_string())?;
    let path = PathBuf::from(arguments.next().ok_or_else(|| usage().to_string())?);
    if arguments.next().is_some() || command != "inspect" {
        return Err(usage().into());
    }
    inspect(&path)
}

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string(&report).expect("JSON serialization")
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("omasheets-fastcore-spike: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn invalid_command_is_rejected() {
        let error = run(["write".to_string(), "book.xlsx".to_string()]).unwrap_err();
        assert!(error.to_string().contains("usage:"));
    }
}
