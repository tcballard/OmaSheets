//! A reproducible replacement-contract scenario. Use the reference XLSX from
//! `scripts/generate_m0_xlsx.py --rows 10`, and a new empty output directory.
use omasheets_core::CellValue;
use omasheets_kit::{WorkbookSession, probe};
use omasheets_service::spreadsheet::Action;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 {
        return Err("usage: round_trip REFERENCE.xlsx NEW_OUTPUT_DIR".into());
    }
    let source = PathBuf::from(&arguments[0]);
    let output = PathBuf::from(&arguments[1]);
    std::fs::create_dir(&output)?;
    let hash = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let source_sha256 = hash(&std::fs::read(&source)?);
    let report = probe(&source)?;
    assert!(report.can_import, "Admission refused: {:?}", report.reasons);
    let working = output.join("working.omasheets");
    let mut session =
        WorkbookSession::open_xlsx_checked(&source, &working, report.source_sha256.as_deref())?;
    let sheet = session.sheets()?[0].id.to_string();
    let revision = session.revision()?;
    session.edit(
        &sheet,
        &revision,
        Action::SetCells {
            row: 1,
            column: 0,
            values: vec![vec!["20".into()]],
        },
    )?;
    let page = session.viewport(&sheet, 1, 2, 1, 1)?;
    assert_eq!(page.cells[0].value, CellValue::Number(22.0));
    assert!(session.is_dirty());
    let digest = session.snapshot()?;
    assert!(session.is_dirty());
    let manifest = session.save_xlsx_copy(output.join("edited.xlsx"))?;
    assert_eq!(manifest.document_digest, digest);
    assert_eq!(manifest.formula_cells_preserved, 10);
    assert_eq!(manifest.formula_cells_flattened, 0);
    session.close()?;
    let mut reopened = WorkbookSession::open_native(&working)?;
    assert_eq!(reopened.snapshot()?, digest);
    assert_eq!(
        reopened.viewport(&sheet, 1, 2, 1, 1)?.cells[0].value,
        CellValue::Number(22.0)
    );
    reopened.close()?;
    assert_eq!(hash(&std::fs::read(&source)?), source_sha256);
    println!(
        "{}",
        json!({"engine":"omasheets-kit","source_sha256":source_sha256,
            "source_preserved":true,"reopened_digest":digest,
            "dependent_cell":"C2","dependent_value":22.0,"export":manifest})
    );
    Ok(())
}
