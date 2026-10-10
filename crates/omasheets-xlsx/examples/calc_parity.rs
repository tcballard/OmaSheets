//! Trusted-fixture diagnostic runner for the separate Calc reference workflow.
use omasheets_calc::Value;
use omasheets_xlsx::{ImportLimits, import_xlsx};
use serde_json::{Value as Json, json};
use std::{env, path::Path, process::ExitCode};

fn value(value: &Value) -> Json {
    match value {
        Value::Blank => json!({"type": "blank"}),
        Value::Number(n) => json!({"type": "number", "value": n}),
        Value::Boolean(b) => json!({"type": "boolean", "value": b}),
        Value::Text(s) => json!({"type": "text", "value": s}),
        Value::Error(e) => json!({"type": "error", "value": e.label()}),
    }
}

fn run() -> Result<(), String> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 1 {
        return Err("usage: calc_parity TRUSTED_REFERENCE.xlsx".into());
    }
    let imported = import_xlsx(
        Path::new(&args[0]),
        ImportLimits {
            max_sheets: 8,
            max_cells: 10_000,
            max_formulas: 1_000,
        },
    )
    .map_err(|e| e.to_string())?;
    let mismatches: Vec<_> = imported
        .mismatched_cells()
        .map(|(id, stored, calculated)| {
            let formula = imported
                .source_cells()
                .iter()
                .find(|s| s.cell == id)
                .and_then(|s| s.formula.as_deref());
            json!({"sheet": id.sheet, "row": id.row + 1, "column": id.column + 1,
            "formula": formula, "reference": value(stored), "owned": value(&calculated)})
        })
        .collect();
    let unsupported: Vec<_> = imported
        .unsupported
        .iter()
        .map(|s| {
            json!({
                "sheet": s.cell.sheet, "row": s.cell.row + 1, "column": s.cell.column + 1,
                "reason": s.reason,
            })
        })
        .collect();
    println!(
        "{}",
        json!({"report": imported.report(), "mismatches": mismatches,
        "unsupported": unsupported})
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("calc_parity: {e}");
            ExitCode::FAILURE
        }
    }
}
