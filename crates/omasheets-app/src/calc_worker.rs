//! Compatibility workbook logic. Only UNO dispatch/types cross the native adapter.
use crate::{
    Result, files, operations,
    uno::{Uno, enumeration, property, structure},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};
pub struct Workbook {
    pub uno: Uno,
    pub doc: Value,
}
fn array(v: &Value) -> Result<&Vec<Value>> {
    v.as_array()
        .ok_or("UNO returned an invalid sequence".into())
}
fn number(v: &Value) -> Result<i64> {
    v.as_i64()
        .or_else(|| v.as_f64().map(|n| n as i64))
        .ok_or("UNO returned an invalid number".into())
}
fn string(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_else(|| {
        if v.is_null() {
            String::new()
        } else {
            v.to_string()
        }
    })
}
pub fn column_name(mut n: usize) -> String {
    let mut s = String::new();
    n += 1;
    while n > 0 {
        n -= 1;
        s.insert(0, (b'A' + (n % 26) as u8) as char);
        n /= 26;
    }
    s
}
fn fields(v: &Value) -> &Value {
    &v["fields"]
}
impl Workbook {
    pub fn load(job: &Path, source: &Path, read_only: bool) -> Result<Self> {
        let mut uno = Uno::connect(job)?;
        let doc = uno.load(source, read_only)?;
        Ok(Self { uno, doc })
    }
    fn sheets(&mut self) -> Result<Value> {
        let d = self.doc.clone();
        self.uno.call(&d, "getSheets", vec![])
    }
    fn sheet(&mut self, name: &str) -> Result<Value> {
        let sheets = self.sheets()?;
        if self.uno.call(&sheets, "hasByName", vec![json!(name)])? != true {
            return Err("sheet not found".into());
        }
        self.uno.call(&sheets, "getByName", vec![json!(name)])
    }
    fn area(&mut self, name: &str, range: &str) -> Result<Value> {
        operations::range_shape(range)?;
        let sheet = self.sheet(name)?;
        self.uno
            .call(&sheet, "getCellRangeByName", vec![json!(range)])
    }
    fn used(&mut self) -> Result<Vec<(String, Value, usize, usize)>> {
        let sheets = self.sheets()?;
        let names = self.uno.call(&sheets, "getElementNames", vec![])?;
        let names = array(&names)?;
        if names.len() > 256 {
            return Err("workbook exceeds the sheet limit".into());
        }
        let mut result = vec![];
        let mut total = 0usize;
        for name in names {
            let name = name.as_str().ok_or("invalid sheet name")?;
            let sheet = self.sheet(name)?;
            let cursor = self.uno.call(&sheet, "createCursor", vec![])?;
            self.uno
                .call(&cursor, "gotoEndOfUsedArea", vec![json!(true)])?;
            let a = self.uno.call(&cursor, "getRangeAddress", vec![])?;
            let rows = number(&fields(&a)["EndRow"])? as usize + 1;
            let cols = number(&fields(&a)["EndColumn"])? as usize + 1;
            total = total
                .checked_add(rows.checked_mul(cols).ok_or("workbook range overflow")?)
                .ok_or("workbook range overflow")?;
            if total > 250000 {
                return Err("workbook exceeds the inspected-cell limit".into());
            }
            let area = self.uno.call(
                &sheet,
                "getCellRangeByPosition",
                vec![json!(0), json!(0), json!(cols - 1), json!(rows - 1)],
            )?;
            self.uno.release(&cursor)?;
            result.push((name.into(), area, rows, cols));
        }
        Ok(result)
    }
    fn named_ranges(&mut self) -> Result<Value> {
        let d = self.doc.clone();
        let ranges = self.uno.get(&d, "NamedRanges")?;
        let names = self.uno.call(&ranges, "getElementNames", vec![])?;
        let mut names = array(&names)?.clone();
        names.sort_by_key(string);
        let mut items = vec![];
        for name in names.iter().take(200) {
            let obj = self.uno.call(&ranges, "getByName", vec![name.clone()])?;
            let content = self.uno.call(&obj, "getContent", vec![])?;
            let text = string(&content);
            let redacted = text.to_ascii_lowercase().contains("file:") || text.contains("://");
            items.push(if redacted {
                json!({"name":name,"content_redacted":true})
            } else {
                json!({"name":name,"content":content,"content_redacted":false})
            });
            self.uno.release(&obj)?;
        }
        Ok(json!({"items":items,"truncated":names.len()>200,"total":names.len()}))
    }
    pub fn inspect(&mut self, include_formulas: bool) -> Result<Value> {
        let used = self.used()?;
        let mut sheets = vec![];
        let mut records = vec![];
        let mut errors = vec![];
        let mut count = 0usize;
        let mut total = 0usize;
        for (name, area, rows, cols) in used {
            total += rows * cols;
            let formulas = self.uno.call(&area, "getFormulaArray", vec![])?;
            let mut sheet_count = 0;
            for (ri, row) in array(&formulas)?.iter().enumerate() {
                for (ci, formula) in array(row)?.iter().enumerate() {
                    if string(formula).starts_with('=') {
                        sheet_count += 1;
                        count += 1;
                        if count > 20000 {
                            return Err("workbook exceeds the formula limit".into());
                        }
                        if include_formulas {
                            records.push(
                                json!({"sheet":name,"row":ri+1,"column":ci+1,"formula":formula}),
                            );
                        }
                        if errors.len() < 200 {
                            let cell = self.uno.call(
                                &area,
                                "getCellByPosition",
                                vec![json!(ci), json!(ri)],
                            )?;
                            let code = self.uno.call(&cell, "getError", vec![])?;
                            let displayed = self.uno.call(&cell, "getString", vec![])?;
                            if number(&code)? != 0 || string(&displayed).starts_with('#') {
                                errors.push(json!({"sheet":name,"row":ri+1,"column":ci+1,"formula":formula,"error_code":code,"displayed":displayed}));
                            }
                            self.uno.release(&cell)?;
                        }
                    }
                }
            }
            sheets.push(json!({"name":name,"used_range":{"start_column":1,"start_row":1,"end_column":cols,"end_row":rows},"rows":rows,"columns":cols,"formula_count":sheet_count}));
            self.uno.release(&area)?;
        }
        Ok(
            json!({"sheet_count":sheets.len(),"sheets":sheets,"inspected_cells":total,"formula_count":count,"formula_errors":errors,"formulas":records,"named_ranges":self.named_ranges()?}),
        )
    }
    fn format_snapshot(&mut self, area: &Value, op: &Value) -> Result<Value> {
        let mut result = json!({});
        for (key, prop) in [
            ("bold", "CharWeight"),
            ("text_color", "CharColor"),
            ("background_color", "CellBackColor"),
            ("wrap_text", "IsTextWrapped"),
        ] {
            if op.get(key).is_some() {
                let value = self.uno.get(area, prop)?;
                result[key] = match key {
                    "bold" => json!(value.as_f64().ok_or("invalid font weight")? >= 150.0),
                    "text_color" | "background_color" => {
                        json!(format!("#{:06X}", number(&value)? & 0xffffff))
                    }
                    _ => value,
                };
            }
        }
        if op.get("number_format").is_some() {
            let d = self.doc.clone();
            let formats = self.uno.call(&d, "getNumberFormats", vec![])?;
            let key = self.uno.get(area, "NumberFormat")?;
            let props = self.uno.call(&formats, "getByKey", vec![key])?;
            result["number_format"] =
                self.uno
                    .call(&props, "getPropertyValue", vec![json!("FormatString")])?;
        }
        Ok(result)
    }
    fn style(&mut self, cell: &Value) -> Result<Value> {
        let mut s=self.format_snapshot(cell,&json!({"bold":true,"text_color":true,"background_color":true,"wrap_text":true,"number_format":true}))?;
        s["style_name"] = self.uno.get(cell, "CellStyle")?;
        for (k, p) in [
            ("text_color", "CharColor"),
            ("background_color", "CellBackColor"),
        ] {
            if number(&self.uno.get(cell, p)?)? < 0 {
                s[k] = json!("automatic");
            }
        }
        Ok(s)
    }
    pub fn read(&mut self, args: &Value) -> Result<Value> {
        let sheet = args["sheet"].as_str().ok_or("sheet required")?;
        let range = args["range"].as_str().ok_or("range required")?;
        let (rows, cols) = operations::range_shape(range)?;
        if rows as u64 * cols as u64 > 10000 {
            return Err("requested range exceeds the read limit".into());
        }
        let area = self.area(sheet, range)?;
        let mut result = json!({"sheet":sheet,"range":range,"values":self.uno.call(&area,"getDataArray",vec![])?});
        if args["include_formulas"].as_bool().unwrap_or(true) {
            result["formulas"] = self.uno.call(&area, "getFormulaArray", vec![])?;
        }
        if args["include_styles"].as_bool().unwrap_or(false) {
            if rows * cols > 1000 {
                return Err("styled range reads are limited to 1000 cells".into());
            }
            let mut styles = vec![];
            let mut ids = vec![];
            for r in 0..rows {
                let mut row = vec![];
                for c in 0..cols {
                    let cell =
                        self.uno
                            .call(&area, "getCellByPosition", vec![json!(c), json!(r)])?;
                    let style = self.style(&cell)?;
                    let id = styles.iter().position(|s| s == &style).unwrap_or_else(|| {
                        styles.push(style);
                        styles.len() - 1
                    });
                    row.push(id);
                    self.uno.release(&cell)?;
                }
                ids.push(row);
            }
            result["style_table"] = json!({"styles":styles,"style_ids":ids});
        }
        self.uno.release(&area)?;
        Ok(result)
    }
    pub fn search(&mut self, args: &Value) -> Result<Value> {
        let query = args["query"]
            .as_str()
            .ok_or("query required")?
            .to_lowercase();
        if query.is_empty() || query.chars().count() > 256 {
            return Err("invalid query".into());
        }
        let scope = args["scope"].as_str().unwrap_or("both");
        if !["both", "values", "formulas"].contains(&scope) {
            return Err("invalid search scope".into());
        }
        let max = args["max_results"].as_u64().unwrap_or(50);
        if !(1..=200).contains(&max) {
            return Err("invalid search result limit".into());
        }
        let mut matches = vec![];
        for (name, area, _, _) in self.used()? {
            let values = self.uno.call(&area, "getDataArray", vec![])?;
            let formulas = self.uno.call(&area, "getFormulaArray", vec![])?;
            for (r, row) in array(&values)?.iter().enumerate() {
                for (c, value) in array(row)?.iter().enumerate() {
                    let f = &formulas[r][c];
                    let vs = string(value);
                    let fs = string(f);
                    if (scope != "formulas" && vs.to_lowercase().contains(&query))
                        || (scope != "values" && fs.to_lowercase().contains(&query))
                    {
                        matches.push(json!({"sheet":name,"row":r+1,"column":c+1,"value":vs,"formula":if fs.starts_with('='){Some(fs)}else{None}}));
                        if matches.len() >= max as usize {
                            return Ok(json!({"matches":matches,"truncated":true}));
                        }
                    }
                }
            }
            self.uno.release(&area)?;
        }
        Ok(json!({"matches":matches,"truncated":false}))
    }
    pub fn trace(&mut self, args: &Value) -> Result<Value> {
        let sheet = args["sheet"].as_str().ok_or("sheet required")?;
        let cell = args["cell"].as_str().ok_or("cell required")?;
        if cell.contains(':') {
            return Err("trace must target one cell".into());
        }
        let area = self.area(sheet, cell)?;
        let formula = self.uno.call(&area, "getFormula", vec![])?;
        let regex=regex::Regex::new(r"(?:'([^']+)'|([A-Za-z_][^.!]*))?[.!]?(\$?[A-Z]{1,3}\$?[1-9][0-9]{0,6}(?::\$?[A-Z]{1,3}\$?[1-9][0-9]{0,6})?)").unwrap();
        let mut precedents = vec![];
        if string(&formula).starts_with('=') {
            for c in regex.captures_iter(&string(&formula)).take(200) {
                precedents.push(json!({"sheet":c.get(1).or_else(||c.get(2)).map_or(sheet,|m|m.as_str()),"range":c[3].replace('$',"")}));
            }
        }
        let direction = args["direction"].as_str().unwrap_or("both");
        let depth = args["max_depth"].as_u64().unwrap_or(5);
        if !["precedents", "dependents", "both"].contains(&direction) || !(1..=10).contains(&depth)
        {
            return Err("invalid trace bounds".into());
        }
        self.uno.release(&area)?;
        Ok(
            json!({"root":{"sheet":sheet,"cell":cell,"formula":formula},"precedents":if direction=="dependents"{vec![]}else{precedents},"dependents":[],"max_depth":depth,"warnings":["Formula tracing resolves literal A1 precedents only; dynamic references and dependents may be incomplete"]}),
        )
    }
    fn number_format(&mut self, code: &Value) -> Result<Value> {
        let d = self.doc.clone();
        let formats = self.uno.call(&d, "getNumberFormats", vec![])?;
        let locale = structure("lang.Locale", json!({}));
        let key = self.uno.call(
            &formats,
            "queryKey",
            vec![code.clone(), locale.clone(), json!(true)],
        )?;
        if number(&key)? >= 0 {
            Ok(key)
        } else {
            self.uno
                .call(&formats, "addNew", vec![code.clone(), locale])
        }
    }
    pub fn apply(&mut self, ops: &[Value]) -> Result<()> {
        for op in ops {
            let kind = op["type"].as_str().ok_or("operation type required")?;
            let sheet_name = op["sheet"].as_str().ok_or("sheet required")?;
            let sheets = self.sheets()?;
            match kind {
                "add_sheet" => {
                    let count = self.uno.call(&sheets, "getCount", vec![])?;
                    self.uno
                        .call(&sheets, "insertNewByName", vec![json!(sheet_name), count])?;
                }
                "delete_sheet" => {
                    self.uno
                        .call(&sheets, "removeByName", vec![json!(sheet_name)])?;
                }
                "rename_sheet" => {
                    let sheet = self.sheet(sheet_name)?;
                    self.uno
                        .call(&sheet, "setName", vec![op["new_name"].clone()])?;
                }
                "insert_rows" | "delete_rows" | "insert_columns" | "delete_columns" => {
                    let sheet = self.sheet(sheet_name)?;
                    let rows = kind.ends_with("rows");
                    let collection = self.uno.call(
                        &sheet,
                        if rows { "getRows" } else { "getColumns" },
                        vec![],
                    )?;
                    let at = if rows {
                        op["row"].as_u64().ok_or("invalid row")? - 1
                    } else {
                        operations::column_number(op["column"].as_str().ok_or("invalid column")?)?
                            as u64
                            - 1
                    };
                    self.uno.call(
                        &collection,
                        if kind.starts_with("insert") {
                            "insertByIndex"
                        } else {
                            "removeByIndex"
                        },
                        vec![json!(at), op["count"].clone()],
                    )?;
                }
                "upsert_chart" => self.chart(op)?,
                "upsert_pivot" => self.pivot(op)?,
                "refresh_pivot" => {
                    let sheet = self.sheet(sheet_name)?;
                    let tables = self.uno.call(&sheet, "getDataPilotTables", vec![])?;
                    let table = self
                        .uno
                        .call(&tables, "getByName", vec![op["name"].clone()])?;
                    self.uno.call(&table, "refresh", vec![])?;
                }
                _ => {
                    let area =
                        self.area(sheet_name, op["range"].as_str().ok_or("range required")?)?;
                    match kind {
                        "clear_range" => {
                            self.uno.call(&area, "clearContents", vec![json!(1023)])?;
                        }
                        "set_formula" => {
                            self.uno
                                .call(&area, "setFormula", vec![op["formula"].clone()])?;
                        }
                        "set_value" => {
                            let value = &op["value"];
                            if value.is_null() {
                                self.uno.call(&area, "clearContents", vec![json!(1023)])?;
                            } else if let Some(v) = value.as_bool() {
                                self.uno.call(
                                    &area,
                                    "setValue",
                                    vec![json!(if v { 1 } else { 0 })],
                                )?;
                            } else {
                                self.uno.call(
                                    &area,
                                    if value.is_number() {
                                        "setValue"
                                    } else {
                                        "setString"
                                    },
                                    vec![value.clone()],
                                )?;
                            }
                        }
                        "set_range_values" => {
                            let mut values = op["values"].clone();
                            for row in values.as_array_mut().ok_or("invalid matrix")? {
                                for value in row.as_array_mut().ok_or("invalid matrix row")? {
                                    if value.is_null() {
                                        *value = json!("");
                                    } else if let Some(v) = value.as_bool() {
                                        *value = json!(if v { 1 } else { 0 });
                                    }
                                }
                            }
                            self.uno.call(&area, "clearContents", vec![json!(1023)])?;
                            self.uno.call(&area, "setDataArray", vec![values])?;
                        }
                        "set_range_formulas" => {
                            self.uno.call(
                                &area,
                                "setFormulaArray",
                                vec![op["formulas"].clone()],
                            )?;
                        }
                        "format_cells" => {
                            if op.get("number_format").is_some() {
                                let key = self.number_format(&op["number_format"])?;
                                self.uno.set(&area, "NumberFormat", key)?;
                            }
                            for (key, prop) in [
                                ("bold", "CharWeight"),
                                ("text_color", "CharColor"),
                                ("background_color", "CellBackColor"),
                                ("wrap_text", "IsTextWrapped"),
                            ] {
                                if let Some(value) = op.get(key) {
                                    let v = match key {
                                        "bold" => json!(if value == true { 150.0 } else { 100.0 }),
                                        "text_color" | "background_color" => json!(
                                            u32::from_str_radix(
                                                &value.as_str().ok_or("invalid color")?[1..],
                                                16
                                            )
                                            .map_err(|e| e.to_string())?
                                        ),
                                        _ => value.clone(),
                                    };
                                    self.uno.set(&area, prop, v)?;
                                }
                            }
                        }
                        "fill_down" | "fill_right" => {
                            self.uno.call(
                                &area,
                                "fillAuto",
                                vec![
                                    enumeration(
                                        "sheet.FillDirection",
                                        if kind == "fill_down" {
                                            "TO_BOTTOM"
                                        } else {
                                            "TO_RIGHT"
                                        },
                                    ),
                                    op[if kind == "fill_down" {
                                        "source_rows"
                                    } else {
                                        "source_columns"
                                    }]
                                    .clone(),
                                ],
                            )?;
                        }
                        "sort_range" => {
                            let mut descriptor =
                                self.uno.call(&area, "createSortDescriptor", vec![])?;
                            for item in
                                descriptor.as_array_mut().ok_or("invalid sort descriptor")?
                            {
                                match item["fields"]["Name"].as_str() {
                                    Some("ContainsHeader") => {
                                        item["fields"]["Value"] = op["has_header"].clone()
                                    }
                                    Some("SortFields") => {
                                        item["fields"]["Value"] = json!({"$sequence":"[]com.sun.star.util.SortField","items":[structure("util.SortField",json!({"Field":op["key_column"].as_u64().ok_or("invalid sort key")?-1,"SortAscending":op["ascending"]}))]})
                                    }
                                    _ => {}
                                }
                            }
                            self.uno.call(&area, "sort", vec![descriptor])?;
                        }
                        _ => return Err("unsupported operation".into()),
                    }
                    self.uno.release(&area)?;
                }
            }
            self.uno.release(&sheets)?;
        }
        Ok(())
    }
    fn chart(&mut self, op: &Value) -> Result<()> {
        let target = self.sheet(op["sheet"].as_str().unwrap())?;
        let source = self.area(
            op["source_sheet"].as_str().unwrap(),
            op["source_range"].as_str().unwrap(),
        )?;
        let anchor = self.area(
            op["sheet"].as_str().unwrap(),
            op["anchor_range"].as_str().unwrap(),
        )?;
        let address = self.uno.call(&anchor, "getRangeAddress", vec![])?;
        let a = fields(&address);
        let start = self.uno.call(
            &target,
            "getCellByPosition",
            vec![a["StartColumn"].clone(), a["StartRow"].clone()],
        )?;
        let end = self.uno.call(
            &target,
            "getCellByPosition",
            vec![a["EndColumn"].clone(), a["EndRow"].clone()],
        )?;
        let start_position = self.uno.get(&start, "Position")?;
        let end_position = self.uno.get(&end, "Position")?;
        let end_size = self.uno.get(&end, "Size")?;
        let x = number(&fields(&start_position)["X"])?;
        let y = number(&fields(&start_position)["Y"])?;
        let rectangle = structure(
            "awt.Rectangle",
            json!({"X":x,"Y":y,"Width":1000.max(number(&fields(&end_position)["X"])?+number(&fields(&end_size)["Width"])?-x),"Height":1000.max(number(&fields(&end_position)["Y"])?+number(&fields(&end_size)["Height"])?-y)}),
        );
        let charts = self.uno.call(&target, "getCharts", vec![])?;
        if self
            .uno
            .call(&charts, "hasByName", vec![op["name"].clone()])?
            == true
        {
            self.uno
                .call(&charts, "removeByName", vec![op["name"].clone()])?;
        }
        let source_address = self.uno.call(&source, "getRangeAddress", vec![])?;
        self.uno.call(
            &charts,
            "addNewByName",
            vec![
                op["name"].clone(),
                rectangle,
                json!([source_address]),
                op["has_column_headers"].clone(),
                op["has_row_headers"].clone(),
            ],
        )?;
        let chart = self
            .uno
            .call(&charts, "getByName", vec![op["name"].clone()])?;
        let embedded = self.uno.call(&chart, "getEmbeddedObject", vec![])?;
        let kind = op["chart_type"].as_str().unwrap();
        let service = match kind {
            "column" | "bar" => "BarDiagram",
            "line" => "LineDiagram",
            "pie" => "PieDiagram",
            "scatter" => "XYDiagram",
            _ => return Err("invalid chart type".into()),
        };
        let diagram = self.uno.call(
            &embedded,
            "createInstance",
            vec![json!(format!("com.sun.star.chart.{service}"))],
        )?;
        if matches!(kind, "column" | "bar") {
            self.uno
                .set(&diagram, "Vertical", json!(kind == "column"))?;
        }
        self.uno.call(&embedded, "setDiagram", vec![diagram])?;
        self.uno.set(&embedded, "HasMainTitle", json!(true))?;
        let title = self.uno.get(&embedded, "Title")?;
        self.uno.set(&title, "String", op["title"].clone())?;
        self.uno.set(&embedded, "HasLegend", op["legend"].clone())?;
        Ok(())
    }
    fn pivot(&mut self, op: &Value) -> Result<()> {
        let target = self.sheet(op["sheet"].as_str().unwrap())?;
        let tables = self.uno.call(&target, "getDataPilotTables", vec![])?;
        if self
            .uno
            .call(&tables, "hasByName", vec![op["name"].clone()])?
            == true
        {
            self.uno
                .call(&tables, "removeByName", vec![op["name"].clone()])?;
        }
        let descriptor = self
            .uno
            .call(&tables, "createDataPilotDescriptor", vec![])?;
        let source = self.area(
            op["source_sheet"].as_str().unwrap(),
            op["source_range"].as_str().unwrap(),
        )?;
        let source_address = self.uno.call(&source, "getRangeAddress", vec![])?;
        self.uno
            .call(&descriptor, "setSourceRange", vec![source_address])?;
        let collection = self.uno.call(&descriptor, "getDataPilotFields", vec![])?;
        let count = number(&self.uno.call(&collection, "getCount", vec![])?)?;
        let mut fields = BTreeMap::new();
        for i in 0..count {
            let field = self.uno.call(&collection, "getByIndex", vec![json!(i)])?;
            let name = string(&self.uno.get(&field, "Name")?);
            fields.insert(name.to_lowercase(), field);
        }
        for (key, orientation) in [("rows", "ROW"), ("columns", "COLUMN"), ("filters", "PAGE")] {
            for name in array(&op[key])? {
                let field = fields
                    .get(&name.as_str().unwrap().to_lowercase())
                    .ok_or("pivot field not found")?;
                self.uno.set(
                    field,
                    "Orientation",
                    enumeration("sheet.DataPilotFieldOrientation", orientation),
                )?;
            }
        }
        for value in array(&op["values"])? {
            let field = fields
                .get(&value["field"].as_str().unwrap().to_lowercase())
                .ok_or("pivot value field not found")?;
            self.uno.set(
                field,
                "Orientation",
                enumeration("sheet.DataPilotFieldOrientation", "DATA"),
            )?;
            self.uno.set(
                field,
                "Function",
                enumeration(
                    "sheet.GeneralFunction",
                    &value["function"].as_str().unwrap().to_ascii_uppercase(),
                ),
            )?;
            if let Some(label) = value.get("label") {
                self.uno.set(field, "Name", label.clone())?;
            }
        }
        let cell = self.area(
            op["sheet"].as_str().unwrap(),
            op["output_cell"].as_str().unwrap(),
        )?;
        let address = self.uno.call(&cell, "getCellAddress", vec![])?;
        self.uno.call(
            &tables,
            "insertNewByName",
            vec![op["name"].clone(), address, descriptor],
        )?;
        Ok(())
    }
    fn targets(&mut self, ops: &[Value]) -> Result<Value> {
        let sheets = self.sheets()?;
        let mut targets = BTreeMap::<(String, String), Value>::new();
        for op in ops {
            if let (Some(sheet), Some(range)) = (op["sheet"].as_str(), op["range"].as_str()) {
                if self.uno.call(&sheets, "hasByName", vec![json!(sheet)])? != true {
                    continue;
                }
                let target = targets
                    .entry((sheet.into(), range.into()))
                    .or_insert_with(|| json!({"sheet":sheet,"range":range,"format":{}}));
                if op["type"] == "format_cells" {
                    let area = self.area(sheet, range)?;
                    target["format"].as_object_mut().unwrap().extend(
                        self.format_snapshot(&area, op)?
                            .as_object()
                            .unwrap()
                            .clone(),
                    );
                    self.uno.release(&area)?;
                }
            }
        }
        for ((sheet, range), target) in &mut targets {
            let area = self.area(sheet, range)?;
            target["values"] = self.uno.call(&area, "getDataArray", vec![])?;
            target["formulas"] = self.uno.call(&area, "getFormulaArray", vec![])?;
            self.uno.release(&area)?;
        }
        Ok(json!(targets.into_values().collect::<Vec<_>>()))
    }
    fn objects(&mut self, ops: &[Value]) -> Result<Value> {
        let mut charts = vec![];
        let mut pivots = vec![];
        let sheets = self.sheets()?;
        for op in ops {
            let sheet = op["sheet"].as_str().unwrap();
            if self.uno.call(&sheets, "hasByName", vec![json!(sheet)])? != true {
                continue;
            }
            let target = self.sheet(sheet)?;
            if op["type"] == "upsert_chart" {
                let collection = self.uno.call(&target, "getCharts", vec![])?;
                let mut title = None;
                if self
                    .uno
                    .call(&collection, "hasByName", vec![op["name"].clone()])?
                    == true
                {
                    let chart =
                        self.uno
                            .call(&collection, "getByName", vec![op["name"].clone()])?;
                    let embedded = self.uno.call(&chart, "getEmbeddedObject", vec![])?;
                    title = Some(self.chart_title(&embedded)?);
                } else {
                    let draw = self.uno.call(&target, "getDrawPage", vec![])?;
                    let count = number(&self.uno.call(&draw, "getCount", vec![])?)?;
                    if count > 10000 {
                        return Err("workbook draw-page exceeds object limit".into());
                    }
                    for i in 0..count {
                        let shape = self.uno.call(&draw, "getByIndex", vec![json!(i)])?;
                        if let Ok(model) = self.uno.get(&shape, "Model") {
                            if let Ok(candidate) = self.chart_title(&model) {
                                if candidate == op["title"] {
                                    title = Some(candidate);
                                    break;
                                }
                            }
                        }
                        self.uno.release(&shape)?;
                    }
                }
                if let Some(title) = title {
                    charts.push(json!({"sheet":sheet,"name":op["name"],"title":title}));
                }
            } else if op["type"] == "upsert_pivot" || op["type"] == "refresh_pivot" {
                let tables = self.uno.call(&target, "getDataPilotTables", vec![])?;
                if self
                    .uno
                    .call(&tables, "hasByName", vec![op["name"].clone()])?
                    == true
                {
                    let table = self
                        .uno
                        .call(&tables, "getByName", vec![op["name"].clone()])?;
                    let source = self.uno.call(&table, "getSourceRange", vec![])?;
                    let output = self.uno.call(&table, "getOutputRange", vec![])?;
                    let o = fields(&output);
                    let mut start = json!([o["Sheet"], o["StartColumn"], o["StartRow"]]);
                    if op["type"] == "upsert_pivot" {
                        let cell = self.area(sheet, op["output_cell"].as_str().unwrap())?;
                        let anchor = self.uno.call(&cell, "getCellAddress", vec![])?;
                        let a = fields(&anchor);
                        let row = number(&o["StartRow"])?;
                        let ar = number(&a["Row"])?;
                        if o["Sheet"] == a["Sheet"]
                            && o["StartColumn"] == a["Column"]
                            && (row == ar || row == ar + 1)
                        {
                            start = json!([a["Sheet"], a["Column"], a["Row"]]);
                        }
                    }
                    pivots.push(json!({"sheet":sheet,"name":op["name"],"source":address_values(&source),"output_start":start}));
                }
            }
        }
        Ok(json!({"charts":charts,"pivots":pivots}))
    }
    fn chart_title(&mut self, model: &Value) -> Result<Value> {
        if self.uno.get(model, "HasMainTitle")? == true {
            let title = self.uno.get(model, "Title")?;
            self.uno.get(&title, "String")
        } else {
            Ok(json!(""))
        }
    }
    fn store(&mut self, path: &Path, filter: &str, export: bool) -> Result<()> {
        // Output does not yet exist, so construct its URL from the existing parent.
        let parent = path.parent().ok_or("output needs parent")?;
        let base = crate::uno::file_url(parent)?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("invalid artifact filename")?;
        let url = format!("{base}/{name}");
        let d = self.doc.clone();
        self.uno.call(
            &d,
            if export { "storeToURL" } else { "storeAsURL" },
            vec![
                json!(url),
                json!([
                    property("FilterName", json!(filter)),
                    property("Overwrite", json!(false))
                ]),
            ],
        )?;
        Ok(())
    }
    fn recalculate(&mut self) -> Result<()> {
        let d = self.doc.clone();
        self.uno.call(&d, "calculateAll", vec![]).map(|_| ())
    }
    fn close(&mut self) -> Result<()> {
        let d = self.doc.clone();
        self.uno.call(&d, "close", vec![json!(true)]).map(|_| ())
    }
}
impl Drop for Workbook {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
fn address_values(v: &Value) -> Value {
    let a = fields(v);
    json!([
        a["Sheet"],
        a["StartColumn"],
        a["StartRow"],
        a["EndColumn"],
        a["EndRow"]
    ])
}
fn inventory(v: &Value) -> Value {
    json!({"sheets":v["sheets"].as_array().unwrap().iter().map(|s|json!({"name":s["name"],"used_range":s["used_range"]})).collect::<Vec<_>>(),"formula_count":v["formula_count"],"formula_errors":v["formula_errors"],"named_ranges":v["named_ranges"]})
}
fn target_changes(before: &Value, after: &Value) -> Value {
    let mut changes = BTreeMap::new();
    for (side, items) in [("before", before), ("after", after)] {
        for item in items.as_array().unwrap() {
            let key = (string(&item["sheet"]), string(&item["range"]));
            let change = changes
                .entry(key.clone())
                .or_insert_with(|| json!({"sheet":key.0,"range":key.1,"before":{},"after":{}}));
            change[side] = item.clone();
        }
    }
    json!(changes.into_values().collect::<Vec<_>>())
}
pub fn run(request: &Value, job: &Path) -> Result<Value> {
    let relative = Path::new(request["source"].as_str().ok_or("source required")?);
    if relative.parent() != Some(Path::new("input"))
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("source must be a private job input".into());
    }
    let source = job.join(relative);
    let action = request["action"].as_str().ok_or("action required")?;
    let args = &request["arguments"];
    let mut w = Workbook::load(job, &source, !matches!(action, "stage" | "convert_xls"))?;
    let read = match action {
        "describe" => Some(w.inspect(args["include_formulas"].as_bool().unwrap_or(false))?),
        "read_range" => Some(w.read(args)?),
        "search" => Some(w.search(args)?),
        "trace" => Some(w.trace(args)?),
        "query" => Some(w.query(args)?),
        "analyze" => Some(w.analyze(args)?),
        _ => None,
    };
    if let Some(result) = read {
        return Ok(json!({"result":result,"artifacts":{}}));
    }
    let out = job.join("out");
    if action == "render" {
        w.store(&out.join("preview.pdf"), "calc_pdf_Export", true)?;
        return Ok(
            json!({"result":{"format":"pdf","engine":{"name":"LibreOffice Calc"}},"artifacts":{"preview":"out/preview.pdf"}}),
        );
    }
    let ops = if action == "stage" {
        crate::policy::stageable(&source)?;
        operations::validate(&args["operations"])?
    } else if action == "convert_xls" {
        crate::policy::conversion_destination(&source)?;
        vec![]
    } else {
        return Err("unsupported Calc action".into());
    };
    let before = w.inspect(true)?;
    let before_targets = w.targets(&ops)?;
    let before_objects = w.objects(&ops)?;
    w.apply(&ops)?;
    w.recalculate()?;
    let expected = w.inspect(true)?;
    let expected_targets = w.targets(&ops)?;
    let expected_objects = w.objects(&ops)?;
    let ext = if action == "convert_xls" {
        "xlsx"
    } else {
        source
            .extension()
            .and_then(|s| s.to_str())
            .ok_or("missing format")?
    };
    let filter = match ext {
        "xlsx" => "Calc MS Excel 2007 XML",
        "ods" => "calc8",
        _ => return Err("unsupported writable workbook format".into()),
    };
    let workbook = out.join(format!("workbook.{ext}"));
    w.store(&workbook, filter, false)?;
    w.close()?;
    w.doc = w.uno.load(&workbook, true)?;
    w.recalculate()?;
    let after = w.inspect(true)?;
    let reopened_targets = w.targets(&ops)?;
    let reopened_objects = w.objects(&ops)?;
    w.store(&out.join("preview.pdf"), "calc_pdf_Export", true)?;
    let comparison = json!({"sheet_inventory_match":inventory(&expected)["sheets"]==inventory(&after)["sheets"],"named_ranges_match":expected["named_ranges"]==after["named_ranges"],"formula_count_match":expected["formula_count"]==after["formula_count"],"target_ranges_match":expected_targets==reopened_targets,"workbook_objects_match":expected_objects==reopened_objects,"new_formula_errors":array(&after["formula_errors"])?.iter().filter(|e|!array(&before["formula_errors"]).unwrap().contains(e)).cloned().collect::<Vec<_>>()});
    let failed: Vec<_> = [
        "sheet_inventory_match",
        "named_ranges_match",
        "formula_count_match",
        "target_ranges_match",
        "workbook_objects_match",
    ]
    .into_iter()
    .filter(|k| comparison[k] != true)
    .collect();
    if !failed.is_empty() {
        return Err(format!(
            "staged workbook did not survive save and reopen verification: {}",
            failed.join(", ")
        ));
    }
    let warnings = if action == "convert_xls" {
        vec!["Legacy conversion requires manual review in both Calc and the rendered PDF"]
    } else if ext == "xlsx"
        && ops
            .iter()
            .any(|op| op["type"] == "upsert_pivot" || op["type"] == "refresh_pivot")
    {
        vec![
            "Pivot tables in .xlsx may render differently in Microsoft Excel; review the staged workbook and PDF",
        ]
    } else {
        vec![]
    };
    Ok(
        json!({"result":{"semantic_diff":{"operation_count":ops.len(),"before":inventory(&before),"after":inventory(&after),"target_changes":target_changes(&before_targets,&expected_targets),"object_changes":{"before":before_objects,"after":expected_objects}},"verification":{"status":if action=="convert_xls"{"manual_review_required"}else{"verified"},"recalculated":true,"reopened":true,"filter_name":filter,"comparison":comparison,"excel_equivalence":"not_claimed"},"warnings":warnings,"engine":{"name":"LibreOffice Calc","filter_name":filter}},"artifacts":{"workbook":format!("out/workbook.{ext}"),"preview":"out/preview.pdf"}}),
    )
}
pub fn main(request: &Path, result: &Path) -> Result<i32> {
    let payload = files::read_json(request, 4 * 1024 * 1024)
        .and_then(|r| run(&r, request.parent().ok_or("job needs parent")?));
    let (code, value) = match payload {
        Ok(mut v) => {
            v["ok"] = json!(true);
            (0, v)
        }
        Err(e) => (
            1,
            json!({"ok":false,"error":e.chars().take(512).collect::<String>()}),
        ),
    };
    files::write_json(result, &value)?;
    Ok(code)
}
impl Workbook {
    pub fn query(&mut self, args: &Value) -> Result<Value> {
        let queries = args["queries"]
            .as_array()
            .ok_or("queries must be an array")?;
        if !(1..=8).contains(&queries.len()) {
            return Err("query batch must contain between 1 and 8 items".into());
        }
        let mut ids = std::collections::HashSet::new();
        let id_pattern = regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$").unwrap();
        for q in queries {
            let m = q.as_object().ok_or("invalid query item")?;
            if m.len() != 3
                || ["id", "tool", "arguments"]
                    .iter()
                    .any(|k| !m.contains_key(*k))
            {
                return Err("query item has invalid fields".into());
            }
            let id = q["id"].as_str().ok_or("invalid query id")?;
            if !id_pattern.is_match(id) || !ids.insert(id) {
                return Err("invalid or duplicate query id".into());
            }
            let (allowed, required): (&[&str], &[&str]) = match q["tool"].as_str() {
                Some("describe_workbook") => (&["include_formulas"], &[]),
                Some("read_range") => (
                    &["sheet", "range", "include_formulas", "include_styles"],
                    &["sheet", "range"],
                ),
                Some("search_workbook") => (&["query", "scope", "max_results"], &["query"]),
                Some("trace_formula") => (
                    &["sheet", "cell", "direction", "max_depth"],
                    &["sheet", "cell"],
                ),
                _ => return Err("unsupported batch read tool".into()),
            };
            let a = q["arguments"]
                .as_object()
                .ok_or("invalid query arguments")?;
            if a.keys().any(|k| !allowed.contains(&k.as_str()))
                || required.iter().any(|k| !a.contains_key(*k))
            {
                return Err("invalid query arguments".into());
            }
            for (k, v) in a {
                let valid = match k.as_str() {
                    "include_formulas" | "include_styles" => v.is_boolean(),
                    "max_results" => v.as_u64().is_some_and(|n| (1..=200).contains(&n)),
                    "max_depth" => v.as_u64().is_some_and(|n| (1..=10).contains(&n)),
                    "scope" => v
                        .as_str()
                        .is_some_and(|s| ["both", "values", "formulas"].contains(&s)),
                    "direction" => v
                        .as_str()
                        .is_some_and(|s| ["both", "precedents", "dependents"].contains(&s)),
                    _ => v.as_str().is_some_and(|s| {
                        !s.is_empty()
                            && s.chars().count()
                                <= if k == "query" {
                                    256
                                } else if k == "sheet" {
                                    128
                                } else {
                                    64
                                }
                    }),
                };
                if !valid {
                    return Err("invalid batch argument type or bound".into());
                }
            }
        }
        let mut items = vec![];
        for q in queries {
            let a = &q["arguments"];
            let result = match q["tool"].as_str().unwrap() {
                "describe_workbook" => {
                    self.inspect(a["include_formulas"].as_bool().unwrap_or(false))?
                }
                "read_range" => self.read(a)?,
                "search_workbook" => self.search(a)?,
                "trace_formula" => self.trace(a)?,
                _ => unreachable!(),
            };
            items.push(json!({"id":q["id"],"tool":q["tool"],"result":result}));
        }
        Ok(json!({"items":items}))
    }
    fn object_inventory(&mut self) -> Result<Value> {
        let sheets = self.sheets()?;
        let names = self.uno.call(&sheets, "getElementNames", vec![])?;
        let mut charts = vec![];
        let mut pivots = vec![];
        let mut truncated = false;
        for name in array(&names)? {
            let sheet = self.sheet(name.as_str().ok_or("invalid sheet name")?)?;
            for (kind, method) in [("charts", "getCharts"), ("pivots", "getDataPilotTables")] {
                let collection = self.uno.call(&sheet, method, vec![])?;
                let names = self.uno.call(&collection, "getElementNames", vec![])?;
                let mut names = array(&names)?.clone();
                names.sort_by_key(string);
                for object_name in names {
                    let items = if kind == "charts" {
                        &mut charts
                    } else {
                        &mut pivots
                    };
                    if items.len() >= 500 {
                        truncated = true;
                        break;
                    }
                    let obj = self
                        .uno
                        .call(&collection, "getByName", vec![object_name.clone()])?;
                    let mut item = json!({"sheet":name,"name":object_name});
                    if kind == "charts" {
                        item["title"] = json!("");
                        item["source_ranges"] = json!([]);
                        item["column_headers"] = json!(false);
                        item["row_headers"] = json!(false);
                        if let Ok(embedded) = self.uno.call(&obj, "getEmbeddedObject", vec![]) {
                            if let Ok(title) = self.chart_title(&embedded) {
                                item["title"] = title;
                            }
                            if let Ok(ranges) = self.uno.call(&obj, "getRanges", vec![]) {
                                item["source_ranges"] = json!(
                                    array(&ranges)?
                                        .iter()
                                        .map(address_values)
                                        .collect::<Vec<_>>()
                                );
                            }
                            if let Ok(v) = self.uno.call(&obj, "getHasColumnHeaders", vec![]) {
                                item["column_headers"] = v;
                            }
                            if let Ok(v) = self.uno.call(&obj, "getHasRowHeaders", vec![]) {
                                item["row_headers"] = v;
                            }
                        }
                    } else {
                        match (
                            self.uno.call(&obj, "getSourceRange", vec![]),
                            self.uno.call(&obj, "getOutputRange", vec![]),
                        ) {
                            (Ok(source), Ok(output)) => {
                                item["source"] = address_values(&source);
                                item["output"] = address_values(&output);
                            }
                            _ => item["details_unavailable"] = json!(true),
                        }
                    }
                    items.push(item);
                    self.uno.release(&obj)?;
                }
                self.uno.release(&collection)?;
            }
        }
        Ok(json!({"charts":charts,"pivots":pivots,"truncated":truncated}))
    }
    pub fn analyze(&mut self, args: &Value) -> Result<Value> {
        let focus = args["focus"].as_str().unwrap_or("all");
        if !["all", "quality", "management", "formulas"].contains(&focus) {
            return Err("invalid analysis focus".into());
        }
        let max = args["max_findings"].as_u64().unwrap_or(50);
        if !(1..=100).contains(&max) {
            return Err("invalid finding limit".into());
        }
        let mut findings = Findings {
            items: vec![],
            total: 0,
            max: max as usize,
        };
        let used = self.used()?;
        let sheet_count = used.len();
        let mut inspected = 0;
        let mut data_rows = 0;
        let mut formula_count = 0;
        let mut errors = vec![];
        let mut profiles = vec![];
        for (name, area, rows, cols) in used {
            inspected += rows * cols;
            data_rows += rows.saturating_sub(1);
            let values = self.uno.call(&area, "getDataArray", vec![])?;
            let formulas = self.uno.call(&area, "getFormulaArray", vec![])?;
            let headers: Vec<_> = array(&values[0])?
                .iter()
                .map(|v| string(v).trim().to_string())
                .collect();
            if rows > 1 && headers.iter().any(|h| h.is_empty()) {
                let blanks: Vec<_> = headers
                    .iter()
                    .enumerate()
                    .filter(|(_, h)| h.is_empty())
                    .map(|(i, _)| column_name(i))
                    .take(20)
                    .collect();
                findings.add(
                    "warning",
                    "missing_header",
                    &name,
                    &format!("A1:{}1", column_name(cols - 1)),
                    "The table has blank header cells.",
                    json!({"blank_columns":blanks}),
                );
            }
            let mut header_counts = BTreeMap::new();
            for h in &headers {
                if !h.is_empty() {
                    *header_counts.entry(h.to_lowercase()).or_insert(0) += 1;
                }
            }
            let duplicates: Vec<_> = header_counts
                .into_iter()
                .filter_map(|(h, n)| (n > 1).then_some(h))
                .take(20)
                .collect();
            if !duplicates.is_empty() {
                findings.add(
                    "warning",
                    "duplicate_header",
                    &name,
                    &format!("A1:{}1", column_name(cols - 1)),
                    "The table has duplicate column names.",
                    json!({"headers":duplicates}),
                );
            }
            let quality = matches!(focus, "all" | "quality");
            let mut seen = BTreeMap::new();
            let mut duplicate_examples = vec![];
            let mut duplicate_count = 0;
            let mut stats: Vec<_> = (0..cols).map(|_| ColumnStats::default()).collect();
            for (r, row) in array(&values)?.iter().enumerate() {
                let mut has_data = false;
                for (c, value) in array(row)?.iter().enumerate() {
                    let formula = string(&formulas[r][c]);
                    if formula.starts_with('=') {
                        formula_count += 1;
                        if formula_count > 20000 {
                            return Err("workbook exceeds the formula limit".into());
                        }
                        if r > 0 {
                            stats[c].formulas += 1;
                        }
                        if errors.len() < 200 {
                            let cell = self.uno.call(
                                &area,
                                "getCellByPosition",
                                vec![json!(c), json!(r)],
                            )?;
                            let code = self.uno.call(&cell, "getError", vec![])?;
                            let displayed = self.uno.call(&cell, "getString", vec![])?;
                            if number(&code)? != 0 || string(&displayed).starts_with('#') {
                                errors.push(json!({"sheet":name,"row":r+1,"column":c+1,"formula":formula,"error_code":code,"displayed":displayed}));
                            }
                            self.uno.release(&cell)?;
                        }
                    }
                    if r == 0 {
                        continue;
                    }
                    let s = &mut stats[c];
                    if value.is_null() || value == "" {
                        s.blanks += 1;
                        continue;
                    }
                    has_data = true;
                    s.populated += 1;
                    if s.populated <= 10000 {
                        s.distinct.insert(string(value));
                    }
                    if let Some(n) = value.as_f64().filter(|n| n.is_finite()) {
                        s.numeric.push(n);
                        s.sum += n;
                    }
                }
                if quality && r > 0 && has_data {
                    let hash = crate::transactions::canonical_hash(row)?;
                    if let Some(prior) = seen.get(&hash) {
                        duplicate_count += 1;
                        if duplicate_examples.len() < 10 {
                            duplicate_examples.push(json!({"row":r+1,"matches_row":prior}));
                        }
                    } else {
                        seen.insert(hash, r + 1);
                    }
                }
            }
            if !duplicate_examples.is_empty() {
                findings.add(
                    "warning",
                    "duplicate_rows",
                    &name,
                    &format!("A1:{}{rows}", column_name(cols - 1)),
                    "Duplicate data rows may distort totals.",
                    json!({"duplicate_count":duplicate_count,"examples":duplicate_examples}),
                );
            }
            let mut column_profiles = vec![];
            for (c, s) in stats.iter_mut().enumerate() {
                let header = if headers[c].is_empty() {
                    format!("Column {}", column_name(c))
                } else {
                    headers[c].clone()
                };
                let mut profile = json!({"column":column_name(c),"header":header,"populated":s.populated,"blanks":s.blanks,"distinct":s.distinct.len(),"numeric":s.numeric.len(),"formula_cells":s.formulas});
                if !s.numeric.is_empty() {
                    s.numeric.sort_by(f64::total_cmp);
                    profile["min"] = json!(s.numeric[0]);
                    profile["max"] = json!(s.numeric[s.numeric.len() - 1]);
                    profile["sum"] = json!(s.sum);
                    profile["mean"] = json!(s.sum / s.numeric.len() as f64);
                    if s.numeric.len() >= 8 && matches!(focus, "all" | "quality" | "management") {
                        let q1 = s.numeric[s.numeric.len() / 4];
                        let q3 = s.numeric[s.numeric.len() * 3 / 4];
                        let low = q1 - 1.5 * (q3 - q1);
                        let high = q3 + 1.5 * (q3 - q1);
                        let outliers: Vec<_> = s
                            .numeric
                            .iter()
                            .filter(|&&n| n < low || n > high)
                            .copied()
                            .collect();
                        if !outliers.is_empty() {
                            findings.add("notice","numeric_outliers",&name,&format!("{}2:{}{rows}",column_name(c),column_name(c)),&format!("{header} contains values outside the IQR range."),json!({"count":outliers.len(),"low":low,"high":high,"examples":outliers.into_iter().take(5).collect::<Vec<_>>()}));
                        }
                    }
                }
                if rows > 1 && quality && s.blanks as f64 / (rows - 1) as f64 >= 0.25 {
                    findings.add(
                        "notice",
                        "sparse_column",
                        &name,
                        &format!("{}2:{}{rows}", column_name(c), column_name(c)),
                        &format!("{header} is at least 25% blank."),
                        json!({"blank_count":s.blanks,"row_count":rows-1}),
                    );
                }
                column_profiles.push(profile);
            }
            profiles.push(json!({"sheet":name,"table_range":format!("A1:{}{rows}",column_name(cols-1)),"headers":headers,"columns":column_profiles}));
            self.uno.release(&area)?;
        }
        for e in &errors {
            findings.add(
                "error",
                "formula_error",
                e["sheet"].as_str().unwrap(),
                &format!(
                    "{}{}",
                    column_name(number(&e["column"])? as usize - 1),
                    number(&e["row"])?
                ),
                "A formula currently evaluates to an error.",
                json!({"displayed":e["displayed"],"error_code":e["error_code"]}),
            );
        }
        let mut opportunities = vec![];
        for profile in &profiles {
            let columns = profile["columns"].as_array().unwrap();
            let numeric: Vec<_> = columns
                .iter()
                .filter(|c| c["numeric"].as_u64().unwrap_or(0) > 0)
                .map(|c| c["header"].clone())
                .take(8)
                .collect();
            let dimension: Vec<_> = columns
                .iter()
                .filter(|c| c["populated"].as_u64().unwrap_or(0) > 0 && c["numeric"] == 0)
                .map(|c| c["header"].clone())
                .take(4)
                .collect();
            if !numeric.is_empty() {
                opportunities.push(json!({"sheet":profile["sheet"],"source_range":profile["table_range"],"recommended_chart":if dimension.is_empty(){"line"}else{"column"},"dimensions":dimension,"measures":numeric}));
            }
        }
        Ok(
            json!({"focus":focus,"summary":{"sheet_count":sheet_count,"inspected_cells":inspected,"data_rows":data_rows,"formula_count":formula_count,"formula_error_count":errors.len(),"finding_count":findings.items.len(),"finding_total":findings.total,"truncated":findings.total>findings.items.len()},"sheets":profiles,"findings":findings.items,"objects":self.object_inventory()?,"management_summary_opportunities":opportunities.into_iter().take(20).collect::<Vec<_>>(),"method":"deterministic_bounded_profile_v1"}),
        )
    }
}
#[derive(Default)]
struct ColumnStats {
    populated: usize,
    blanks: usize,
    distinct: std::collections::BTreeSet<String>,
    numeric: Vec<f64>,
    sum: f64,
    formulas: usize,
}
struct Findings {
    items: Vec<Value>,
    total: usize,
    max: usize,
}
impl Findings {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        severity: &str,
        category: &str,
        sheet: &str,
        range: &str,
        message: &str,
        metrics: Value,
    ) {
        self.total += 1;
        if self.items.len() < self.max {
            self.items.push(json!({"id":format!("F{:03}",self.items.len()+1),"severity":severity,"category":category,"sheet":sheet,"range":range,"message":message,"metrics":metrics}));
        }
    }
}
