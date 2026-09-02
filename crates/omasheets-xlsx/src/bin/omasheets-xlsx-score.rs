use omasheets_xlsx::{ImportLimits, import_xlsx};
use std::env;
use std::path::Path;
use std::process::ExitCode;

fn run() -> Result<(), String> {
    let mut arguments = env::args().skip(1);
    let input = arguments
        .next()
        .ok_or_else(|| "usage: omasheets-xlsx-score INPUT.xlsx".to_string())?;
    if arguments.next().is_some() {
        return Err("usage: omasheets-xlsx-score INPUT.xlsx".into());
    }
    let imported = import_xlsx(Path::new(&input), ImportLimits::default())
        .map_err(|error| error.to_string())?;
    let parity = imported.parity();
    println!(
        concat!(
            "{{\n",
            "  \"schema\": 1,\n",
            "  \"engine\": \"omasheets-owned-m0\",\n",
            "  \"source_sha256\": \"{source_sha256}\",\n",
            "  \"sheets\": {sheets},\n",
            "  \"formula_cells_observed\": {observed},\n",
            "  \"formula_cells_loaded\": {loaded},\n",
            "  \"formula_cells_compared\": {compared},\n",
            "  \"stored_values_matched\": {matched},\n",
            "  \"stored_values_mismatched\": {mismatched},\n",
            "  \"unsupported_formulas\": {unsupported}\n",
            "}}"
        ),
        source_sha256 = imported.source_sha256,
        sheets = imported.sheets.len(),
        observed = parity.formula_cells_observed,
        loaded = parity.formula_cells_loaded,
        compared = parity.formula_cells_compared,
        matched = parity.stored_values_matched,
        mismatched = parity.stored_values_mismatched,
        unsupported = parity.unsupported_formulas,
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("omasheets-xlsx-score: {error}");
            ExitCode::FAILURE
        }
    }
}
