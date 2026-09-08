use omasheets_service::{Request, Service};
use serde_json::{Value, json};
use std::path::PathBuf;

struct Fixture {
    service: Service,
    path: PathBuf,
    sheet: String,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "omasheets-controls-{}-{}.omasheets",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut f = Self {
            service: Service::new(|| 456),
            path,
            sheet: String::new(),
        };
        f.call(json!({"kind":"create","name":"Controls","actor":{"kind":"human","id":"test"}}));
        f.append(json!({"command":"add_sheet","name":"Data"}));
        f.sheet = f.call(json!({"kind":"document"}))["sheets"][0]["id"]
            .as_str()
            .unwrap()
            .into();
        f.append(json!({"command":"add_rows","sheet":f.sheet,"count":20,"at":0,"table":null}));
        f.append(json!({"command":"add_columns","sheet":f.sheet,"count":8,"at":0}));
        f
    }
    fn call(&mut self, mut request: Value) -> Value {
        request["path"] = json!(self.path);
        serde_json::to_value(
            self.service
                .handle(serde_json::from_value(request).unwrap())
                .unwrap(),
        )
        .unwrap()
    }
    fn append(&mut self, command: Value) {
        self.call(json!({"kind":"append","actor":{"kind":"human","id":"test"},"command":command}));
    }
    fn number(&mut self, a1: &str, value: f64) {
        self.append(json!({"command":"set_value","sheet":self.sheet,"a1":a1,"value":{"type":"number","value":value}}));
    }
    fn text(&mut self, a1: &str, value: &str) {
        self.append(json!({"command":"set_value","sheet":self.sheet,"a1":a1,"value":{"type":"text","value":value}}));
    }
    fn formula(&mut self, a1: &str, source: &str) {
        self.append(json!({"command":"set_formula","sheet":self.sheet,"a1":a1,"source":source}));
    }
    fn request(&mut self, action: Value) -> Request {
        let revision = self.call(json!({"kind":"revision"}))["revision"].clone();
        serde_json::from_value(json!({"kind":"edit_sheet","path":self.path,"sheet":self.sheet,"expected_revision":revision,"action":action})).unwrap()
    }
    fn edit(&mut self, action: Value) -> Value {
        let request = self.request(action);
        serde_json::to_value(self.service.handle(request).unwrap()).unwrap()
    }
    fn cell(&mut self, a1: &str) -> Value {
        self.call(json!({"kind":"cell","sheet":self.sheet,"a1":a1}))
    }
    fn page(&mut self) -> Value {
        self.call(json!({"kind":"grid_page","sheet":self.sheet,"row_start":0,"column_start":0,"rows":10,"columns":8}))
    }
    fn view(&mut self) -> Value {
        self.call(json!({"kind":"sheet_view","sheet":self.sheet}))
    }
    fn undo(&mut self, result: Value) {
        self.call(json!({"kind":"append_batch","actor":{"kind":"human","id":"test"},"commands":result["undo"],"expected_revision":result["revision"]}));
    }
    fn reopen(&mut self) {
        self.call(json!({"kind":"close"}));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.service.handle(Request::Close {
            path: self.path.clone(),
        });
        let _ = std::fs::remove_file(&self.path);
    }
}
fn range(row: usize, column: usize, rows: usize, columns: usize) -> Value {
    json!({"row":row,"column":column,"rows":rows,"columns":columns})
}

#[test]
fn array_and_financial_formulas_keep_stable_bindings_through_sort_and_reopen() {
    let mut f = Fixture::new();
    f.number("A1", 10.0);
    f.number("A2", 20.0);
    f.formula("B2", "=SUM(A2*{2,3})+PV(0,1,-5)");
    f.formula("E5", "=IFERROR(SUM(#REF!:#REF!),7)");
    assert_eq!(f.cell("B2")["value"]["value"], 105.0);
    f.edit(
        json!({"action":"sort","range":range(0,0,2,1),"column":0,"header":false,"descending":true}),
    );
    let page = f.page();
    let moved = page["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|cell| cell["a1"] == "B1")
        .unwrap();
    assert_eq!(moved["formula"], "=SUM(A1*{2,3})+PV(0,1,-5)");
    assert_eq!(moved["value"]["value"], 105.0);
    assert!(moved.get("formula_projection_error").is_none());
    let revision = f.call(json!({"kind":"revision"}));
    f.reopen();
    assert_eq!(f.call(json!({"kind":"revision"})), revision);
    assert_eq!(f.cell("B1")["value"]["value"], 105.0);
    assert_eq!(f.cell("E5")["value"]["value"], 7.0);
    f.number("A1", 30.0);
    assert_eq!(f.cell("B1")["value"]["value"], 155.0);
}

#[test]
fn moved_formula_text_preserves_absolute_axes_and_refuses_nonrectangular_ranges() {
    let mut f = Fixture::new();
    f.number("A1", 30.0);
    f.number("A2", 10.0);
    f.number("A3", 20.0);
    f.formula("B2", "=$A2+A$2+$A$2+LOG10(100)+LEN(\"A2\")");
    f.formula("E6", "=SUM(A1:A2)");
    f.edit(json!({"action":"sort","range":range(0,0,3,1),"column":0,"header":false,"descending":false}));
    let page = f.page();
    let b1 = page["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|cell| cell["a1"] == "B1")
        .unwrap();
    assert_eq!(b1["formula"], "=$A1+A$1+$A$1+LOG10(100)+LEN(\"A2\")");
    assert_eq!(b1["value"]["value"], 34.0);
    assert!(b1.get("formula_projection_error").is_none());
    let e6 = page["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|cell| cell["a1"] == "E6")
        .unwrap();
    assert!(!e6["formula_projection_error"].as_str().unwrap().is_empty());
    assert_eq!(e6["value"]["value"], 40.0);
    // The event text is immutable; only the editable view gets new addresses.
    assert_eq!(
        f.cell("B1")["state"]["input"]["formula"]["source"],
        "=$A2+A$2+$A$2+LOG10(100)+LEN(\"A2\")"
    );
    f.reopen();
    assert_eq!(f.cell("B1")["value"]["value"], 34.0);
}

#[test]
#[ignore = "requires Python with openpyxl; independent interchange gate"]
fn independent_reader_verifies_supported_xlsx_presentation() {
    let mut f = Fixture::new();
    let source = f.path.with_extension("source.xlsx");
    let imported = f.path.with_extension("import.omasheets");
    let exported = f.path.with_extension("export.xlsx");
    let python = |code: &str, path: &std::path::Path| {
        let output = std::process::Command::new("python")
            .args(["-c", code])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    python(
        r#"
from openpyxl import Workbook
from openpyxl.styles import Font,PatternFill,Alignment,Border,Side
import sys
w=Workbook();s=w.active;s.title='Plan'
s['A1']='Budget';s['B3']=12.5;s['C3']='=B3*2'
s.merge_cells('A1:D1');s.freeze_panes='C3';s.sheet_view.showGridLines=False
s.row_dimensions[1].height=24;s.column_dimensions['A'].width=25
s['B3'].number_format='£#,##0.00'
s['B3'].font=Font(bold=True,size=12,color='FF112233')
s['B3'].fill=PatternFill('solid',fgColor='FFCCDD88')
s['B3'].alignment=Alignment(horizontal='right',wrap_text=True)
s['B3'].border=Border(bottom=Side(style='thin'))
s['F9'].font=Font(italic=True,size=17)
w.save(sys.argv[1])
"#,
        &source,
    );
    let original = std::fs::read(&source).unwrap();
    f.service.handle(serde_json::from_value(json!({"kind":"import_xlsx","source":source,"output":imported,"actor":{"kind":"human","id":"test"}})).unwrap()).unwrap();
    f.call(json!({"kind":"close"}));
    std::fs::remove_file(&f.path).unwrap();
    f.path = imported;
    f.sheet = f.call(json!({"kind":"document"}))["sheets"][0]["id"]
        .as_str()
        .unwrap()
        .into();
    assert_eq!(f.cell("C3")["value"]["value"], 25.0);
    let view = f.view();
    assert_eq!(view["frozen_rows"], 2);
    assert_eq!(view["frozen_columns"], 2);
    assert_eq!(view["show_grid_lines"], false);
    let page = f.page();
    let blank = page["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|cell| cell["a1"] == "F9")
        .unwrap();
    assert_eq!(blank["style"]["italic"], true);
    f.call(json!({"kind":"export_xlsx","output":exported}));
    python(
        r#"
from openpyxl import load_workbook
import sys
s=load_workbook(sys.argv[1])['Plan']
assert s['C3'].value=='=B3*2'
assert s['B3'].number_format=='£#,##0.00'
assert s['B3'].font.bold and s['B3'].font.sz==12
assert s['B3'].fill.fgColor.rgb.upper()=='FFCCDD88'
assert s['B3'].alignment.horizontal=='right' and s['B3'].alignment.wrap_text
assert s['B3'].border.bottom.style=='thin'
assert s['F9'].font.italic and s['F9'].font.sz==17
assert s.freeze_panes=='C3' and str(s.merged_cells)=='A1:D1'
assert not s.sheet_view.showGridLines
assert s.row_dimensions[1].height==24 and abs(s.column_dimensions['A'].width-25)<.01
assert load_workbook(sys.argv[1],data_only=True)['Plan']['C3'].value==25
"#,
        &exported,
    );
    assert_eq!(std::fs::read(&source).unwrap(), original);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(exported).unwrap();
}

#[test]
fn formatting_preserves_raw_values_blank_styles_and_replay() {
    let mut f = Fixture::new();
    f.number("A1", 0.125);
    let result=f.edit(json!({"action":"format","range":range(0,0,1,2),"patch":{"bold":true,"background":"#ffe080","number_format":"0.0%"}}));
    let page = f.page();
    let cells = page["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[0]["display"], "12.5%");
    assert_eq!(f.cell("A1")["value"]["value"], 0.125);
    let digest = f.call(json!({"kind":"document"}))["digest"].clone();
    f.reopen();
    assert_eq!(f.call(json!({"kind":"document"}))["digest"], digest);
    f.undo(result);
    assert_eq!(f.page()["cells"][0]["style"]["bold"], false);
}

#[test]
fn rejected_format_merge_and_stale_actions_leave_no_events() {
    let mut f = Fixture::new();
    f.number("A1", 5.0);
    f.number("B1", 6.0);
    let before = f.call(json!({"kind":"document"}))["digest"].clone();
    for action in [
        json!({"action":"format","range":range(0,0,1,1),"patch":{"font_size":0}}),
        json!({"action":"format","range":range(0,0,1,1),"patch":{"unknown":true}}),
        json!({"action":"merge","range":range(0,0,1,2)}),
    ] {
        let request = f.request(action);
        assert!(f.service.handle(request).is_err());
        assert_eq!(f.call(json!({"kind":"document"}))["digest"], before);
    }
    let stale = f.request(json!({"action":"grid_lines","visible":false}));
    f.number("C1", 7.0);
    assert_eq!(
        f.service.handle(stale).unwrap_err().code,
        "document_changed"
    );
}

#[test]
fn sorted_rows_keep_stable_formula_bindings_and_formatting() {
    let mut f = Fixture::new();
    f.number("A2", 20.0);
    f.number("A3", 10.0);
    f.formula("B2", "=A2*2");
    f.formula("B3", "=A3*2");
    f.edit(json!({"action":"format","range":range(1,0,1,1),"patch":{"bold":true}}));
    let original = f.cell("A2")["cell"].clone();
    let result = f.edit(json!({"action":"sort","range":range(1,0,2,2),"column":0}));
    assert_eq!(f.cell("A3")["cell"], original);
    assert_eq!(f.cell("B3")["value"]["value"], 40.0);
    assert!(
        f.page()["cells"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cell| cell["a1"] == "A3" && cell["style"]["bold"] == true)
    );
    f.undo(result);
    assert_eq!(f.cell("A2")["cell"], original);
}

#[test]
fn inserting_columns_preserves_merged_ranges_and_frozen_boundaries() {
    let mut f = Fixture::new();
    f.text("A1", "Heading");
    f.edit(json!({"action":"merge","range":range(0,0,1,2)}));
    f.edit(json!({"action":"freeze","rows":1,"columns":2}));
    f.edit(json!({"action":"insert_columns","at":1,"count":1}));
    assert_eq!(f.view()["merges"][0]["columns"], 3);
    assert_eq!(f.view()["frozen_columns"], 3);
    let request=serde_json::from_value(json!({"kind":"append","path":f.path,"actor":{"kind":"human","id":"test"},"command":{"command":"set_value","sheet":f.sheet,"a1":"B1","value":{"type":"text","value":"Hidden"}}})).unwrap();
    assert!(f.service.handle(request).is_err());
    f.reopen();
    assert_eq!(f.view()["merges"][0]["columns"], 3);
}

#[test]
fn filters_charts_and_conditional_styles_recalculate_from_saved_values() {
    let mut f = Fixture::new();
    f.text("A1", "Category");
    f.text("B1", "Score");
    f.text("A2", "Alpha");
    f.text("A3", "Beta");
    f.number("B2", 5.0);
    f.number("B3", 15.0);
    f.edit(
        json!({"action":"filter","range":range(0,0,3,2),"column":0,"text":"Alpha","header":true}),
    );
    assert_eq!(f.view()["hidden_rows"], json!([2]));
    f.edit(json!({"action":"chart","range":range(0,0,3,2),"title":"Scores","kind":"bar"}));
    f.edit(json!({"action":"conditional","range":range(1,1,2,1),"comparison":"greater","value":10,"patch":{"background":"#ffe080"}}));
    assert_eq!(
        f.view()["charts"][0]["series"][0]["values"],
        json!([5.0, 15.0])
    );
    assert!(
        f.page()["cells"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cell| cell["a1"] == "B3" && cell["style"]["background"] == "#ffe080")
    );
    f.number("B3", 7.0);
    f.reopen();
    assert_eq!(
        f.view()["charts"][0]["series"][0]["values"],
        json!([5.0, 7.0])
    );
    assert!(
        f.page()["cells"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cell| cell["a1"] == "B3" && cell["style"]["background"].is_null())
    );
}

#[test]
fn unicode_replace_does_not_change_formulas_and_can_be_undone() {
    let mut f = Fixture::new();
    f.text("A1", "Été été");
    f.formula("B1", "=\"été\"");
    let result = f.edit(
        json!({"action":"replace","range":range(0,0,1,2),"find":"ÉTÉ","replacement":"summer"}),
    );
    assert_eq!(f.cell("A1")["value"]["value"], "summer summer");
    assert_eq!(f.cell("B1")["value"]["value"], "été");
    f.undo(result);
    assert_eq!(f.cell("A1")["value"]["value"], "Été été");
}

#[test]
fn sheet_duplication_copies_presentation_and_preserves_notes_verbatim() {
    let mut f = Fixture::new();
    f.number("A1", 4.0);
    f.formula("B1", "=A1*2");
    let row = f.cell("A1")["cell"]["row"].as_str().unwrap().to_owned();
    f.edit(json!({"action":"note","row":0,"column":0,"text":row}));
    f.edit(json!({"action":"format","range":range(0,0,1,1),"patch":{"italic":true}}));
    f.edit(json!({"action":"duplicate_sheet","name":"Copy"}));
    f.sheet = f.call(json!({"kind":"document"}))["sheets"][1]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(f.cell("B1")["value"]["value"], 8.0);
    let page = f.page();
    assert_eq!(page["cells"][0]["note"], row);
    assert_eq!(page["cells"][0]["style"]["italic"], true);
}

#[test]
fn undo_restores_formula_bindings_after_rows_move() {
    let mut f = Fixture::new();
    f.number("A2", 20.0);
    f.number("A3", 10.0);
    f.formula("B2", "=A2*2");
    f.edit(json!({"action":"sort","range":range(1,0,2,2),"column":0}));
    let before = f.cell("B3")["state"]["input"].clone();
    let edited = f.edit(json!({"action":"set_cells","row":2,"column":1,"values":[["99"]]}));
    f.undo(edited);
    f.reopen();
    assert_eq!(f.cell("B3")["state"]["input"], before);
    assert_eq!(f.cell("B3")["value"]["value"], 40.0);
}
