use omasheets_app::{
    files::{self, AppPaths},
    operations,
    policy::{self, Actor, PublishMode},
    transactions::Publisher,
    workflow,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
};
struct Fixture {
    root: PathBuf,
    paths: AppPaths,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("omasheets-parity-{}", files::random_id().unwrap()));
        let paths = AppPaths {
            state: root.join("state"),
            cache: root.join("cache"),
            runtime: root.join("runtime"),
        };
        paths.ensure().unwrap();
        Self { root, paths }
    }
    fn file(&self, name: &str, data: &[u8]) -> PathBuf {
        let p = self.root.join(name);
        fs::write(&p, data).unwrap();
        p
    }
    fn plan(&self, source: &std::path::Path, staged: &std::path::Path, mode: &str) -> Value {
        let id = files::random_id().unwrap();
        json!({"plan_id":id,"receipt_id":id,"session_id":"a".repeat(32),"revision":1,"staged_artifact":staged,"staged_sha256":files::identify(staged).unwrap().sha256,"target_destination":if mode=="replace"{source.to_owned()}else{self.root.join("copy.xlsx")},"target_mode":mode,"source_sha256":files::identify(source).unwrap().sha256,"backup_artifact":self.paths.state.join("backups").join(format!("{id}.xlsx")),"seal":"test-sealed-plan"})
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn every_compatibility_operation_is_typed_and_bounded() {
    let examples = json!([
{"type":"set_value","sheet":"Data","range":"a1","value":true},
{"type":"set_formula","sheet":"Data","range":"B1","formula":"=A1+1"},
{"type":"clear_range","sheet":"Data","range":"A1:B2"},
{"type":"rename_sheet","sheet":"Data","new_name":"Renamed"},
{"type":"add_sheet","sheet":"New"},{"type":"delete_sheet","sheet":"Old"},
{"type":"set_range_values","sheet":"Data","range":"A1:B1","values":[[1,null]]},
{"type":"set_range_formulas","sheet":"Data","range":"A1:B1","formulas":[["=1","=2"]]},
{"type":"format_cells","sheet":"Data","range":"A1","bold":true,"background_color":"#aabbcc"},
{"type":"insert_rows","sheet":"Data","row":1,"count":2},{"type":"delete_rows","sheet":"Data","row":1,"count":2},
{"type":"insert_columns","sheet":"Data","column":"b","count":2},{"type":"delete_columns","sheet":"Data","column":"b","count":2},
{"type":"fill_down","sheet":"Data","range":"A1:A3","source_rows":1},{"type":"fill_right","sheet":"Data","range":"A1:C1","source_columns":1},
{"type":"sort_range","sheet":"Data","range":"A1:C3","key_column":2,"ascending":false,"has_header":true},
{"type":"upsert_chart","sheet":"Data","name":"RevenueChart","source_sheet":"Data","source_range":"A1:B3","anchor_range":"D2:J20","chart_type":"column","title":"Revenue","has_column_headers":true,"has_row_headers":true,"legend":false},
{"type":"upsert_pivot","sheet":"Data","name":"RevenuePivot","source_sheet":"Data","source_range":"A1:C3","output_cell":"H1","rows":["Region"],"columns":[],"filters":[],"values":[{"field":"Amount","function":"sum","label":"Total"}]},
{"type":"refresh_pivot","sheet":"Data","name":"RevenuePivot"}]);
    let normalized = operations::validate(&examples).unwrap();
    assert_eq!(normalized.len(), operations::SUPPORTED.len());
    assert_eq!(normalized[0]["range"], "A1");
    assert_eq!(normalized[8]["background_color"], "#AABBCC");
    for mut op in normalized {
        op["approve"] = json!(true);
        assert!(operations::validate(&json!([op])).is_err());
    }
}
#[test]
fn malformed_ranges_formulas_scalars_and_layouts_are_rejected() {
    for op in [
        json!({"type":"set_value","sheet":"Data","range":"A1:A1","value":1}),
        json!({"type":"clear_range","sheet":"Data","range":"XFE1"}),
        json!({"type":"clear_range","sheet":"Data","range":"A1:A10001"}),
        json!({"type":"set_range_values","sheet":"Data","range":"A1:B2","values":[[1,2]]}),
        json!({"type":"insert_rows","sheet":"Data","row":true,"count":1}),
        json!({"type":"fill_down","sheet":"Data","range":"A1:A2","source_rows":2}),
    ] {
        assert!(operations::validate(&json!([op])).is_err());
    }
    for formula in [
        "SUM(A1:A2)",
        "=WEBSERVICE(\"https://host\")",
        "=DDE(\"server\";\"topic\";\"item\")",
        "='file:///tmp/a.xlsx'#$Data.A1",
    ] {
        assert!(
            operations::validate(
                &json!([{"type":"set_formula","sheet":"Data","range":"A1","formula":formula}])
            )
            .is_err()
        );
    }
    for address in ["B2:A1", "A1048577", "XFE1", "A0"] {
        assert!(operations::range_shape(address).is_err());
    }
}
#[test]
fn workflow_requires_exact_coverage_and_evidence() {
    let good = json!({"goal":"Enter amount","summary":"Correct the inspected amount","evidence_ids":["a".repeat(32)],"groups":[{"title":"Inputs","purpose":"Correct amounts","operation_indexes":[0,1]}]});
    assert!(workflow::validate(&good, 2).is_ok());
    let mut bad = good.clone();
    bad["groups"][0]["operation_indexes"] = json!([0, 0]);
    assert!(workflow::validate(&bad, 2).is_err());
    bad = good;
    bad["evidence_ids"] = json!([]);
    assert!(workflow::validate(&bad, 2).is_err());
}
#[test]
fn identities_and_private_state_reject_links_devices_and_broad_permissions() {
    let f = Fixture::new();
    let input = f.file("input.xlsx", b"source");
    let link = f.root.join("link.xlsx");
    symlink(&input, &link).unwrap();
    assert!(files::identify(&link).is_err());
    assert!(files::identify(std::path::Path::new("/dev/null")).is_err());
    let dst = f.root.join("copied.xlsx");
    let identity = files::copy_no_clobber(&input, &dst).unwrap();
    assert_eq!(identity.sha256, files::identify(&dst).unwrap().sha256);
    assert!(files::copy_no_clobber(&input, &dst).is_err());
    assert_eq!(
        fs::metadata(&dst).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let state = f.paths.state.join("state.json");
    files::write_json(&state, &json!({"a":1})).unwrap();
    assert_eq!(files::read_json(&state, 1024).unwrap()["a"], 1);
    fs::set_permissions(&state, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(files::read_json(&state, 1024).is_err());
}
#[test]
fn agent_authority_and_read_only_formats_are_rejected() {
    for mode in [PublishMode::Copy, PublishMode::Replace] {
        assert!(policy::publish_authority(Actor::Agent, mode).is_err());
    }
    for name in ["input.xls", "input.xlsm"] {
        assert!(policy::stageable(std::path::Path::new(name)).is_err());
    }
    assert_eq!(
        policy::conversion_destination(std::path::Path::new("input.xls")).unwrap(),
        PathBuf::from("input.xlsx")
    );
}
#[test]
fn copy_publication_preserves_source_and_never_clobbers() {
    let f = Fixture::new();
    let source = f.file("input.xlsx", b"source");
    let staged = f.file("staged.xlsx", b"staged");
    let publisher = Publisher::new(f.paths.clone()).unwrap();
    let plan = f.plan(&source, &staged, "copy");
    assert!(publisher.publish(&plan, &source, Actor::Agent).is_err());
    let receipt = publisher.publish(&plan, &source, Actor::Local).unwrap();
    assert_eq!(fs::read(&source).unwrap(), b"source");
    assert_eq!(fs::read(f.root.join("copy.xlsx")).unwrap(), b"staged");
    assert_eq!(
        publisher.publish(&plan, &source, Actor::Local).unwrap(),
        receipt
    );
    let different = f.file("different.xlsx", b"different");
    let plan = f.plan(&source, &different, "copy");
    assert!(publisher.publish(&plan, &source, Actor::Local).is_err());
    assert_eq!(fs::read(f.root.join("copy.xlsx")).unwrap(), b"staged");
}
#[test]
fn replacement_is_verified_receipted_and_undo_requires_exact_human_token() {
    let f = Fixture::new();
    let source = f.file("input.xlsx", b"source");
    let staged = f.file("staged.xlsx", b"staged");
    let publisher = Publisher::new(f.paths.clone()).unwrap();
    let plan = f.plan(&source, &staged, "replace");
    let receipt = publisher.publish(&plan, &source, Actor::Local).unwrap();
    assert_eq!(fs::read(&source).unwrap(), b"staged");
    let id = receipt["receipt_id"].as_str().unwrap();
    assert!(publisher.undo(id, "UNDO", Actor::Local).is_err());
    assert!(
        publisher
            .undo(id, &format!("UNDO {id}"), Actor::Agent)
            .is_err()
    );
    let undone = publisher
        .undo(id, &format!("UNDO {id}"), Actor::Local)
        .unwrap();
    assert_eq!(fs::read(&source).unwrap(), b"source");
    assert_eq!(
        publisher
            .undo(id, &format!("UNDO {id}"), Actor::Local)
            .unwrap(),
        undone
    );
}
#[test]
fn changed_source_staged_backup_and_published_bytes_block_writes() {
    let f = Fixture::new();
    let source = f.file("input.xlsx", b"source");
    let staged = f.file("staged.xlsx", b"staged");
    let publisher = Publisher::new(f.paths.clone()).unwrap();
    let plan = f.plan(&source, &staged, "replace");
    fs::write(&source, b"new source").unwrap();
    assert!(publisher.publish(&plan, &source, Actor::Local).is_err());
    fs::write(&source, b"source").unwrap();
    fs::write(&staged, b"new staged").unwrap();
    assert!(publisher.publish(&plan, &source, Actor::Local).is_err());
    fs::write(&staged, b"staged").unwrap();
    let r = publisher.publish(&plan, &source, Actor::Local).unwrap();
    let id = r["receipt_id"].as_str().unwrap();
    fs::write(&source, b"post-publication edit").unwrap();
    assert!(
        publisher
            .undo(id, &format!("UNDO {id}"), Actor::Local)
            .is_err()
    );
    fs::write(&source, b"staged").unwrap();
    fs::write(r["backup"].as_str().unwrap(), b"corrupt").unwrap();
    assert!(
        publisher
            .undo(id, &format!("UNDO {id}"), Actor::Local)
            .is_err()
    );
    assert_eq!(fs::read(&source).unwrap(), b"staged");
}
#[test]
fn corrupted_receipts_are_never_accepted() {
    let f = Fixture::new();
    let source = f.file("input.xlsx", b"source");
    let staged = f.file("staged.xlsx", b"staged");
    let publisher = Publisher::new(f.paths.clone()).unwrap();
    let plan = f.plan(&source, &staged, "copy");
    let mut receipt = publisher.publish(&plan, &source, Actor::Local).unwrap();
    let id = receipt["receipt_id"].as_str().unwrap().to_owned();
    receipt["target_mode"] = json!("replace");
    files::write_json(&publisher.receipts.path(&id).unwrap(), &receipt).unwrap();
    assert!(publisher.receipts.get(&id).is_err());
    assert!(publisher.publish(&plan, &source, Actor::Local).is_err());
}
