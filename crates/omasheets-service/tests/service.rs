use arrow_array::{Array, BooleanArray, Float64Array, StringArray};
use omasheets_core::{
    Actor, ActorKind, CellValue, ColumnType, Command, InferredColumnType, Literal, Severity,
};
use omasheets_service::{Request, Response, Service, ServiceError};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_document() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "omasheets-service-{}-{nonce}.omasheets",
        std::process::id()
    ))
}

fn temp_xlsx() -> PathBuf {
    let path = temp_document().with_extension("xlsx");
    let file = std::fs::File::create(&path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    for (name, body) in [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
        ),
        (
            "xl/worksheets/sheet1.xml",
            r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>2</v></c><c r="B1" t="b"><v>1</v></c></row><row r="2"><c r="A2"><v>3</v></c><c r="B2"><f>NOPE(A1)</f><v>9</v></c></row><row r="3"><c r="A3"><f>A1+A2</f><v>5</v></c></row></sheetData></worksheet>"#,
        ),
    ] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(body.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
    path
}

fn human(id: &str) -> Actor {
    Actor::new(ActorKind::Human, id)
}

fn agent(id: &str) -> Actor {
    Actor::new(ActorKind::Agent, id)
}

fn sheet_of(service: &mut Service, path: &Path, branch: Option<&str>) -> String {
    match service
        .handle(Request::Document {
            path: path.to_path_buf(),
            branch: branch.map(str::to_string),
        })
        .unwrap()
    {
        Response::Document(summary) => summary.sheets[0].id.to_string(),
        other => panic!("{other:?}"),
    }
}

fn append(
    service: &mut Service,
    path: &Path,
    branch: Option<&str>,
    actor: Actor,
    command: Command,
) -> Result<Response, ServiceError> {
    service.handle(Request::Append {
        path: path.to_path_buf(),
        branch: branch.map(str::to_string),
        actor,
        command,
    })
}

#[test]
fn one_api_drives_the_whole_branch_workflow() {
    let path = temp_document();
    let clock = std::sync::atomic::AtomicI64::new(0);
    let mut service =
        Service::new(move || clock.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1);
    let created = service
        .handle(Request::Create {
            path: path.clone(),
            name: "Plan".into(),
            actor: human("tom"),
        })
        .unwrap();
    assert!(matches!(created, Response::Created { ref branch, .. } if branch == "main"));

    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::AddSheet {
            name: "Model".into(),
        },
    )
    .unwrap();

    let sheet = sheet_of(&mut service, &path, None);
    let sheet_id = omasheets_core::SheetId(omasheets_core::ObjectId::parse(&sheet).unwrap());
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::AddColumns {
            sheet: sheet_id,
            count: 2,
            at: 0,
        },
    )
    .unwrap();
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::AddRows {
            sheet: sheet_id,
            count: 3,
            at: 0,
            table: None,
        },
    )
    .unwrap();
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::SetValue {
            sheet: sheet_id,
            a1: "A1".into(),
            value: Literal::Number(4.0),
        },
    )
    .unwrap();
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::SetFormula {
            sheet: sheet_id,
            a1: "B1".into(),
            source: "=A1*2".into(),
        },
    )
    .unwrap();
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::AddCheck {
            name: "small".into(),
            sheet: sheet_id,
            a1: "A1".into(),
            severity: Severity::Error,
            message: "must be true".into(),
        },
    )
    .unwrap();

    // Sheets resolve by name as well as by id.
    let by_name = service
        .handle(Request::Cell {
            path: path.clone(),
            branch: None,
            sheet: "Model".into(),
            a1: "B1".into(),
        })
        .unwrap();
    assert!(
        matches!(by_name, Response::Cell(ref report) if report.value == CellValue::Number(8.0) && report.a1.as_deref() == Some("B1"))
    );

    let cells = service
        .handle(Request::Cells {
            path: path.clone(),
            branch: None,
            sheet: sheet.clone(),
            start: 0,
            limit: Some(1),
        })
        .unwrap();
    let Response::Cells(page) = cells else {
        panic!("{cells:?}")
    };
    assert_eq!(page.total, 2);
    assert_eq!(page.cells.len(), 1);
    assert_eq!(page.next, Some(1));
    let cells = service
        .handle(Request::Cells {
            path: path.clone(),
            branch: None,
            sheet: sheet.clone(),
            start: 1,
            limit: None,
        })
        .unwrap();
    let Response::Cells(page) = cells else {
        panic!("{cells:?}")
    };
    assert_eq!(page.cells.len(), 1);
    assert_eq!(page.next, None);
    assert!(matches!(
        service.handle(Request::Cells { path: path.clone(), branch: None, sheet: sheet.clone(), start: 0, limit: Some(0) }),
        Err(ServiceError { ref code, .. }) if code == "invalid_limit"
    ));

    let grid_page = service
        .handle(Request::GridPage {
            path: path.clone(),
            branch: None,
            sheet: sheet.clone(),
            row_start: 0,
            column_start: 0,
            rows: 2,
            columns: 2,
        })
        .unwrap();
    let Response::GridPage(grid_page) = grid_page else {
        panic!("{grid_page:?}")
    };
    assert_eq!((grid_page.rows, grid_page.columns), (2, 2));
    assert_eq!(grid_page.cells.len(), 2);
    assert_eq!((grid_page.cells[0].row, grid_page.cells[0].column), (0, 0));
    assert_eq!(grid_page.cells[0].value, CellValue::Number(4.0));
    assert_eq!((grid_page.cells[1].row, grid_page.cells[1].column), (0, 1));
    assert_eq!(grid_page.cells[1].value, CellValue::Number(8.0));
    assert_eq!(grid_page.cells[1].formula.as_deref(), Some("=A1*2"));
    assert!(matches!(
        service.handle(Request::GridPage {
            path: path.clone(),
            branch: None,
            sheet: sheet.clone(),
            row_start: 0,
            column_start: 0,
            rows: 101,
            columns: 100,
        }),
        Err(ServiceError { ref code, .. }) if code == "invalid_grid_page"
    ));

    let lineage = service
        .handle(Request::Lineage {
            path: path.clone(),
            branch: None,
            sheet: sheet.clone(),
            a1: "B1".into(),
        })
        .unwrap();
    assert!(matches!(lineage, Response::Lineage(Some(ref lineage)) if lineage.inputs.len() == 1));

    // An agent cannot touch main, but can work on its own branch.
    let refused = append(
        &mut service,
        &path,
        None,
        agent("planner"),
        Command::SetValue {
            sheet: sheet_id,
            a1: "A1".into(),
            value: Literal::Number(1.0),
        },
    );
    assert!(matches!(refused, Err(ServiceError { ref code, .. }) if code == "agent_on_main"));
    service
        .handle(Request::Branch {
            path: path.clone(),
            name: "agent-work".into(),
            from: None,
            actor: agent("planner"),
        })
        .unwrap();
    append(
        &mut service,
        &path,
        Some("agent-work"),
        agent("planner"),
        Command::SetValue {
            sheet: sheet_id,
            a1: "A1".into(),
            value: Literal::Number(5.0),
        },
    )
    .unwrap();
    let branch_cell = service
        .handle(Request::Cell {
            path: path.clone(),
            branch: Some("agent-work".into()),
            sheet: sheet.clone(),
            a1: "B1".into(),
        })
        .unwrap();
    assert!(
        matches!(branch_cell, Response::Cell(ref report) if report.value == CellValue::Number(10.0))
    );
    let main_cell = service
        .handle(Request::Cell {
            path: path.clone(),
            branch: None,
            sheet: sheet.clone(),
            a1: "B1".into(),
        })
        .unwrap();
    assert!(
        matches!(main_cell, Response::Cell(ref report) if report.value == CellValue::Number(8.0))
    );

    // The check reads A1 as a truth value: a number is not TRUE, so it fails
    // on both branches, and the merge is refused until it passes.
    let checked = service
        .handle(Request::Check {
            path: path.clone(),
            branch: Some("agent-work".into()),
        })
        .unwrap();
    assert!(matches!(checked, Response::Checked { passed: false, .. }));
    let refused = service.handle(Request::Merge {
        path: path.clone(),
        source: "agent-work".into(),
        target: None,
        approver: human("tom"),
    });
    assert!(
        matches!(refused, Err(ServiceError { ref code, ref details, .. }) if code == "checks_failed" && details.is_some())
    );
    let refused = service.handle(Request::Merge {
        path: path.clone(),
        source: "agent-work".into(),
        target: None,
        approver: agent("planner"),
    });
    assert!(matches!(refused, Err(ServiceError { ref code, .. }) if code == "unauthorized"));

    append(
        &mut service,
        &path,
        Some("agent-work"),
        agent("planner"),
        Command::SetValue {
            sheet: sheet_id,
            a1: "A1".into(),
            value: Literal::Boolean(true),
        },
    )
    .unwrap();
    let diff = service
        .handle(Request::Diff {
            path: path.clone(),
            source: "agent-work".into(),
            target: None,
        })
        .unwrap();
    let Response::Diff(diff) = diff else {
        panic!("{diff:?}")
    };
    assert_eq!(diff.source_operations.len(), 2);
    assert!(diff.conflicts.is_empty());
    let merged = service
        .handle(Request::Merge {
            path: path.clone(),
            source: "agent-work".into(),
            target: None,
            approver: human("tom"),
        })
        .unwrap();
    assert!(matches!(merged, Response::Merged(ref report) if report.replayed.len() == 2));
    let main_cell = service
        .handle(Request::Cell {
            path: path.clone(),
            branch: None,
            sheet: sheet.clone(),
            a1: "A1".into(),
        })
        .unwrap();
    assert!(
        matches!(main_cell, Response::Cell(ref report) if report.value == CellValue::Boolean(true))
    );

    // Close, reopen through a fresh service: the same state comes back.
    assert!(matches!(
        service
            .handle(Request::Close { path: path.clone() })
            .unwrap(),
        Response::Closed
    ));
    assert!(matches!(
        service.handle(Request::Close { path: path.clone() }),
        Err(ServiceError { ref code, .. }) if code == "not_open"
    ));
    let mut reopened = Service::default();
    let opened = reopened
        .handle(Request::Open { path: path.clone() })
        .unwrap();
    assert!(
        matches!(opened, Response::Opened { ref branches, .. } if branches == &["agent-work".to_string(), "main".to_string()] || branches == &["main".to_string(), "agent-work".to_string()])
    );
    let summary = reopened
        .handle(Request::Document {
            path: path.clone(),
            branch: None,
        })
        .unwrap();
    let Response::Document(summary) = summary else {
        panic!("{summary:?}")
    };
    assert_eq!(summary.sheets[0].name, "Model");
    assert_eq!(summary.sheets[0].cells, 2);
    assert_eq!(summary.sheets[0].column_types.len(), 2);
    assert_eq!(summary.sheets[0].column_types[0].position, 0);
    assert_eq!(summary.sheets[0].column_types[0].declared, ColumnType::Any);
    assert_eq!(
        summary.sheets[0].column_types[0].inferred,
        InferredColumnType::Boolean
    );
    assert_eq!(
        summary.sheets[0].column_types[1].inferred,
        InferredColumnType::Number
    );
    assert_eq!(summary.checks, 1);
    assert!(summary.load.is_some());
    reopened.close_all().unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

#[test]
fn csv_export_is_bounded_disclosed_and_never_overwrites() {
    let path = temp_document();
    let output = path.with_extension("csv");
    let mut service = Service::new(|| 1);
    service
        .handle(Request::Create {
            path: path.clone(),
            name: "Export fixture".into(),
            actor: human("tom"),
        })
        .unwrap();
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::AddSheet {
            name: "Forecast".into(),
        },
    )
    .unwrap();
    let sheet = sheet_of(&mut service, &path, None);
    let sheet_id = omasheets_core::SheetId(omasheets_core::ObjectId::parse(&sheet).unwrap());
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::AddColumns {
            sheet: sheet_id,
            count: 4,
            at: 0,
        },
    )
    .unwrap();
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::AddRows {
            sheet: sheet_id,
            count: 2,
            at: 0,
            table: None,
        },
    )
    .unwrap();
    for (a1, value) in [
        ("A1", Literal::Text("hello, \"world\"".into())),
        ("C1", Literal::Number(2.0)),
        ("A2", Literal::Text("=not-a-formula".into())),
        ("B2", Literal::Boolean(true)),
    ] {
        append(
            &mut service,
            &path,
            None,
            human("tom"),
            Command::SetValue {
                sheet: sheet_id,
                a1: a1.into(),
                value,
            },
        )
        .unwrap();
    }
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::SetFormula {
            sheet: sheet_id,
            a1: "D1".into(),
            source: "=C1*2".into(),
        },
    )
    .unwrap();

    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::AddSheet {
            name: "Inputs".into(),
        },
    )
    .unwrap();
    let inputs = match service
        .handle(Request::Document {
            path: path.clone(),
            branch: None,
        })
        .unwrap()
    {
        Response::Document(summary) => summary.sheets[1].id,
        other => panic!("{other:?}"),
    };
    for command in [
        Command::AddColumns {
            sheet: inputs,
            count: 1,
            at: 0,
        },
        Command::AddRows {
            sheet: inputs,
            count: 1,
            at: 0,
            table: None,
        },
        Command::SetValue {
            sheet: inputs,
            a1: "A1".into(),
            value: Literal::Number(5.0),
        },
    ] {
        append(&mut service, &path, None, human("tom"), command).unwrap();
    }
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::SetFormula {
            sheet: sheet_id,
            a1: "D2".into(),
            source: "=Inputs!A1".into(),
        },
    )
    .unwrap();
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::RenameSheet {
            sheet: inputs,
            name: "Renamed & safe".into(),
        },
    )
    .unwrap();

    let exported = service
        .handle(Request::ExportCsv {
            path: path.clone(),
            branch: None,
            sheet: sheet.clone(),
            output: output.clone(),
        })
        .unwrap();
    let Response::ExportedCsv(manifest) = exported else {
        panic!("{exported:?}")
    };
    assert_eq!(manifest.format, "csv-rfc4180");
    assert_eq!(manifest.branch, "main");
    assert_eq!(manifest.sheet, sheet_id);
    assert_eq!(manifest.sheet_name, "Forecast");
    assert_eq!((manifest.rows, manifest.columns), (2, 4));
    assert_eq!(manifest.formula_cells, 2);
    assert_eq!(manifest.potential_formula_injection_cells, 1);
    assert_eq!(manifest.limitations.len(), 3);
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        "\"hello, \"\"world\"\"\",,2,4\r\n=not-a-formula,TRUE,,5"
    );

    let refused = service.handle(Request::ExportCsv {
        path: path.clone(),
        branch: None,
        sheet,
        output: output.clone(),
    });
    assert!(matches!(refused, Err(ServiceError { ref code, .. }) if code == "output_exists"));
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        "\"hello, \"\"world\"\"\",,2,4\r\n=not-a-formula,TRUE,,5"
    );

    let xlsx = path.with_extension("projected.xlsx");
    let exported = service
        .handle(Request::ExportXlsx {
            path: path.clone(),
            branch: None,
            output: xlsx.clone(),
        })
        .unwrap();
    let Response::ExportedXlsx(manifest) = exported else {
        panic!("{exported:?}")
    };
    assert_eq!(manifest.format, "xlsx-2007");
    assert_eq!(manifest.branch, "main");
    assert_eq!(manifest.sheets.len(), 2);
    assert_eq!(manifest.formula_cells, 2);
    assert_eq!(manifest.formula_cells_preserved, 1);
    assert_eq!(manifest.formula_cells_flattened, 1);
    assert_eq!(manifest.limitations.len(), 4);
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&xlsx).unwrap()).unwrap();
    let mut workbook = String::new();
    archive
        .by_name("xl/workbook.xml")
        .unwrap()
        .read_to_string(&mut workbook)
        .unwrap();
    assert!(workbook.contains("Renamed &amp; safe"));
    let mut worksheet = String::new();
    archive
        .by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut worksheet)
        .unwrap();
    assert!(worksheet.contains("<f>C1*2</f><v>4</v>"));
    assert!(worksheet.contains("<c r=\"D2\"><v>5</v></c>"));
    assert!(!worksheet.contains("Inputs!A1"));
    drop(archive);
    let refused = service.handle(Request::ExportXlsx {
        path: path.clone(),
        branch: None,
        output: xlsx.clone(),
    });
    assert!(matches!(
        refused,
        Err(ServiceError { ref code, .. }) if code == "output_exists"
    ));

    let parquet = path.with_extension("parquet");
    let exported = service
        .handle(Request::ExportParquet {
            path: path.clone(),
            branch: None,
            sheet: sheet_id.to_string(),
            output: parquet.clone(),
        })
        .unwrap();
    let Response::ExportedParquet(manifest) = exported else {
        panic!("{exported:?}")
    };
    assert_eq!(manifest.format, "parquet-arrow-58.4");
    assert_eq!(manifest.branch, "main");
    assert_eq!(manifest.sheet, sheet_id);
    assert_eq!(manifest.sheet_name, "Forecast");
    assert_eq!(manifest.rows, 2);
    assert_eq!(manifest.columns.len(), 4);
    assert_eq!(manifest.columns[0].inferred, InferredColumnType::Text);
    assert_eq!(manifest.columns[1].inferred, InferredColumnType::Boolean);
    assert_eq!(manifest.columns[2].inferred, InferredColumnType::Number);
    assert_eq!(manifest.formula_cells, 2);
    assert_eq!(manifest.error_cells_as_null, 0);
    let mut reader =
        ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&parquet).unwrap())
            .unwrap()
            .build()
            .unwrap();
    let batch = reader.next().unwrap().unwrap();
    assert_eq!(batch.num_rows(), 2);
    assert_eq!(batch.schema().field(0).name(), "A");
    let text = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(text.value(0), "hello, \"world\"");
    let flags = batch
        .column(1)
        .as_any()
        .downcast_ref::<BooleanArray>()
        .unwrap();
    assert!(flags.is_null(0));
    assert!(flags.value(1));
    let numbers = batch
        .column(2)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    assert_eq!(numbers.value(0), 2.0);
    assert!(numbers.is_null(1));
    assert!(reader.next().is_none());

    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::SetValue {
            sheet: sheet_id,
            a1: "A1".into(),
            value: Literal::Text("🧪".repeat(omasheets_core::MAX_TEXT_CHARS)),
        },
    )
    .unwrap();
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::SetFormula {
            sheet: sheet_id,
            a1: "B1".into(),
            source: "=A1&A1&A1&A1&A1&A1&A1&A1".into(),
        },
    )
    .unwrap();
    let oversized = path.with_extension("oversized.csv");
    let refused = service.handle(Request::ExportCsv {
        path: path.clone(),
        branch: None,
        sheet: sheet_id.to_string(),
        output: oversized.clone(),
    });
    assert!(matches!(refused, Err(ServiceError { ref code, .. }) if code == "field_too_large"));
    assert!(!oversized.exists(), "failed export must not leave output");
    let part_prefix = format!(".{}.", oversized.file_name().unwrap().to_string_lossy());
    assert!(
        std::fs::read_dir(oversized.parent().unwrap())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(&part_prefix)),
        "failed export must clean its temporary file"
    );

    service.close_all().unwrap();
    let _ = std::fs::remove_file(output);
    let _ = std::fs::remove_file(xlsx);
    let _ = std::fs::remove_file(parquet);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

#[test]
fn parquet_export_refuses_mixed_columns_without_creating_output() {
    let path = temp_document();
    let output = path.with_extension("parquet");
    let mut service = Service::new(|| 1);
    service
        .handle(Request::Create {
            path: path.clone(),
            name: "Mixed export".into(),
            actor: human("tom"),
        })
        .unwrap();
    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::AddSheet {
            name: "Data".into(),
        },
    )
    .unwrap();
    let sheet = match service
        .handle(Request::Document {
            path: path.clone(),
            branch: None,
        })
        .unwrap()
    {
        Response::Document(summary) => summary.sheets[0].id,
        other => panic!("{other:?}"),
    };
    for command in [
        Command::AddColumns {
            sheet,
            count: 1,
            at: 0,
        },
        Command::AddRows {
            sheet,
            count: 2,
            at: 0,
            table: None,
        },
        Command::SetValue {
            sheet,
            a1: "A1".into(),
            value: Literal::Number(1.0),
        },
        Command::SetValue {
            sheet,
            a1: "A2".into(),
            value: Literal::Text("one".into()),
        },
    ] {
        append(&mut service, &path, None, human("tom"), command).unwrap();
    }
    let refused = service.handle(Request::ExportParquet {
        path: path.clone(),
        branch: None,
        sheet: sheet.to_string(),
        output: output.clone(),
    });
    assert!(matches!(
        refused,
        Err(ServiceError { ref code, .. }) if code == "mixed_column"
    ));
    assert!(!output.exists());
    service.close_all().unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

#[test]
fn xlsx_export_rejects_unrepresentable_workbooks_before_creating_output() {
    let path = temp_document();
    let output = path.with_extension("xlsx");
    let mut service = Service::new(|| 1);
    service
        .handle(Request::Create {
            path: path.clone(),
            name: "Export boundary".into(),
            actor: human("tom"),
        })
        .unwrap();
    let empty = service.handle(Request::ExportXlsx {
        path: path.clone(),
        branch: None,
        output: output.clone(),
    });
    assert!(matches!(
        empty,
        Err(ServiceError { ref code, .. }) if code == "empty_workbook"
    ));
    assert!(!output.exists());

    append(
        &mut service,
        &path,
        None,
        human("tom"),
        Command::AddSheet {
            name: "Not/Excel".into(),
        },
    )
    .unwrap();
    let invalid_name = service.handle(Request::ExportXlsx {
        path: path.clone(),
        branch: None,
        output: output.clone(),
    });
    assert!(matches!(
        invalid_name,
        Err(ServiceError { ref code, .. }) if code == "unsupported_sheet_name"
    ));
    assert!(!output.exists());

    service.close_all().unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

#[test]
fn xlsx_import_is_bounded_replayable_and_never_overwrites() {
    let source = temp_xlsx();
    let output = temp_document();
    let mut service = Service::new(|| 42);
    let response = service
        .handle(Request::ImportXlsx {
            source: source.clone(),
            output: output.clone(),
            actor: human("tom"),
            name: Some("Imported plan".into()),
        })
        .unwrap();
    let Response::ImportedXlsx(manifest) = response else {
        panic!("{response:?}")
    };
    assert_eq!(manifest.format, "omasheets-native-v1");
    assert_eq!(manifest.date_system, "1900");
    assert_eq!(manifest.sheets.len(), 1);
    assert_eq!(
        (manifest.sheets[0].rows, manifest.sheets[0].columns),
        (3, 2)
    );
    assert_eq!(manifest.occupied_rectangle_cells, 6);
    assert_eq!(manifest.value_cells_imported, 5);
    assert_eq!(manifest.formula_cells_observed, 2);
    assert_eq!(manifest.formula_cells_native, 1);
    assert_eq!(manifest.formula_cells_cached_only, 1);
    assert_eq!(manifest.formula_cells_omitted, 0);
    assert_eq!(manifest.owned_engine_unsupported_formulas, 1);
    assert_eq!(manifest.error_cells_omitted, 0);
    assert_eq!(manifest.rejected_value_cells_omitted, 0);
    assert_eq!(manifest.skipped_source_sheets, 0);
    assert_eq!(manifest.limitations.len(), 4);

    let digest = manifest.document_digest.clone();
    let sheet = manifest.sheets[0].id.to_string();
    assert!(matches!(
        service
            .handle(Request::Cell {
                path: output.clone(),
                branch: None,
                sheet: sheet.clone(),
                a1: "A3".into(),
            })
            .unwrap(),
        Response::Cell(ref cell) if cell.value == CellValue::Number(5.0)
    ));
    assert!(matches!(
        service
            .handle(Request::Cell {
                path: output.clone(),
                branch: None,
                sheet,
                a1: "B2".into(),
            })
            .unwrap(),
        Response::Cell(ref cell) if cell.value == CellValue::Number(9.0)
    ));
    service.close_all().unwrap();

    let reopened = service
        .handle(Request::Document {
            path: output.clone(),
            branch: None,
        })
        .unwrap();
    assert!(matches!(reopened, Response::Document(ref summary) if summary.digest == digest));
    let refused = service.handle(Request::ImportXlsx {
        source: source.clone(),
        output: output.clone(),
        actor: human("tom"),
        name: None,
    });
    assert!(matches!(refused, Err(ServiceError { ref code, .. }) if code == "output_exists"));
    service.close_all().unwrap();
    let _ = std::fs::remove_file(source);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", output.display()));
    }
}

#[test]
fn requests_and_responses_round_trip_as_tagged_json() {
    let request = Request::Append {
        path: "/tmp/x.omasheets".into(),
        branch: None,
        actor: human("tom"),
        command: Command::SetFormula {
            sheet: omasheets_core::SheetId(omasheets_core::ObjectId::from_seed("s")),
            a1: "A1".into(),
            source: "=1+1".into(),
        },
    };
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("\"kind\":\"append\""));
    assert!(json.contains("\"command\":\"set_formula\""));
    assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), request);
    let minimal: Request =
        serde_json::from_str(r#"{"kind":"document","path":"/tmp/x.omasheets"}"#).unwrap();
    assert!(matches!(minimal, Request::Document { branch: None, .. }));
    let error = ServiceError {
        code: "unknown_branch".into(),
        message: "no".into(),
        details: None,
    };
    assert_eq!(
        serde_json::to_string(&error).unwrap(),
        r#"{"code":"unknown_branch","message":"no"}"#
    );
}

#[test]
fn bad_paths_and_actors_are_refused_before_any_file_is_touched() {
    let mut service = Service::default();
    assert!(matches!(
        service.handle(Request::Open { path: "/nonexistent/dir/x.omasheets".into() }),
        Err(ServiceError { ref code, .. }) if code == "invalid_path"
    ));
    let path = temp_document();
    assert!(matches!(
        service.handle(Request::Create { path: path.clone(), name: "x".into(), actor: human("   ") }),
        Err(ServiceError { ref code, .. }) if code == "invalid_actor"
    ));
    assert!(!path.exists());
    assert!(matches!(
        service.handle(Request::Open { path: path.clone() }),
        Err(ServiceError { ref code, .. }) if code == "not_a_document"
    ));
}
