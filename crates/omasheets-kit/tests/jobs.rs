use omasheets_kit::WorkbookSession;
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "omasheets-owned-jobs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn source(&self, extras: &str) -> PathBuf {
        let path = self.path("source.xlsx");
        write_xlsx(&path, extras);
        path
    }
    fn job(&self, source: &Path, action: &str, arguments: Value) -> (Value, PathBuf) {
        let job = self.path(&format!("job-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(job.join("input")).unwrap();
        fs::create_dir(job.join("out")).unwrap();
        let filename = format!("workbook.{}", source.extension().unwrap().to_str().unwrap());
        let copied = job.join("input").join(&filename);
        fs::copy(source, &copied).unwrap();
        let before = fs::read(source).unwrap();
        fs::write(job.join("request.json"), serde_json::to_vec(&json!({"action": action,
            "source": format!("input/{filename}"), "arguments": arguments,
            "limits": {"max_cells": 250_000, "max_formulas": 20_000, "max_sheets": 64, "max_results": 200}})).unwrap()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_omasheets-kit"))
            .arg("job")
            .arg(job.join("request.json"))
            .arg(job.join("result.json"))
            .current_dir(&self.0)
            .output()
            .unwrap();
        let result: Value =
            serde_json::from_slice(&fs::read(job.join("result.json")).unwrap()).unwrap();
        assert_eq!(
            output.status.code(),
            Some(if result["ok"] == true { 0 } else { 2 }),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read(source).unwrap(),
            before,
            "Every job must preserve source bytes"
        );
        assert_eq!(
            fs::read(&copied).unwrap(),
            before,
            "Even private job inputs must remain immutable"
        );
        (result, job)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_xlsx(path: &Path, extras: &str) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    for (name, content) in [
        ("[Content_Types].xml", "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>".into()),
        ("_rels/.rels", "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>".into()),
        ("xl/workbook.xml", format!("<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"Data\" sheetId=\"1\" r:id=\"rId1\"/></sheets>{extras}<calcPr calcMode=\"auto\"/></workbook>")),
        ("xl/_rels/workbook.xml.rels", "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>".into()),
        ("xl/worksheets/sheet1.xml", "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>Left</t></is></c><c r=\"B1\" t=\"inlineStr\"><is><t>Right</t></is></c><c r=\"C1\" t=\"inlineStr\"><is><t>Total</t></is></c></row><row r=\"2\"><c r=\"A2\"><v>1</v></c><c r=\"B2\"><v>2</v></c><c r=\"C2\"><f>A2+B2</f><v>3</v></c></row><row r=\"3\"><c r=\"A3\"><v>3</v></c><c r=\"B3\"><v>4</v></c><c r=\"C3\"><f>A3+B3</f><v>7</v></c></row></sheetData></worksheet>".into()),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn owned_query_inspects_values_styles_search_and_resolved_formula_inputs() {
    let fixture = Fixture::new();
    let source = fixture.source("");
    let (response, _) = fixture.job(&source, "query", json!({"queries": [
        {"id": "inventory", "tool": "describe_workbook", "arguments": {"include_formulas": true}},
        {"id": "values", "tool": "read_range", "arguments": {"sheet": "Data", "range": "A2:C2", "include_styles": true}},
        {"id": "search", "tool": "search_workbook", "arguments": {"query": "A2", "scope": "formulas"}},
        {"id": "inputs", "tool": "trace_formula", "arguments": {"sheet": "Data", "cell": "C2", "direction": "both"}}
    ]}));
    assert_eq!(response["ok"], true, "{response}");
    let items = response["result"]["items"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["inventory", "values", "search", "inputs"]
    );
    assert_eq!(items[0]["result"]["sheet_count"], 1);
    assert_eq!(items[0]["result"]["formula_count"], 2);
    assert_eq!(items[1]["result"]["values"], json!([[1.0, 2.0, 3.0]]));
    assert_eq!(
        items[1]["result"]["formulas"],
        json!([["1", "2", "=A2+B2"]])
    );
    assert_eq!(
        items[1]["result"]["style_table"]["style_ids"],
        json!([[0, 0, 0]])
    );
    assert_eq!(items[2]["result"]["matches"][0]["column"], 3);
    assert_eq!(
        items[3]["result"]["precedents"].as_array().unwrap().len(),
        2
    );
    assert_eq!(items[3]["result"]["resolved_depth"], 1);
}

#[test]
fn owned_audit_profiles_calculated_columns_and_reports_cited_findings() {
    let fixture = Fixture::new();
    let source = fixture.source("");
    let (response, _) = fixture.job(
        &source,
        "analyze",
        json!({"focus": "all", "max_findings": 10}),
    );
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["summary"]["formula_count"], 2);
    assert_eq!(response["result"]["summary"]["data_rows"], 2);
    assert_eq!(response["result"]["sheets"][0]["columns"][2]["sum"], 10.0);
    assert_eq!(
        response["result"]["method"],
        "deterministic_bounded_profile_v1"
    );
    assert_eq!(
        response["result"]["management_summary_opportunities"][0]["measures"],
        json!(["Left", "Right", "Total"])
    );
}

#[test]
fn stage_recalculates_reopens_and_exports_formula_and_cache_independently() {
    let fixture = Fixture::new();
    let source = fixture.source("");
    let (response, job) = fixture.job(
        &source,
        "stage",
        json!({"operations": [
            {"type": "set_value", "sheet": "Data", "range": "A2", "value": 20}
        ]}),
    );
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["verification"]["reopened"], true);
    assert_eq!(
        response["result"]["verification"]["comparison"]["all_cells_and_presentation_match"],
        true
    );
    assert_eq!(
        response["result"]["semantic_diff"]["target_changes"][0]["after"]["values"],
        json!([[20.0]])
    );
    let mut zip = zip::ZipArchive::new(File::open(job.join("out/workbook.xlsx")).unwrap()).unwrap();
    let mut sheet = String::new();
    zip.by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut sheet)
        .unwrap();
    assert!(sheet.contains("<f>A2+B2</f><v>22</v>"), "{sheet}");
    assert!(sheet.contains("<v>20</v>"));
    let pdf = fs::read(job.join("out/preview.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF-1.4"));
    assert!(String::from_utf8(pdf).unwrap().contains("(22) Tj"));
}

#[test]
fn native_stages_preserve_the_source_and_replay_the_new_native_copy() {
    let fixture = Fixture::new();
    let source = fixture.source("");
    let native = fixture.path("source.omasheets");
    WorkbookSession::open_xlsx(&source, &native)
        .unwrap()
        .close()
        .unwrap();
    let (response, job) = fixture.job(
        &native,
        "stage",
        json!({"operations": [
            {"type": "set_value", "sheet": "Data", "range": "A2", "value": 20}
        ]}),
    );
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["artifacts"]["workbook"], "out/workbook.omasheets");
    let mut reopened = WorkbookSession::open_native(job.join("out/workbook.omasheets")).unwrap();
    let page = reopened.viewport("Data", 1, 0, 1, 3).unwrap();
    assert_eq!(
        serde_json::to_value(&page.cells[2].value).unwrap(),
        json!({"type": "number", "value": 22.0})
    );
    reopened.close().unwrap();
}

#[test]
fn unsupported_content_and_operations_are_refused_without_published_artifacts() {
    let fixture = Fixture::new();
    let source = fixture.source(
        "<definedNames><definedName name=\"Budget\">Data!$A$2</definedName></definedNames>",
    );
    let (response, _) = fixture.job(&source, "describe", json!({}));
    assert_eq!(response["ok"], false);
    assert!(
        response["error"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains("defined"),
        "{response}"
    );
    let source = fixture.source("");
    for operation in [
        json!({"type": "fill_down", "sheet": "Data", "range": "A1:C3", "source_rows": 1}),
        json!({"type": "sort_range", "sheet": "Data", "range": "A1:B3", "key_column": 1, "ascending": true, "has_header": true}),
        json!({"type": "set_formula", "sheet": "Data", "range": "C2", "formula": "=UNKNOWN(A2)"}),
    ] {
        let (response, job) = fixture.job(&source, "stage", json!({"operations": [operation]}));
        assert_eq!(response["ok"], false, "{response}");
        assert!(!job.join("out/workbook.xlsx").exists());
        assert!(response.get("artifacts").is_none());
    }
}

#[test]
fn literal_values_and_styles_survive_staging_without_becoming_formulas() {
    let fixture = Fixture::new();
    let source = fixture.source("");
    let (response, _) = fixture.job(&source, "stage", json!({"operations": [
        {"type": "set_value", "sheet": "Data", "range": "A1", "value": "=literal"},
        {"type": "format_cells", "sheet": "Data", "range": "A1", "bold": true, "background_color": "#112233"}
    ]}));
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(
        response["result"]["semantic_diff"]["after"]["formula_count"],
        2
    );
    let after = &response["result"]["semantic_diff"]["target_changes"][0]["after"];
    assert_eq!(after["values"], json!([["=literal"]]));
    assert_eq!(after["format"]["bold"], true);
    assert_eq!(after["format"]["background_color"], "#112233");
}

#[test]
fn query_batches_validate_every_item_before_any_result_is_returned() {
    let fixture = Fixture::new();
    let source = fixture.source("");
    let (response, job) = fixture.job(
        &source,
        "query",
        json!({"queries": [
            {"id": "same", "tool": "describe_workbook", "arguments": {}},
            {"id": "same", "tool": "read_range", "arguments": {"sheet": "Data", "range": "A1:C3"}}
        ]}),
    );
    assert_eq!(response["ok"], false);
    assert!(!job.join("out/session.omasheets").exists());
    assert!(response.get("result").is_none());
}

#[test]
fn unicode_expansion_and_long_text_are_reported_as_cropped_in_owned_previews() {
    let fixture = Fixture::new();
    let source = fixture.source("");
    let (response, job) = fixture.job(
        &source,
        "stage",
        json!({"operations": [
            {"type": "set_value", "sheet": "Data", "range": "A1", "value": "🐈🐈🐈🐈"}
        ]}),
    );
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["preview"]["cell_text_truncated"], true);
    assert_eq!(response["result"]["preview"]["unicode_escaped"], true);
    let pdf = String::from_utf8(fs::read(job.join("out/preview.pdf")).unwrap()).unwrap();
    assert!(pdf.contains("(U+1F408U+1F408U...) Tj"), "{pdf}");
    assert_eq!(
        response["result"]["semantic_diff"]["target_changes"][0]["after"]["values"],
        json!([["🐈🐈🐈🐈"]])
    );
}

#[test]
fn structural_edits_keep_formula_binding_through_strict_xlsx_roundtrip() {
    let fixture = Fixture::new();
    let source = fixture.source("");
    let (response, job) = fixture.job(
        &source,
        "stage",
        json!({"operations": [
            {"type": "insert_rows", "sheet": "Data", "row": 2, "count": 1},
            {"type": "set_value", "sheet": "Data", "range": "A3", "value": 20},
            {"type": "insert_columns", "sheet": "Data", "column": "B", "count": 1},
            {"type": "delete_columns", "sheet": "Data", "column": "B", "count": 1},
            {"type": "delete_rows", "sheet": "Data", "row": 2, "count": 1}
        ]}),
    );
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(
        response["result"]["verification"]["comparison"]["all_cells_and_presentation_match"],
        true
    );
    let mut zip = zip::ZipArchive::new(File::open(job.join("out/workbook.xlsx")).unwrap()).unwrap();
    let mut sheet = String::new();
    zip.by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut sheet)
        .unwrap();
    assert!(sheet.contains("<f>A2+B2</f><v>22</v>"), "{sheet}");
}
