use omasheets_core::CellValue;
use omasheets_kit::{KitError, WorkbookSession, import_xlsx, probe};
use omasheets_service::spreadsheet::{Action, Rect};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "omasheets-kit-tests-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_xlsx(
    path: &Path,
    cells: &str,
    workbook_extras: &str,
    sheet_extras: &str,
    extra_parts: &[(&str, &str)],
) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    for (name, body) in [
        ("[Content_Types].xml", "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>".to_owned()),
        ("_rels/.rels", "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>".to_owned()),
        ("xl/workbook.xml", format!("<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><workbookPr date1904=\"0\"/><sheets><sheet name=\"Data\" sheetId=\"1\" r:id=\"rId1\"/></sheets>{workbook_extras}<calcPr calcMode=\"auto\"/></workbook>")),
        ("xl/_rels/workbook.xml.rels", "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>".to_owned()),
        ("xl/worksheets/sheet1.xml", format!("<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{cells}</sheetData>{sheet_extras}</worksheet>")),
    ].into_iter().chain(extra_parts.iter().map(|(name,body)| (*name,body.to_string()))) {
        zip.start_file(name, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

const CELLS: &str = "<row r=\"1\"><c r=\"A1\"><v>3</v></c><c r=\"B1\"><f>A1*3</f><v>9</v></c></row><row r=\"2\"><c r=\"A2\"><v>1</v></c></row><row r=\"3\"><c r=\"A3\"><v>2</v></c><c r=\"B3\"><f>SUM(A1:A2)</f><v>4</v></c></row>";

#[test]
fn cli_probe_exit_status_matches_workbook_admission() {
    let fixture = Fixture::new();
    for (name, extras, expected) in [
        ("supported.xlsx", "", true),
        (
            "defined-name.xlsx",
            "<definedNames><definedName name=\"Budget\">Data!$A$1</definedName></definedNames>",
            false,
        ),
    ] {
        let source = fixture.path(name);
        write_xlsx(&source, CELLS, extras, "", &[]);
        let bytes = fs::read(&source).unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_omasheets-kit"))
            .arg("probe")
            .arg(&source)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(if expected { 0 } else { 2 }));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["can_import"], expected);
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }
}

fn number(session: &mut WorkbookSession, sheet: &str, row: usize, column: usize) -> f64 {
    let page = session.viewport(sheet, row, column, 1, 1).unwrap();
    match page.cells[0].value {
        CellValue::Number(number) => number,
        ref value => panic!("Expected number, got {value:?}"),
    }
}

#[test]
fn opens_edits_recalculates_saves_and_reopens_without_libreoffice() {
    let f = Fixture::new();
    let source = f.path("source.xlsx");
    let working = f.path("working.omasheets");
    let saved = f.path("edited.xlsx");
    write_xlsx(&source, CELLS, "", "", &[]);
    let original = fs::read(&source).unwrap();
    let report = probe(&source).unwrap();
    assert!(report.can_import, "{:?}", report.reasons);
    assert_eq!(
        report
            .import_manifest
            .as_ref()
            .unwrap()
            .formula_cells_native,
        2
    );
    assert!(!working.exists());

    let mut session =
        WorkbookSession::open_xlsx_checked(&source, &working, report.source_sha256.as_deref())
            .unwrap();
    let sheet = session.sheets().unwrap()[0].id.to_string();
    assert_eq!(number(&mut session, &sheet, 0, 1), 9.0);
    let revision = session.revision().unwrap();
    let edit = session
        .edit(
            &sheet,
            &revision,
            Action::SetCells {
                row: 0,
                column: 0,
                values: vec![vec!["5".into()]],
            },
        )
        .unwrap();
    assert_ne!(edit.revision, revision);
    assert_eq!(number(&mut session, &sheet, 0, 1), 15.0);
    assert_eq!(number(&mut session, &sheet, 2, 1), 6.0);
    assert!(session.is_dirty());
    let digest = session.snapshot().unwrap();
    assert!(
        session.is_dirty(),
        "A recovery snapshot must not clear the export-dirty state"
    );
    let manifest = session.save_xlsx_copy(&saved).unwrap();
    assert_eq!(manifest.document_digest, digest);
    assert_eq!(manifest.formula_cells_preserved, 2);
    assert_eq!(manifest.formula_cells_flattened, 0);
    assert!(!session.is_dirty());
    assert_eq!(fs::read(&source).unwrap(), original);
    session.close().unwrap();

    let mut reopened = WorkbookSession::open_native(&working).unwrap();
    assert_eq!(reopened.snapshot().unwrap(), digest);
    assert_eq!(number(&mut reopened, &sheet, 0, 1), 15.0);
    reopened.close().unwrap();
    let original_native = fs::read(&working).unwrap();
    assert!(probe(&working).unwrap().can_import);
    assert_eq!(
        fs::read(&working).unwrap(),
        original_native,
        "Read-only native probe must not checkpoint the source"
    );
    assert!(!f.path("working.omasheets-wal").exists());
    assert!(!f.path("working.omasheets-shm").exists());
    let mut saved_session = WorkbookSession::open_xlsx(&saved, f.path("saved.omasheets")).unwrap();
    let saved_sheet = saved_session.sheets().unwrap()[0].id.to_string();
    assert_eq!(number(&mut saved_session, &saved_sheet, 0, 1), 15.0);
    // Verify the emitted ZIP independently of the native conversion: formulas
    // and cached results must both be present in the actual package.
    let mut archive = zip::ZipArchive::new(File::open(&saved).unwrap()).unwrap();
    let mut sheet_xml = String::new();
    archive
        .by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut sheet_xml)
        .unwrap();
    assert!(sheet_xml.contains("<f>A1*3</f><v>15</v>"));
    assert!(sheet_xml.contains("<f>SUM(A1:A2)</f><v>6</v>"));
}

#[test]
fn revision_guard_undo_redo_and_no_clobber_preserve_saved_state() {
    let f = Fixture::new();
    let source = f.path("source.xlsx");
    write_xlsx(&source, CELLS, "", "", &[]);
    let working = f.path("working.omasheets");
    let mut session = WorkbookSession::open_xlsx(&source, &working).unwrap();
    let sheet = session.sheets().unwrap()[0].id.to_string();
    let old = session.revision().unwrap();
    session
        .edit(
            &sheet,
            &old,
            Action::SetCells {
                row: 0,
                column: 0,
                values: vec![vec!["8".into()]],
            },
        )
        .unwrap();
    let revision = session.revision().unwrap();
    assert!(
        session
            .edit(
                &sheet,
                &old,
                Action::SetCells {
                    row: 0,
                    column: 0,
                    values: vec![vec!["99".into()]]
                }
            )
            .is_err()
    );
    assert_eq!(session.revision().unwrap(), revision);
    assert_eq!(number(&mut session, &sheet, 0, 1), 24.0);
    let revision = session.undo(&revision).unwrap();
    assert_eq!(number(&mut session, &sheet, 0, 1), 9.0);
    session.redo(&revision).unwrap();
    assert_eq!(number(&mut session, &sheet, 0, 1), 24.0);
    let saved = f.path("saved.xlsx");
    fs::write(&saved, b"existing workbook").unwrap();
    assert!(session.save_xlsx_copy(&saved).is_err());
    assert!(session.is_dirty());
    assert_eq!(fs::read(&saved).unwrap(), b"existing workbook");
    session.close().unwrap();
    let before = fs::read(&working).unwrap();
    assert!(import_xlsx(&source, &working).is_err());
    assert_eq!(fs::read(&working).unwrap(), before);
}

#[test]
fn unsupported_formula_is_reported_and_never_published_as_cached_literal() {
    let f = Fixture::new();
    let source = f.path("source.xlsx");
    write_xlsx(
        &source,
        "<row r=\"1\"><c r=\"A1\"><f>UNIMPLEMENTED(3)</f><v>7</v></c></row>",
        "",
        "",
        &[],
    );
    let bytes = fs::read(&source).unwrap();
    let report = probe(&source).unwrap();
    assert!(!report.can_import);
    assert_eq!(report.import_manifest.unwrap().formula_cells_cached_only, 1);
    let working = f.path("blocked.omasheets");
    assert!(matches!(
        import_xlsx(&source, &working),
        Err(KitError::Admission(_))
    ));
    assert!(!working.exists());
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert!(fs::read_dir(&f.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".omasheets-kit-")
    }));
}

#[test]
fn unknown_objects_names_errors_and_validation_are_refused_before_conversion() {
    let f = Fixture::new();
    for (index, workbook, sheet, parts, cells) in [
        (
            0,
            "<definedNames><definedName name=\"Rate\">Data!$A$1</definedName></definedNames>",
            "",
            vec![],
            CELLS,
        ),
        (
            1,
            "",
            "<dataValidations count=\"1\"><dataValidation type=\"whole\" sqref=\"A1\"/></dataValidations>",
            vec![],
            CELLS,
        ),
        (
            2,
            "",
            "",
            vec![("xl/pivotTables/pivotTable1.xml", "<pivotTableDefinition/>")],
            CELLS,
        ),
        (
            3,
            "",
            "",
            vec![],
            "<row r=\"1\"><c r=\"A1\" t=\"e\"><v>#DIV/0!</v></c></row>",
        ),
        (
            4,
            "",
            "<extLst><x:payload xmlns:x=\"https://example.invalid/unknown\"/></extLst>",
            vec![],
            CELLS,
        ),
    ] {
        let source = f.path(&format!("blocked-{index}.xlsx"));
        write_xlsx(&source, cells, workbook, sheet, &parts);
        let report = probe(&source).unwrap();
        assert!(!report.can_import, "{index}");
        assert!(!report.reasons.is_empty());
        assert!(
            report.import_manifest.is_none(),
            "Metadata losses should be rejected before creating even a staged conversion"
        );
        let output = f.path(&format!("blocked-{index}.omasheets"));
        assert!(import_xlsx(&source, &output).is_err());
        assert!(!output.exists());
    }
}

#[test]
fn legacy_formats_and_source_hash_changes_are_explicit_refusals() {
    let f = Fixture::new();
    for extension in ["xls", "xlsm", "ods"] {
        let source = f.path(&format!("unsupported.{extension}"));
        fs::write(&source, b"legacy placeholder").unwrap();
        let report = probe(&source).unwrap();
        assert_eq!(report.format, extension);
        assert!(!report.can_import);
        assert!(report.reasons[0].contains("not implemented"));
    }
    let source = f.path("source.xlsx");
    write_xlsx(&source, CELLS, "", "", &[]);
    let report = probe(&source).unwrap();
    write_xlsx(&source, &CELLS.replace("<v>3</v>", "<v>6</v>"), "", "", &[]);
    let output = f.path("changed.omasheets");
    assert!(
        WorkbookSession::open_xlsx_checked(&source, &output, report.source_sha256.as_deref())
            .is_err()
    );
    assert!(!output.exists());
}

#[test]
fn copy_refuses_unrepresentable_stable_formula_bindings_before_writing() {
    let f = Fixture::new();
    let source = f.path("source.xlsx");
    write_xlsx(&source, CELLS, "", "", &[]);
    let mut session = WorkbookSession::open_xlsx(&source, f.path("working.omasheets")).unwrap();
    let sheet = session.sheets().unwrap()[0].id.to_string();
    let revision = session.revision().unwrap();
    session
        .edit(
            &sheet,
            &revision,
            Action::Sort {
                range: Rect {
                    row: 0,
                    column: 0,
                    rows: 3,
                    columns: 1,
                },
                column: 0,
                descending: false,
                header: false,
            },
        )
        .unwrap();
    let copy = f.path("unrepresentable.xlsx");
    assert!(session.save_xlsx_copy(&copy).is_err());
    assert!(!copy.exists());
    assert!(session.is_dirty());
}

#[test]
fn malformed_utf8_rgb_is_a_refusal_without_panicking() {
    let f = Fixture::new();
    let source = f.path("malformed-rgb.xlsx");
    write_xlsx(
        &source,
        CELLS,
        "",
        "",
        &[(
            "xl/styles.xml",
            "<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><fonts count=\"1\"><font><color rgb=\"😀abcd\"/></font></fonts></styleSheet>",
        )],
    );
    let report = probe(&source).unwrap();
    assert!(!report.can_import);
    assert!(
        report
            .reasons
            .iter()
            .any(|reason| reason.contains("ASCII hexadecimal"))
    );
}

fn styles_xml(fonts: &str, font_count: usize, cell_xfs: &str, xf_count: usize) -> String {
    format!(
        "<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><fonts count=\"{font_count}\">{fonts}</fonts><fills count=\"1\"><fill><patternFill patternType=\"none\"/></fill></fills><borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs><cellXfs count=\"{xf_count}\">{cell_xfs}</cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>"
    )
}

const DEFAULT_XF: &str = "<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>";
const DEFAULT_FONT: &str = "<font><name val=\"Calibri\"/><sz val=\"11\"/></font>";

#[test]
fn implicit_custom_style_zero_is_refused_and_explicit_style_is_preserved() {
    let f = Fixture::new();
    let styles = styles_xml(
        "<font><name val=\"Calibri\"/><sz val=\"11\"/><b/></font>",
        1,
        DEFAULT_XF,
        1,
    );
    let implicit = f.path("implicit-bold.xlsx");
    write_xlsx(&implicit, CELLS, "", "", &[("xl/styles.xml", &styles)]);
    let original = fs::read(&implicit).unwrap();
    let report = probe(&implicit).unwrap();
    assert!(!report.can_import);
    assert!(
        report
            .reasons
            .iter()
            .any(|reason| reason.contains("Implicit source cell style 0"))
    );
    let working = f.path("refused.omasheets");
    assert!(matches!(
        import_xlsx(&implicit, &working),
        Err(KitError::Admission(_))
    ));
    assert!(!working.exists());
    assert_eq!(fs::read(&implicit).unwrap(), original);

    let explicit = f.path("explicit-bold.xlsx");
    let explicit_cells = CELLS.replace("<c r=", "<c s=\"0\" r=");
    write_xlsx(
        &explicit,
        &explicit_cells,
        "",
        "",
        &[("xl/styles.xml", &styles)],
    );
    let report = probe(&explicit).unwrap();
    assert!(report.can_import, "{:?}", report.reasons);
    let mut session = WorkbookSession::open_xlsx(&explicit, f.path("bold.omasheets")).unwrap();
    let sheet = session.sheets().unwrap()[0].id.to_string();
    assert!(
        session.viewport(&sheet, 0, 0, 1, 1).unwrap().cells[0]
            .style
            .bold
    );
    let copy = f.path("bold-copy.xlsx");
    session.save_xlsx_copy(&copy).unwrap();
    session.close().unwrap();
    let mut reopened = WorkbookSession::open_xlsx(&copy, f.path("bold-copy.omasheets")).unwrap();
    let sheet = reopened.sheets().unwrap()[0].id.to_string();
    assert!(
        reopened.viewport(&sheet, 0, 0, 1, 1).unwrap().cells[0]
            .style
            .bold
    );
}

#[test]
fn default_theme_text_is_admitted_but_custom_and_unknown_theme_text_is_refused() {
    let f = Fixture::new();
    let styles = styles_xml(
        "<font><name val=\"Calibri\"/><sz val=\"11\"/><color theme=\"1\"/><scheme val=\"minor\"/></font>",
        1,
        DEFAULT_XF,
        1,
    );
    let default_theme = "<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"Office\"><a:themeElements><a:clrScheme name=\"Office\"><a:dk1><a:sysClr val=\"windowText\" lastClr=\"000000\"/></a:dk1></a:clrScheme><a:fontScheme name=\"Office\"><a:minorFont><a:latin typeface=\"Calibri\"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>";
    let default = f.path("default-theme.xlsx");
    write_xlsx(
        &default,
        CELLS,
        "",
        "",
        &[
            ("xl/styles.xml", &styles),
            ("xl/theme/theme1.xml", default_theme),
        ],
    );
    let report = probe(&default).unwrap();
    assert!(report.can_import, "{:?}", report.reasons);
    let mut session =
        WorkbookSession::open_xlsx(&default, f.path("default-theme.omasheets")).unwrap();
    let sheet = session.sheets().unwrap()[0].id.to_string();
    assert_eq!(number(&mut session, &sheet, 0, 1), 9.0);
    session.close().unwrap();

    for (index, theme) in [
        Some(default_theme.replace("lastClr=\"000000\"", "lastClr=\"FF0000\"")),
        Some(default_theme.replace("typeface=\"Calibri\"", "typeface=\"Papyrus\"")),
        Some(default_theme.replace(
            "http://schemas.openxmlformats.org/drawingml/2006/main",
            "https://example.invalid/unknown-theme",
        )),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        let source = f.path(&format!("unsupported-theme-{index}.xlsx"));
        let mut parts = vec![("xl/styles.xml", styles.as_str())];
        if let Some(theme) = theme.as_deref() {
            parts.push(("xl/theme/theme1.xml", theme));
        }
        write_xlsx(&source, CELLS, "", "", &parts);
        let original = fs::read(&source).unwrap();
        let report = probe(&source).unwrap();
        assert!(!report.can_import, "{index}");
        assert!(
            report
                .reasons
                .iter()
                .any(|reason| reason.to_ascii_lowercase().contains("theme")),
            "{:?}",
            report.reasons
        );
        let working = f.path(&format!("unsupported-theme-{index}.omasheets"));
        assert!(matches!(
            import_xlsx(&source, &working),
            Err(KitError::Admission(_))
        ));
        assert!(!working.exists());
        assert_eq!(fs::read(&source).unwrap(), original);
    }
}

#[test]
fn disabled_style_application_flags_with_conflicting_records_are_refused() {
    let f = Fixture::new();
    let source = f.path("disabled-font.xlsx");
    let fonts = format!("{DEFAULT_FONT}<font><name val=\"Calibri\"/><sz val=\"11\"/><b/></font>");
    let cell_xfs = format!(
        "{DEFAULT_XF}<xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"0\"/>"
    );
    let styles = styles_xml(&fonts, 2, &cell_xfs, 2);
    write_xlsx(
        &source,
        "<row r=\"1\"><c r=\"A1\" s=\"1\"><v>3</v></c></row>",
        "",
        "",
        &[("xl/styles.xml", &styles)],
    );
    let original = fs::read(&source).unwrap();
    let report = probe(&source).unwrap();
    assert!(!report.can_import);
    assert!(
        report
            .reasons
            .iter()
            .any(|reason| reason.contains("disabled style application flags"))
    );
    let working = f.path("disabled-font.omasheets");
    assert!(matches!(
        import_xlsx(&source, &working),
        Err(KitError::Admission(_))
    ));
    assert!(!working.exists());
    assert_eq!(fs::read(&source).unwrap(), original);
}
