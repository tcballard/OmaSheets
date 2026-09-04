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
    row_start: usize,
    column_start: usize,
    cells: BTreeMap<(usize, usize), GridCell>,
}

struct ServiceClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
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
        })
    }

    fn call(&mut self, request: &Value) -> Result<Value, String> {
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
        if envelope["ok"].as_bool() != Some(true) {
            let code = envelope["error"]["code"].as_str().unwrap_or("service");
            let message = envelope["error"]["message"]
                .as_str()
                .unwrap_or("request failed");
            return Err(format!("{code}: {message}"));
        }
        envelope
            .get_mut("response")
            .map(Value::take)
            .ok_or_else(|| "service response is missing its payload".into())
    }
}

struct DocumentState {
    client: ServiceClient,
    path: PathBuf,
    branch: Option<String>,
    sheet: String,
    actor: String,
    pages: VecDeque<CachedPage>,
    requests: u64,
}

pub struct GridDocument {
    pub name: String,
    pub sheet_name: String,
    pub rows: usize,
    pub columns: usize,
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
        let sheet = summary["sheets"]
            .as_array()
            .and_then(|sheets| sheets.first())
            .ok_or("the document has no sheets")?;
        let sheet_id = sheet["id"]
            .as_str()
            .ok_or("the first sheet has no stable ID")?
            .to_string();
        let actor = std::env::var("OMASHEETS_ACTOR")
            .ok()
            .filter(|actor| !actor.trim().is_empty() && actor.chars().count() <= 128)
            .or_else(|| std::env::var("USER").ok())
            .filter(|actor| !actor.trim().is_empty() && actor.chars().count() <= 128)
            .unwrap_or_else(|| "local-user".into());
        Ok(Self {
            name: summary["name"].as_str().unwrap_or("Untitled").to_string(),
            sheet_name: sheet["name"].as_str().unwrap_or("Sheet").to_string(),
            rows: sheet["rows"].as_u64().unwrap_or(0) as usize,
            columns: sheet["columns"].as_u64().unwrap_or(0) as usize,
            state: Mutex::new(DocumentState {
                client,
                path: path.to_path_buf(),
                branch,
                sheet: sheet_id,
                actor,
                pages: VecDeque::new(),
                requests: 2,
            }),
        })
    }

    pub fn cell(&self, row: usize, column: usize) -> Result<GridCell, String> {
        if row >= self.rows || column >= self.columns {
            return Ok(GridCell::default());
        }
        let row_start = row / PAGE_ROWS * PAGE_ROWS;
        let column_start = column / PAGE_COLUMNS * PAGE_COLUMNS;
        let mut state = self.state.lock().map_err(|_| "grid cache is poisoned")?;
        let position = state
            .pages
            .iter()
            .position(|page| page.row_start == row_start && page.column_start == column_start);
        let page = match position {
            Some(position) => state.pages.remove(position).expect("position was found"),
            None => fetch_page(&mut state, row_start, column_start)?,
        };
        let cell = page.cells.get(&(row, column)).cloned().unwrap_or_default();
        state.pages.push_back(page);
        while state.pages.len() > MAX_CACHED_PAGES {
            state.pages.pop_front();
        }
        Ok(cell)
    }

    pub fn set_text(&self, row: usize, column: usize, text: &str) -> Result<(), String> {
        if row >= self.rows || column >= self.columns {
            return Err("cell is outside the document view".into());
        }
        let mut state = self.state.lock().map_err(|_| "grid cache is poisoned")?;
        let a1 = format!("{}{}", column_letters(column), row + 1);
        let command = edit_command(&state.sheet, &a1, text);
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

    pub fn request_count(&self) -> u64 {
        self.state.lock().map_or(0, |state| state.requests)
    }
}

fn fetch_page(
    state: &mut DocumentState,
    row_start: usize,
    column_start: usize,
) -> Result<CachedPage, String> {
    let request = json!({
        "kind": "grid_page",
        "path": &state.path,
        "branch": state.branch.as_deref(),
        "sheet": state.sheet.as_str(),
        "row_start": row_start,
        "column_start": column_start,
        "rows": PAGE_ROWS,
        "columns": PAGE_COLUMNS,
    });
    let response = state.client.call(&request)?;
    state.requests += 1;
    let mut cells = BTreeMap::new();
    for cell in response["cells"].as_array().into_iter().flatten() {
        let Some(row) = cell["row"].as_u64().map(|value| value as usize) else {
            continue;
        };
        let Some(column) = cell["column"].as_u64().map(|value| value as usize) else {
            continue;
        };
        cells.insert((row, column), parse_cell(cell));
    }
    Ok(CachedPage {
        row_start,
        column_start,
        cells,
    })
}

fn parse_cell(cell: &Value) -> GridCell {
    let value = &cell["value"];
    let value_type = value["type"].as_str().unwrap_or("blank");
    let display = match value_type {
        "number" => value["value"]
            .as_f64()
            .map(format_number)
            .unwrap_or_default(),
        "text" | "error" => value["value"].as_str().unwrap_or("").to_string(),
        "boolean" => match value["value"].as_bool() {
            Some(true) => "TRUE".into(),
            Some(false) => "FALSE".into(),
            None => String::new(),
        },
        _ => String::new(),
    };
    let formula = cell["formula"].as_str();
    let input_text = if let Some(formula) = formula {
        formula.to_string()
    } else {
        display.clone()
    };
    GridCell {
        display,
        input: input_text,
        kind: if formula.is_some() { "formula" } else { value_type }.into(),
    }
}

fn edit_command(sheet: &str, a1: &str, text: &str) -> Value {
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

    #[test]
    fn parses_values_and_preserves_formula_input() {
        let formula = json!({
            "value": { "type": "number", "value": 8.0 },
            "formula": "=A1*2"
        });
        assert_eq!(
            parse_cell(&formula),
            GridCell { display: "8".into(), input: "=A1*2".into(), kind: "formula".into() }
        );
        let error = json!({
            "value": { "type": "error", "value": "#REF!" }
        });
        assert_eq!(parse_cell(&error).display, "#REF!");
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
    fn page_and_cache_bounds_fit_the_service_contract() {
        assert!(PAGE_ROWS * PAGE_COLUMNS <= 10_000);
        assert_eq!(MAX_CACHED_PAGES * PAGE_ROWS * PAGE_COLUMNS, 8_192);
    }
}
