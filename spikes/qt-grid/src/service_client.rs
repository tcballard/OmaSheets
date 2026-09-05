use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const PAGE_ROWS: usize = 64;
const PAGE_COLUMNS: usize = 16;
const MAX_CACHED_PAGES: usize = 8;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridCell {
    pub display: String,
    pub input: String,
    pub kind: String,
}

#[derive(Debug)]
struct CachedPage {
    sheet: String,
    row_start: usize,
    column_start: usize,
    cells: BTreeMap<(usize, usize), GridCell>,
}

struct ServiceClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    usable: bool,
}

impl ServiceClient {
    fn connect() -> Result<Self, String> {
        let directory = runtime_directory()?;
        let token = fs::read_to_string(directory.join("native.token"))
            .map_err(|error| format!("cannot read the service token: {error}"))?;
        let stream = UnixStream::connect(directory.join("native.sock"))
            .map_err(|error| format!("cannot reach the local service: {error}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .map_err(|error| error.to_string())?;
        let mut writer = stream.try_clone().map_err(|error| error.to_string())?;
        writeln!(writer, "{}", token.trim()).map_err(|error| error.to_string())?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
            usable: true,
        })
    }

    fn call(&mut self, request: &Value) -> Result<Value, String> {
        if !self.usable {
            return Err("service connection lost synchronization; copy your draft and reopen the document to verify whether the previous edit was saved".into());
        }
        // A failed exchange may leave a delayed response in the stream. Never
        // send another request until this exchange has a complete valid reply.
        self.usable = false;
        serde_json::to_writer(&mut self.writer, request).map_err(|error| error.to_string())?;
        self.writer
            .write_all(b"\n")
            .and_then(|()| self.writer.flush())
            .map_err(|error| error.to_string())?;
        let mut answer = String::new();
        self.reader
            .by_ref()
            .take(MAX_RESPONSE_BYTES as u64)
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        if !answer.ends_with('\n') {
            return Err("service response is empty or exceeds the size limit".into());
        }
        let mut envelope: Value =
            serde_json::from_str(answer.trim()).map_err(|error| error.to_string())?;
        let ok = envelope["ok"]
            .as_bool()
            .ok_or("service response is missing its status")?;
        if !ok {
            let code = envelope["error"]["code"].as_str().unwrap_or("service");
            let message = envelope["error"]["message"]
                .as_str()
                .unwrap_or("request failed");
            self.usable = true;
            return Err(format!("{code}: {message}"));
        }
        let response = envelope
            .get_mut("response")
            .map(Value::take)
            .ok_or_else(|| "service response is missing its payload".to_string())?;
        self.usable = true;
        Ok(response)
    }
}

struct DocumentState {
    client: ServiceClient,
    path: PathBuf,
    branch: Option<String>,
    current_sheet: usize,
    actor: String,
    pages: VecDeque<CachedPage>,
    requests: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetInfo {
    pub id: String,
    pub name: String,
    pub rows: usize,
    pub columns: usize,
}

pub struct GridDocument {
    pub name: String,
    pub sheets: Vec<SheetInfo>,
    state: Mutex<DocumentState>,
}

impl GridDocument {
    pub fn open(path: &Path, branch: Option<String>) -> Result<Self, String> {
        let mut client = ServiceClient::connect()?;
        client.call(&json!({ "kind": "open", "path": path }))?;
        let summary = client.call(&json!({
            "kind": "document",
            "path": path,
            "branch": branch.as_deref(),
        }))?;
        let sheets = summary["sheets"]
            .as_array()
            .ok_or("the document summary has no sheet list")?
            .iter()
            .map(parse_sheet)
            .collect::<Result<Vec<_>, _>>()?;
        if sheets.is_empty() {
            return Err("the document has no sheets".into());
        }
        let actor = std::env::var("OMASHEETS_ACTOR")
            .ok()
            .filter(|actor| !actor.trim().is_empty() && actor.chars().count() <= 128)
            .or_else(|| std::env::var("USER").ok())
            .filter(|actor| !actor.trim().is_empty() && actor.chars().count() <= 128)
            .unwrap_or_else(|| "local-user".into());
        Ok(Self {
            name: summary["name"].as_str().unwrap_or("Untitled").to_string(),
            sheets,
            state: Mutex::new(DocumentState {
                client,
                path: path.to_path_buf(),
                branch,
                current_sheet: 0,
                actor,
                pages: VecDeque::new(),
                requests: 2,
            }),
        })
    }

    pub fn cell(&self, row: usize, column: usize) -> Result<GridCell, String> {
        let mut state = self.state.lock().map_err(|_| "grid cache is poisoned")?;
        let sheet = self
            .sheets
            .get(state.current_sheet)
            .ok_or("the current sheet is unavailable")?;
        if row >= sheet.rows || column >= sheet.columns {
            return Ok(GridCell::default());
        }
        let row_start = row / PAGE_ROWS * PAGE_ROWS;
        let column_start = column / PAGE_COLUMNS * PAGE_COLUMNS;
        let position = state
            .pages
            .iter()
            .position(|page| {
                page.sheet == sheet.id
                    && page.row_start == row_start
                    && page.column_start == column_start
            });
        let page = match position {
            Some(position) => state.pages.remove(position).expect("position was found"),
            None => fetch_page(&mut state, &sheet.id, row_start, column_start)?,
        };
        let cell = page.cells.get(&(row, column)).cloned().unwrap_or_default();
        state.pages.push_back(page);
        while state.pages.len() > MAX_CACHED_PAGES {
            state.pages.pop_front();
        }
        Ok(cell)
    }

    pub fn set_text(&self, row: usize, column: usize, text: &str) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "grid cache is poisoned")?;
        let sheet = self
            .sheets
            .get(state.current_sheet)
            .ok_or("the current sheet is unavailable")?;
        if row >= sheet.rows || column >= sheet.columns {
            return Err("cell is outside the document view".into());
        }
        let a1 = format!("{}{}", column_letters(column), row + 1);
        let command = edit_command(&sheet.id, &a1, text);
        let request = json!({
            "kind": "append",
            "path": &state.path,
            "branch": state.branch.as_deref(),
            "actor": { "kind": "human", "id": state.actor.as_str() },
            "command": command,
        });
        state.client.call(&request)?;
        state.requests += 1;
        state.pages.clear();
        Ok(())
    }

    pub fn current_sheet(&self) -> Result<SheetInfo, String> {
        let state = self.state.lock().map_err(|_| "grid cache is poisoned")?;
        self.sheets
            .get(state.current_sheet)
            .cloned()
            .ok_or_else(|| "the current sheet is unavailable".into())
    }

    pub fn current_sheet_index(&self) -> usize {
        self.state.lock().map_or(0, |state| state.current_sheet)
    }

    pub fn select_sheet(&self, index: usize) -> Result<SheetInfo, String> {
        let sheet = self
            .sheets
            .get(index)
            .cloned()
            .ok_or_else(|| "sheet index is outside the document".to_string())?;
        let mut state = self.state.lock().map_err(|_| "grid cache is poisoned")?;
        if state.current_sheet != index {
            state.current_sheet = index;
            state.pages.clear();
        }
        Ok(sheet)
    }

    pub fn request_count(&self) -> u64 {
        self.state.lock().map_or(0, |state| state.requests)
    }
}

fn fetch_page(
    state: &mut DocumentState,
    sheet: &str,
    row_start: usize,
    column_start: usize,
) -> Result<CachedPage, String> {
    let request = json!({
        "kind": "grid_page",
        "path": &state.path,
        "branch": state.branch.as_deref(),
        "sheet": sheet,
        "row_start": row_start,
        "column_start": column_start,
        "rows": PAGE_ROWS,
        "columns": PAGE_COLUMNS,
    });
    let response = state.client.call(&request)?;
    state.requests += 1;
    let page = parse_page(&response, sheet, row_start, column_start);
    if page.is_err() {
        // Do not repeat a malformed page request for every visible delegate.
        state.client.usable = false;
    }
    page
}

fn parse_page(
    response: &Value,
    sheet: &str,
    row_start: usize,
    column_start: usize,
) -> Result<CachedPage, String> {
    if response["kind"] != "grid_page"
        || response["sheet"].as_str() != Some(sheet)
        || response["row_start"].as_u64() != Some(row_start as u64)
        || response["column_start"].as_u64() != Some(column_start as u64)
    {
        return Err("service returned a different grid page".into());
    }
    let rows = response["rows"].as_u64().filter(|rows| *rows <= PAGE_ROWS as u64)
        .ok_or("service returned invalid page dimensions")?;
    let columns = response["columns"].as_u64().filter(|columns| *columns <= PAGE_COLUMNS as u64)
        .ok_or("service returned invalid page dimensions")?;
    let entries = response["cells"].as_array().ok_or("service page has no cell list")?;
    if entries.len() as u64 > rows * columns {
        return Err("service page contains too many cells".into());
    }
    let mut cells = BTreeMap::new();
    for cell in entries {
        let row = cell["row"].as_u64().ok_or("service cell has no row")?;
        let column = cell["column"].as_u64().ok_or("service cell has no column")?;
        if row.checked_sub(row_start as u64).is_none_or(|offset| offset >= rows)
            || column.checked_sub(column_start as u64).is_none_or(|offset| offset >= columns)
        {
            return Err("service cell is outside the requested page".into());
        }
        if cells.insert((row as usize, column as usize), parse_cell(cell)?).is_some() {
            return Err("service page contains a duplicate cell".into());
        }
    }
    Ok(CachedPage {
        sheet: sheet.to_string(),
        row_start,
        column_start,
        cells,
    })
}

fn parse_sheet(sheet: &Value) -> Result<SheetInfo, String> {
    Ok(SheetInfo {
        id: sheet["id"]
            .as_str()
            .ok_or("a sheet has no stable ID")?
            .to_string(),
        name: sheet["name"].as_str().unwrap_or("Sheet").to_string(),
        rows: sheet["rows"].as_u64().unwrap_or(0) as usize,
        columns: sheet["columns"].as_u64().unwrap_or(0) as usize,
    })
}

fn parse_cell(cell: &Value) -> Result<GridCell, String> {
    let value = &cell["value"];
    let value_type = value["type"].as_str().ok_or("service cell has no value type")?;
    let display = match value_type {
        "number" => value["value"]
            .as_f64()
            .map(format_number)
            .ok_or("service cell has an invalid number")?,
        "text" | "error" => value["value"].as_str().ok_or("service cell has invalid text")?.to_string(),
        "boolean" => match value["value"].as_bool() {
            Some(true) => "TRUE".into(),
            Some(false) => "FALSE".into(),
            None => return Err("service cell has an invalid boolean".into()),
        },
        "blank" => String::new(),
        _ => return Err("service cell has an unknown value type".into()),
    };
    if cell.get("formula").is_some_and(|formula| !formula.is_null() && !formula.is_string()) {
        return Err("service cell has an invalid formula".into());
    }
    let formula = cell["formula"].as_str();
    let input_text = if let Some(formula) = formula {
        formula.to_string()
    } else if value_type == "text"
        && (display.is_empty() || display.starts_with(['\'', '='])
            || display.eq_ignore_ascii_case("true") || display.eq_ignore_ascii_case("false")
            || display.parse::<f64>().is_ok_and(|number| number.is_finite()))
    {
        format!("'{display}")
    } else {
        display.clone()
    };
    Ok(GridCell {
        display,
        input: input_text,
        kind: if formula.is_some() { "formula" } else { value_type }.into(),
    })
}

fn edit_command(sheet: &str, a1: &str, text: &str) -> Value {
    if let Some(literal) = text.strip_prefix('\'') {
        return json!({ "command": "set_value", "sheet": sheet, "a1": a1,
            "value": { "type": "text", "value": literal } });
    }
    if text.is_empty() {
        return json!({ "command": "clear_cell", "sheet": sheet, "a1": a1 });
    }
    if text.starts_with('=') {
        return json!({ "command": "set_formula", "sheet": sheet, "a1": a1, "source": text });
    }
    let value = if text.eq_ignore_ascii_case("true") {
        json!({ "type": "boolean", "value": true })
    } else if text.eq_ignore_ascii_case("false") {
        json!({ "type": "boolean", "value": false })
    } else if let Ok(number) = text.parse::<f64>()
        && number.is_finite()
    {
        json!({ "type": "number", "value": number })
    } else {
        json!({ "type": "text", "value": text })
    };
    json!({ "command": "set_value", "sheet": sheet, "a1": a1, "value": value })
}

fn runtime_directory() -> Result<PathBuf, String> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .map(|directory| directory.join("omasheets"))
        .ok_or_else(|| "XDG_RUNTIME_DIR is not set".into())
}

fn format_number(number: f64) -> String {
    if number == 0.0 {
        "0".into()
    } else {
        number.to_string()
    }
}

fn column_letters(column: usize) -> String {
    let mut index = column;
    let mut letters = Vec::new();
    loop {
        letters.push(b'A' + (index % 26) as u8);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    letters.reverse();
    String::from_utf8(letters).expect("ASCII column label")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected_pair() -> (ServiceClient, UnixStream) {
        let (stream, peer) = UnixStream::pair().unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let writer = stream.try_clone().unwrap();
        (ServiceClient { reader: BufReader::new(stream), writer, usable: true }, peer)
    }

    #[test]
    fn incomplete_exchange_never_sends_a_second_edit() {
        for reply in ["", "not-json\n", "{}\n", "{\"ok\":true}\n"] {
            let (mut client, mut peer) = connected_pair();
            peer.write_all(reply.as_bytes()).unwrap();
            assert!(client.call(&json!({"edit": 1})).is_err());
            let mut received = BufReader::new(peer.try_clone().unwrap());
            let mut request = String::new();
            received.read_line(&mut request).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&request).unwrap()["edit"], 1);
            // A late successful response must not acknowledge a new mutation.
            peer.write_all(b"{\"ok\":true,\"response\":{}}\n").unwrap();
            assert!(client.call(&json!({"edit": 2})).unwrap_err().contains("reopen"));
            peer.set_nonblocking(true).unwrap();
            let mut byte = [0];
            assert_eq!(peer.read(&mut byte).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        }
    }

    #[test]
    fn complete_rejection_keeps_connection_usable() {
        let (mut client, mut peer) = connected_pair();
        peer.write_all(b"{\"ok\":false,\"error\":{\"code\":\"validation\",\"message\":\"rejected\"}}\n").unwrap();
        assert_eq!(client.call(&json!({})).unwrap_err(), "validation: rejected");
        peer.write_all(b"{\"ok\":true,\"response\":{\"saved\":true}}\n").unwrap();
        assert_eq!(client.call(&json!({})).unwrap()["saved"], true);
    }

    #[test]
    fn parses_values_and_preserves_formula_input() {
        let formula = json!({
            "value": { "type": "number", "value": 8.0 },
            "formula": "=A1*2"
        });
        assert_eq!(
            parse_cell(&formula).unwrap(),
            GridCell { display: "8".into(), input: "=A1*2".into(), kind: "formula".into() }
        );
        let error = json!({
            "value": { "type": "error", "value": "#REF!" }
        });
        assert_eq!(parse_cell(&error).unwrap().display, "#REF!");
    }

    #[test]
    fn grid_pages_reject_malformed_data_instead_of_showing_blanks() {
        let page = json!({
            "kind": "grid_page", "sheet": "sheet-id", "row_start": 64,
            "column_start": 16, "rows": 2, "columns": 2,
            "cells": [{"row": 64, "column": 16, "value": {"type": "text", "value": "<b>literal</b>"}}]
        });
        let parsed = parse_page(&page, "sheet-id", 64, 16).unwrap();
        assert_eq!(parsed.cells[&(64, 16)].display, "<b>literal</b>");
        assert_eq!(parsed.cells.len(), 1);
        for (key, invalid) in [
            ("kind", json!("document")), ("sheet", json!("other-sheet")),
            ("row_start", json!(0)), ("column_start", json!(0)),
            ("rows", json!(65)), ("columns", json!(17)), ("cells", Value::Null),
        ] {
            let mut changed = page.clone();
            changed[key] = invalid;
            assert!(parse_page(&changed, "sheet-id", 64, 16).is_err(), "{key}");
        }
        for invalid_cell in [
            json!({"row": 63, "column": 16, "value": {"type": "blank"}}),
            json!({"row": 66, "column": 16, "value": {"type": "blank"}}),
            json!({"row": 64, "column": 18, "value": {"type": "blank"}}),
            json!({"row": 64, "value": {"type": "blank"}}),
            json!({"row": 64, "column": 16, "value": {"type": "number", "value": "bad"}}),
            json!({"row": 64, "column": 16, "value": {"type": "mystery"}}),
        ] {
            let mut changed = page.clone();
            changed["cells"] = json!([invalid_cell]);
            assert!(parse_page(&changed, "sheet-id", 64, 16).is_err());
        }
        let mut duplicate = page.clone();
        duplicate["cells"] = json!([page["cells"][0], page["cells"][0]]);
        assert!(parse_page(&duplicate, "sheet-id", 64, 16).is_err());
        let mut empty = page;
        empty["cells"] = json!([]);
        assert!(parse_page(&empty, "sheet-id", 64, 16).unwrap().cells.is_empty());
    }

    #[test]
    fn edits_use_native_command_types() {
        assert_eq!(edit_command("sheet-id", "A1", "")["command"], "clear_cell");
        assert_eq!(edit_command("sheet-id", "A1", "=1+1")["command"], "set_formula");
        assert_eq!(edit_command("sheet-id", "A1", "12.5")["value"]["type"], "number");
        assert_eq!(edit_command("sheet-id", "A1", "TRUE")["value"]["type"], "boolean");
        assert_eq!(edit_command("sheet-id", "A1", "hello")["value"]["type"], "text");
    }

    #[test]
    fn literal_text_round_trips_without_coercion() {
        for text in ["00123", "TRUE", "false", "=SUM(A1:A2)", "'quoted", "", "hello", "<b>text</b>"] {
            let cell = parse_cell(&json!({"value": {"type": "text", "value": text}})).unwrap();
            assert_eq!(cell.display, text);
            let command = edit_command("sheet-id", "A1", &cell.input);
            assert_eq!(command["command"], "set_value");
            assert_eq!(command["value"], json!({"type": "text", "value": text}));
        }
        assert_eq!(edit_command("sheet-id", "A1", "'00123")["value"]["value"], "00123");
        assert_eq!(edit_command("sheet-id", "A1", "'=1+1")["value"]["type"], "text");
        assert_eq!(edit_command("sheet-id", "A1", "'TRUE")["value"]["type"], "text");
    }

    #[test]
    fn page_and_cache_bounds_fit_the_service_contract() {
        assert!(PAGE_ROWS * PAGE_COLUMNS <= 10_000);
        assert_eq!(MAX_CACHED_PAGES * PAGE_ROWS * PAGE_COLUMNS, 8_192);
    }

    #[test]
    fn parses_stable_sheet_metadata() {
        let sheet = parse_sheet(&json!({
            "id": "sheet-id",
            "name": "Forecast",
            "rows": 256,
            "columns": 16
        }))
        .expect("valid sheet metadata");
        assert_eq!(
            sheet,
            SheetInfo {
                id: "sheet-id".into(),
                name: "Forecast".into(),
                rows: 256,
                columns: 16,
            }
        );
    }
}
