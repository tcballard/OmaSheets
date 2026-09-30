//! Real LibreOffice/sandbox parity. No interpreter participates in this test.
use omasheets_app::{
    engine::Engine,
    files::{self, AppPaths},
    uno::{Uno, property},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture(root: &Path) -> Vec<PathBuf> {
    let mut u = Uno::connect(root).unwrap();
    let desktop = u.service("frame.Desktop").unwrap();
    let doc = u
        .call(
            &desktop,
            "loadComponentFromURL",
            vec![
                json!("private:factory/scalc"),
                json!("_blank"),
                json!(0),
                json!([property("Hidden", json!(true))]),
            ],
        )
        .unwrap();
    let sheets = u.call(&doc, "getSheets", vec![]).unwrap();
    let sheet = u.call(&sheets, "getByIndex", vec![json!(0)]).unwrap();
    u.call(&sheet, "setName", vec![json!("Revenue")]).unwrap();
    let area = u
        .call(&sheet, "getCellRangeByName", vec![json!("A1:C4")])
        .unwrap();
    u.call(
        &area,
        "setDataArray",
        vec![json!([
            ["Region", "Amount", "Period"],
            ["North", 20, "Q1"],
            ["South", 30, "Q1"],
            ["North", 25, "Q2"]
        ])],
    )
    .unwrap();
    let mut paths = vec![];
    for (extension, filter) in [
        ("ods", "calc8"),
        ("xlsx", "Calc MS Excel 2007 XML"),
        ("xls", "MS Excel 97"),
    ] {
        let path = root.join(format!("source.{extension}"));
        u.call(
            &doc,
            "storeToURL",
            vec![
                json!(format!(
                    "{}/source.{extension}",
                    omasheets_app::uno::file_url(root).unwrap()
                )),
                json!([
                    property("FilterName", json!(filter)),
                    property("Overwrite", json!(false))
                ]),
            ],
        )
        .unwrap();
        paths.push(path);
    }
    u.call(&doc, "close", vec![json!(true)]).unwrap();
    paths
}
fn main() {
    let root = std::env::temp_dir().join(format!(
        "omasheets-rust-compat-{}",
        files::random_id().unwrap()
    ));
    files::private_dir(&root).unwrap();
    let _fixture = Fixture(root.clone());
    for name in ["home", "runtime", "profile"] {
        files::private_dir(&root.join(name)).unwrap();
    }
    let sources = fixture(&root);
    let paths = AppPaths {
        state: root.join("state"),
        cache: root.join("cache"),
        runtime: root.join("runtime"),
    };
    paths.ensure().unwrap();
    let engine = Engine { paths };
    for source in &sources[..2] {
        let before = files::identify(source).unwrap();
        let result=engine.execute("query",source,json!({"queries":[{"id":"structure","tool":"describe_workbook","arguments":{"include_formulas":true}},{"id":"inputs","tool":"read_range","arguments":{"sheet":"Revenue","range":"A1:C4","include_styles":true}}]}),&[]).unwrap();
        assert_eq!(result["items"][0]["result"]["sheet_count"], 1);
        assert_eq!(result["items"][1]["result"]["values"][1][1], 20.0);
        assert!(
            !result["items"][1]["result"]["style_table"]["styles"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let audit = engine
            .execute(
                "analyze",
                source,
                json!({"focus":"all","max_findings":20}),
                &[],
            )
            .unwrap();
        assert_eq!(audit["summary"]["sheet_count"], 1);
        assert_eq!(audit["method"], "deterministic_bounded_profile_v1");
        let ops = json!([
        {"type":"insert_rows","sheet":"Revenue","row":5,"count":2},
        {"type":"set_range_values","sheet":"Revenue","range":"A5:C6","values":[["East",10,"Q2"],["West",15,"Q2"]]},
        {"type":"set_formula","sheet":"Revenue","range":"D2","formula":"=B2*2"},
        {"type":"fill_down","sheet":"Revenue","range":"D2:D6","source_rows":1},
        {"type":"format_cells","sheet":"Revenue","range":"B2:B6","bold":true,"number_format":"0.00"},
        {"type":"add_sheet","sheet":"Summary"},
        {"type":"upsert_pivot","sheet":"Summary","name":"RevenuePivot","source_sheet":"Revenue","source_range":"A1:C6","output_cell":"A2","rows":["Region"],"columns":[],"filters":["Period"],"values":[{"field":"Amount","function":"sum","label":"Total amount"}]},
        {"type":"upsert_chart","sheet":"Summary","name":"RevenueChart","source_sheet":"Revenue","source_range":"A1:B6","anchor_range":"H2:P18","chart_type":"column","title":"Revenue by region","has_column_headers":true,"has_row_headers":true,"legend":true}
        ]);
        let ext = source.extension().unwrap().to_str().unwrap();
        let staged = root.join(format!("staged.{ext}"));
        let pdf = root.join(format!("preview-{ext}.pdf"));
        let verified = engine
            .execute(
                "stage",
                source,
                json!({"operations":ops}),
                &[("workbook", &staged), ("preview", &pdf)],
            )
            .unwrap();
        assert_eq!(verified["verification"]["status"], "verified");
        assert_eq!(
            verified["verification"]["comparison"]["workbook_objects_match"],
            true
        );
        assert_eq!(
            verified["semantic_diff"]["object_changes"]["after"]["charts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            verified["semantic_diff"]["object_changes"]["after"]["pivots"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(fs::read(&pdf).unwrap().starts_with(b"%PDF-"));
        let read = engine
            .execute(
                "read_range",
                &staged,
                json!({"sheet":"Revenue","range":"D2:D6"}),
                &[],
            )
            .unwrap();
        let values: Vec<Value> = read["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r[0].clone())
            .collect();
        assert_eq!(
            values,
            json!([40.0, 60.0, 50.0, 20.0, 30.0])
                .as_array()
                .unwrap()
                .clone()
        );
        assert_eq!(files::identify(source).unwrap(), before);
        println!(
            "PASS: Rust {ext} read/styles/audit/stage/formula-fill/chart/pivot/reopen/PDF, source unchanged"
        );
    }
    let legacy = &sources[2];
    let converted = root.join("converted.xlsx");
    let preview = root.join("converted.pdf");
    let before = files::identify(legacy).unwrap();
    let evidence = engine
        .execute(
            "convert_xls",
            legacy,
            json!({}),
            &[("preview", &preview), ("workbook", &converted)],
        )
        .unwrap();
    assert_eq!(evidence["verification"]["status"], "manual_review_required");
    assert_eq!(files::identify(legacy).unwrap(), before);
    assert!(
        engine
            .execute(
                "convert_xls",
                legacy,
                json!({}),
                &[("workbook", &converted)]
            )
            .is_err()
    );
    println!("PASS: Rust legacy conversion, no-clobber output, original unchanged");
}
