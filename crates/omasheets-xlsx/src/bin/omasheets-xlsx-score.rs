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
    let report =
        serde_json::to_string_pretty(&imported.report()).map_err(|error| error.to_string())?;
    println!("{report}");
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
