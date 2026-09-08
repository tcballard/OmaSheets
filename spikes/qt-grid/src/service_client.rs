use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const PAGE_ROWS: usize = 64;
const PAGE_COLUMNS: usize = 16;
const MAX_CACHED_PAGES: usize = 32;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridCell {
    pub display: String,
    pub input: String,
    pub kind: String,
    pub presentation: String,
}

#[derive(Debug)]
struct CachedPage {
    sheet: String,
    row_start: usize,
    column_start: usize,
    cells: BTreeMap<(usize, usize), GridCell>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PageKey {
    sheet: String,
    row: usize,
    column: usize,
}

struct PageReply {
    generation: u64,
    key: PageKey,
    page: Result<CachedPage, String>,
}

struct PageLoader {
    requests: mpsc::SyncSender<(u64, PageKey)>,
    replies: mpsc::Receiver<PageReply>,
    generation: Arc<AtomicU64>,
    pending: BTreeSet<PageKey>,
    error: Option<String>,
}

impl PageLoader {
    fn spawn(
        path: PathBuf,
        branch: Option<String>,
        connect: impl FnOnce() -> Result<ServiceClient, String> + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> Self {
        let (requests, work) = mpsc::sync_channel::<(u64, PageKey)>(MAX_CACHED_PAGES);
        let (answers, replies) = mpsc::sync_channel(MAX_CACHED_PAGES);
        let generation = Arc::new(AtomicU64::new(0));
        let current = Arc::clone(&generation);
        std::thread::spawn(move || {
            let mut client = connect();
            while let Ok((generation, key)) = work.recv() {
                if generation != current.load(Ordering::Acquire) {
                    // Wake the viewport when cancelled work frees queue space.
                    if answers
                        .send(PageReply {
                            generation,
                            key,
                            page: Err("obsolete page".into()),
                        })
                        .is_err()
                    {
                        break;
                    }
                    notify();
                    continue;
                }
                let page = match &mut client {
                    Ok(client) => client
                        .call(&page_request(
                            &path, &branch, &key.sheet, key.row, key.column,
                        ))
                        .and_then(|response| {
                            let page = parse_page(&response, &key.sheet, key.row, key.column);
                            if page.is_err() {
                                client.usable = false;
                            }
                            page
                        }),
                    Err(error) => Err(error.clone()),
                };
                if answers
                    .send(PageReply {
                        generation,
                        key,
                        page,
                    })
                    .is_err()
                {
                    break;
                }
                notify();
            }
        });
        Self {
            requests,
            replies,
            generation,
            pending: BTreeSet::new(),
            error: None,
        }
    }

    fn request(&mut self, key: PageKey) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if self.pending.len() < MAX_CACHED_PAGES && !self.pending.contains(&key) {
            match self
                .requests
                .try_send((self.generation.load(Ordering::Acquire), key.clone()))
            {
                Ok(()) => {
                    self.pending.insert(key);
                }
                Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err("page loader stopped; reopen the document".into());
                }
            }
        }
        Ok(())
    }

    fn invalidate(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.pending.clear();
    }
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

/// File actions use their own connection so a slow transfer cannot desynchronize
/// the editing connection. Call from a worker, never from the render thread.
pub(crate) fn desktop_call(request: &Value) -> Result<Value, String> {
    let mut client = ServiceClient::connect()?;
    client
        .writer
        .set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|error| error.to_string())?;
    client.call(request)
}

/// Publish only the user-selected workbook to the bounded agent bridge.
pub(crate) fn publish_agent_session(
    path: &Path,
    sheet: &str,
    row: i32,
    column: i32,
    rows: i32,
    columns: i32,
) -> Result<String, String> {
    use std::os::unix::fs::OpenOptionsExt;
    if !path.is_absolute() || row < 0 || column < 0 || rows <= 0 || columns <= 0 {
        return Err("Invalid agent selection".into());
    }
    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|e| e.to_string())?;
    let session_id = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let directory = runtime_directory()?;
    let temporary = directory.join(format!("native-agent-session-{session_id}.tmp"));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut file, &json!({"schema": 1, "session_id": session_id,
            "pid": std::process::id(), "path": path,
            "selection": {"sheet": sheet, "row": row, "column": column, "rows": rows, "columns": columns}}))
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&temporary, directory.join("native-agent-session.json"))
            .map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map(|()| session_id)
}

fn clear_agent_session(path: &Path) {
    let Ok(directory) = runtime_directory() else {
        return;
    };
    let context = directory.join("native-agent-session.json");
    let selected = fs::read(&context)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if selected.is_some_and(|value| {
        value["pid"].as_u64() == Some(std::process::id().into())
            && value["path"] == path.to_string_lossy().as_ref()
    }) {
        let _ = fs::remove_file(context);
    }
}

pub(crate) fn create_workbook(path: &Path) -> Result<(), String> {
    let actor = json!({"kind": "human", "id": "omasheets-desktop"});
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled");
    desktop_call(&json!({"kind": "create", "path": path, "name": name, "actor": actor}))?;
    // Creation never overwrites a file. Keep a partially created document on
    // failure: deleting it after a lost reply could destroy confirmed work.
    let sheet = desktop_call(&json!({"kind": "append", "path": path, "actor": actor,
        "command": {"command": "add_sheet", "name": "Sheet1"}}))?;
    let sheet = sheet["operation"]["sheet"]
        .as_str()
        .ok_or("new sheet has no identity")?;
    desktop_call(
        &json!({"kind": "append_batch", "path": path, "actor": actor,
        "commands": [
            {"command": "add_columns", "sheet": sheet, "count": 26, "at": 0},
            {"command": "add_rows", "sheet": sheet, "count": 1000, "at": 0, "table": null}
        ]}),
    )?;
    Ok(())
}

pub(crate) fn create_example(path: &Path) -> Result<(), String> {
    create_workbook(path)?;
    let document = GridDocument::open(path, None)?;
    let rows = [
        ["Weekend budget", "Quantity", "Cost each", "Total"],
        ["Train tickets", "2", "18", "=B2*C2"],
        ["Lunch", "2", "12", "=B3*C3"],
        ["Coffee", "2", "3", "=B4*C4"],
        ["Total", "", "", "=SUM(D2:D4)"],
    ];
    let values: Vec<Vec<String>> = rows
        .iter()
        .map(|row| row.iter().map(|value| (*value).to_string()).collect())
        .collect();
    document.set_matrix(0, 0, &values)
}

pub(crate) fn transfer_summary(manifest: &Value) -> String {
    let mut lines = vec![format!(
        "File written: {}",
        manifest["output"]
            .as_str()
            .unwrap_or("selected destination")
    )];
    for key in [
        "formula_cells_preserved",
        "formula_cells_flattened",
        "formula_cells_native",
        "formula_cells_cached_only",
        "formula_cells_omitted",
        "error_cells_omitted",
        "rejected_value_cells_omitted",
        "skipped_source_sheets",
        "error_cells_as_null",
        "potential_formula_injection_cells",
    ] {
        if let Some(count) = manifest[key].as_u64() {
            lines.push(format!("{}: {count}", key.replace('_', " ")));
        }
    }
    if let Some(limitations) = manifest["limitations"].as_array() {
        lines.extend(
            limitations
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    lines.join("\n\n")
}

struct DocumentState {
    client: ServiceClient,
    path: PathBuf,
    branch: Option<String>,
    current_sheet: usize,
    actor: String,
    pages: VecDeque<CachedPage>,
    loader: Option<PageLoader>,
    requests: u64,
    revision: String,
    undo: Vec<EditRecord>,
    redo: Vec<EditRecord>,
}

struct EditRecord {
    before: Vec<Value>,
    after: Vec<Value>,
    bytes: usize,
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

impl Drop for GridDocument {
    fn drop(&mut self) {
        // Checkpoint before the owning launcher stops the service. Do not close
        // the shared store: another window may still be editing this document.
        if let Ok(state) = self.state.get_mut() {
            clear_agent_session(&state.path);
            let request = json!({"kind": "snapshot", "path": state.path, "branch": state.branch});
            if let Err(error) = state.client.call(&request) {
                eprintln!(
                    "omasheets-grid: final checkpoint unavailable; durable edits will replay on reopen: {error}"
                );
            }
        }
    }
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
                loader: None,
                requests: 2,
                revision: summary["revision"]
                    .as_str()
                    .ok_or("document has no revision")?
                    .to_string(),
                undo: Vec::new(),
                redo: Vec::new(),
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
        let position = state.pages.iter().position(|page| {
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

    /// Rendering never waits for service I/O. Explicit edit/copy reads still use
    /// `cell`, preserving their validation and save ordering.
    pub fn display_cell(
        &self,
        row: usize,
        column: usize,
        notify: impl Fn() + Send + 'static,
    ) -> Result<GridCell, String> {
        let mut state = self.state.lock().map_err(|_| "grid cache is poisoned")?;
        let sheet = self
            .sheets
            .get(state.current_sheet)
            .ok_or("the current sheet is unavailable")?;
        if row >= sheet.rows || column >= sheet.columns {
            return Ok(GridCell::default());
        }
        let key = PageKey {
            sheet: sheet.id.clone(),
            row: row / PAGE_ROWS * PAGE_ROWS,
            column: column / PAGE_COLUMNS * PAGE_COLUMNS,
        };
        if let Some(position) = state.pages.iter().position(|p| {
            p.sheet == key.sheet && p.row_start == key.row && p.column_start == key.column
        }) {
            let page = state.pages.remove(position).expect("found");
            let cell = page.cells.get(&(row, column)).cloned().unwrap_or_default();
            state.pages.push_back(page);
            return Ok(cell);
        }
        if state.loader.is_none() {
            state.loader = Some(PageLoader::spawn(
                state.path.clone(),
                state.branch.clone(),
                ServiceClient::connect,
                notify,
            ));
        }
        state.loader.as_mut().expect("created").request(key)?;
        Ok(GridCell {
            display: "…".into(),
            input: String::new(),
            kind: "loading".into(),
            presentation: String::new(),
        })
    }

    /// Called on the Qt thread after a background reply; stale generations are
    /// discarded and never overwrite a newer edit or a different sheet's cache.
    pub fn accept_pages(&self) -> Result<bool, String> {
        let mut state = self.state.lock().map_err(|_| "grid cache is poisoned")?;
        let mut accepted = Vec::new();
        let mut changed = false;
        let mut requests = 0;
        let mut error = None;
        if let Some(loader) = &mut state.loader {
            while let Ok(reply) = loader.replies.try_recv() {
                requests += 1;
                // Even an obsolete reply frees a queue slot for the viewport.
                changed = true;
                if reply.generation != loader.generation.load(Ordering::Acquire) {
                    continue;
                }
                loader.pending.remove(&reply.key);
                match reply.page {
                    Ok(page) => accepted.push(page),
                    Err(message) => {
                        loader.error = Some(message.clone());
                        error = Some(message);
                    }
                }
            }
        }
        state.requests += requests;
        for page in accepted {
            state.pages.retain(|old| {
                !(old.sheet == page.sheet
                    && old.row_start == page.row_start
                    && old.column_start == page.column_start)
            });
            state.pages.push_back(page);
        }
        while state.pages.len() > MAX_CACHED_PAGES {
            state.pages.pop_front();
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(changed)
    }

    pub fn set_text(&self, row: usize, column: usize, text: &str) -> Result<(), String> {
        self.set_matrix(row, column, &[vec![text.to_string()]])
    }

    pub fn set_matrix(
        &self,
        row: usize,
        column: usize,
        values: &[Vec<String>],
    ) -> Result<(), String> {
        self.sheet_action(json!({"action":"set_cells","row":row,"column":column,"values":values}))
            .map(|_| ())
    }

    pub fn sheet_action(&self, action: Value) -> Result<Value, String> {
        let mut state = self.state.lock().map_err(|_| "grid cache is poisoned")?;
        if state
            .branch
            .as_deref()
            .is_some_and(|branch| branch != "main")
        {
            return Err("Spreadsheet controls require the main workbook".into());
        }
        let sheet = self
            .sheets
            .get(state.current_sheet)
            .ok_or("current sheet is unavailable")?;
        let request = json!({"kind":"edit_sheet","path":state.path,"sheet":sheet.id,
            "expected_revision":state.revision,"action":action});
        let response = state.client.call(&request)?;
        state.requests += 1;
        let parsed = (|| -> Result<_, String> {
            if response["kind"] != "sheet_edited" {
                return Err("unexpected spreadsheet action response".into());
            }
            let revision = response["revision"]
                .as_str()
                .ok_or("missing edit revision")?
                .to_string();
            let before = response["undo"]
                .as_array()
                .ok_or("missing undo record")?
                .clone();
            let after = response["redo"]
                .as_array()
                .ok_or("missing redo record")?
                .clone();
            let structural = response["structural"]
                .as_bool()
                .ok_or("missing structural status")?;
            Ok((revision, before, after, structural))
        })();
        let (revision, before, after, structural) = match parsed {
            Ok(record) => record,
            Err(error) => {
                state.client.usable = false;
                return Err(format!(
                    "{error}; reopen to verify whether the action was saved"
                ));
            }
        };
        state.revision = revision;
        state.pages.clear();
        if let Some(loader) = &mut state.loader {
            loader.invalidate();
        }
        if structural {
            state.undo.clear();
            state.redo.clear();
        } else if !after.is_empty() {
            state.redo.clear();
            if !before.is_empty() {
                let mut record = EditRecord {
                    before,
                    after,
                    bytes: 0,
                };
                record.bytes = record_bytes(&record);
                state.undo.push(record);
                while state.undo.len() > 32
                    || state.undo.iter().map(|record| record.bytes).sum::<usize>() > 8 * 1024 * 1024
                {
                    state.undo.remove(0);
                }
            }
        }
        Ok(response)
    }

    fn sheet_read(&self, kind: &str, extra: Value) -> Result<Value, String> {
        let mut state = self.state.lock().map_err(|_| "grid cache is poisoned")?;
        let sheet = self
            .sheets
            .get(state.current_sheet)
            .ok_or("current sheet is unavailable")?;
        let mut request = json!({"kind":kind,"path":state.path,"sheet":sheet.id});
        for (key, value) in extra.as_object().ok_or("invalid sheet query")? {
            request[key] = value.clone();
        }
        let result = state.client.call(&request);
        state.requests += 1;
        result
    }

    pub fn sheet_view(&self) -> Result<Value, String> {
        self.sheet_read("sheet_view", json!({}))
    }
    pub fn inspect_range(&self, range: Value) -> Result<Value, String> {
        self.sheet_read("inspect_range", json!({"range":range}))
    }
    pub fn find_sheet(&self, query: &str) -> Result<Value, String> {
        self.sheet_read("find_in_sheet", json!({"query":query}))
    }

    pub fn fill_range(
        &self,
        row: usize,
        column: usize,
        rows: usize,
        columns: usize,
        right: bool,
    ) -> Result<(), String> {
        let sheet = self.current_sheet()?;
        if rows == 0
            || columns == 0
            || rows.checked_mul(columns).is_none_or(|count| count > 1000)
            || row.checked_add(rows).is_none_or(|end| end > sheet.rows)
            || column
                .checked_add(columns)
                .is_none_or(|end| end > sheet.columns)
        {
            return Err("Fill a rectangle of at most 1,000 cells inside the sheet".into());
        }
        let mut values = Vec::new();
        for dr in 0..rows {
            let mut line = Vec::new();
            for dc in 0..columns {
                let source = self.cell(
                    row + if right { dr } else { 0 },
                    column + if right { 0 } else { dc },
                )?;
                if source.kind == "bound_formula" {
                    return Err("This formula's stable references cannot be filled as an A1 rectangle. Inspect its lineage first.".into());
                }
                let input = if source.kind == "formula" && !source.input.starts_with('=') {
                    format!("={}", source.input)
                } else {
                    source.input
                };
                let mut value = vec![vec![input]];
                crate::clipboard::translate(
                    &mut value,
                    if right { 0 } else { dr as i32 },
                    if right { dc as i32 } else { 0 },
                );
                line.push(value.remove(0).remove(0));
            }
            values.push(line);
        }
        self.set_matrix(row, column, &values)
    }

    pub fn undo(&self, redo: bool) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "grid cache is poisoned")?;
        let record = if redo {
            state.redo.pop()
        } else {
            state.undo.pop()
        }
        .ok_or(if redo {
            "nothing to redo"
        } else {
            "nothing to undo"
        })?;
        if let Err(error) = apply_commands(
            &mut state,
            if redo { &record.after } else { &record.before },
        ) {
            if redo {
                state.redo.push(record);
            } else {
                state.undo.push(record);
            }
            return Err(error);
        }
        if redo {
            state.undo.push(record);
        } else {
            state.redo.push(record);
        }
        Ok(())
    }

    pub fn verify_revision(&self) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "grid cache is poisoned")?;
        let request = json!({"kind": "revision", "path": state.path, "branch": state.branch});
        let summary = state.client.call(&request)?;
        state.requests += 1;
        if summary["revision"].as_str() != Some(state.revision.as_str()) {
            return Err("document changed; reopen before copying or editing".into());
        }
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

    pub(crate) fn export_request(&self, output: &Path, format: &str) -> Result<Value, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "document state is unavailable")?;
        let sheet = self
            .sheets
            .get(state.current_sheet)
            .ok_or("current sheet is unavailable")?;
        let mut request = json!({"kind": format!("export_{format}"),
            "path": state.path, "branch": state.branch, "output": output});
        if format != "xlsx" {
            request["sheet"] = sheet.id.clone().into();
        }
        Ok(request)
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
            if let Some(loader) = &mut state.loader {
                loader.invalidate();
            }
        }
        Ok(sheet)
    }

    pub fn request_count(&self) -> u64 {
        self.state.lock().map_or(0, |state| state.requests)
    }
}

fn record_bytes(record: &EditRecord) -> usize {
    serde_json::to_vec(&record.before).map_or(usize::MAX / 2, |bytes| bytes.len())
        + serde_json::to_vec(&record.after).map_or(usize::MAX / 2, |bytes| bytes.len())
}

fn apply_commands(state: &mut DocumentState, commands: &[Value]) -> Result<(), String> {
    let request = json!({
        "kind": "append_batch", "path": state.path, "branch": state.branch,
        "actor": {"kind": "human", "id": state.actor},
        "commands": commands, "expected_revision": state.revision,
    });
    let response = state.client.call(&request)?;
    if response["kind"] != "appended_batch" || !response["revision"].is_string() {
        state.client.usable = false;
        return Err(
            "save response was invalid; copy the draft and reopen to verify saved state".into(),
        );
    }
    state.revision = response["revision"].as_str().unwrap().to_string();
    state.requests += 1;
    state.pages.clear();
    if let Some(loader) = &mut state.loader {
        loader.invalidate();
    }
    Ok(())
}

fn fetch_page(
    state: &mut DocumentState,
    sheet: &str,
    row_start: usize,
    column_start: usize,
) -> Result<CachedPage, String> {
    let request = page_request(&state.path, &state.branch, sheet, row_start, column_start);
    let response = state.client.call(&request)?;
    state.requests += 1;
    let page = parse_page(&response, sheet, row_start, column_start);
    if page.is_err() {
        // Do not repeat a malformed page request for every visible delegate.
        state.client.usable = false;
    }
    page
}

fn page_request(
    path: &Path,
    branch: &Option<String>,
    sheet: &str,
    row: usize,
    column: usize,
) -> Value {
    json!({"kind":"grid_page", "path":path, "branch":branch, "sheet":sheet,
        "row_start":row, "column_start":column, "rows":PAGE_ROWS, "columns":PAGE_COLUMNS})
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
    let rows = response["rows"]
        .as_u64()
        .filter(|rows| *rows <= PAGE_ROWS as u64)
        .ok_or("service returned invalid page dimensions")?;
    let columns = response["columns"]
        .as_u64()
        .filter(|columns| *columns <= PAGE_COLUMNS as u64)
        .ok_or("service returned invalid page dimensions")?;
    let entries = response["cells"]
        .as_array()
        .ok_or("service page has no cell list")?;
    if entries.len() as u64 > rows * columns {
        return Err("service page contains too many cells".into());
    }
    let mut cells = BTreeMap::new();
    for cell in entries {
        let row = cell["row"].as_u64().ok_or("service cell has no row")?;
        let column = cell["column"]
            .as_u64()
            .ok_or("service cell has no column")?;
        if row
            .checked_sub(row_start as u64)
            .is_none_or(|offset| offset >= rows)
            || column
                .checked_sub(column_start as u64)
                .is_none_or(|offset| offset >= columns)
        {
            return Err("service cell is outside the requested page".into());
        }
        if cells
            .insert((row as usize, column as usize), parse_cell(cell)?)
            .is_some()
        {
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
    let value_type = value["type"]
        .as_str()
        .ok_or("service cell has no value type")?;
    let display = match value_type {
        "number" => value["value"]
            .as_f64()
            .map(format_number)
            .ok_or("service cell has an invalid number")?,
        "text" | "error" => value["value"]
            .as_str()
            .ok_or("service cell has invalid text")?
            .to_string(),
        "boolean" => match value["value"].as_bool() {
            Some(true) => "TRUE".into(),
            Some(false) => "FALSE".into(),
            None => return Err("service cell has an invalid boolean".into()),
        },
        "blank" => String::new(),
        _ => return Err("service cell has an unknown value type".into()),
    };
    if cell
        .get("formula")
        .is_some_and(|formula| !formula.is_null() && !formula.is_string())
    {
        return Err("service cell has an invalid formula".into());
    }
    let formula = cell["formula"].as_str();
    let input_text = if let Some(formula) = formula {
        formula.to_string()
    } else if value_type == "text"
        && (display.is_empty()
            || display.starts_with(['\'', '='])
            || display.eq_ignore_ascii_case("true")
            || display.eq_ignore_ascii_case("false")
            || display
                .parse::<f64>()
                .is_ok_and(|number| number.is_finite()))
    {
        format!("'{display}")
    } else {
        display.clone()
    };
    let presentation = if cell.get("display").is_some()
        || cell["style"].is_object()
        || cell["note"].as_str().is_some_and(|note| !note.is_empty())
    {
        json!({"style":cell["style"],"note":cell["note"],"value_type":value_type,"raw_display":display}).to_string()
    } else {
        String::new()
    };
    Ok(GridCell {
        display: cell["display"].as_str().unwrap_or(&display).into(),
        input: input_text,
        kind: if cell["formula_projection_error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
        {
            "bound_formula"
        } else if formula.is_some() {
            "formula"
        } else {
            value_type
        }
        .into(),
        presentation,
    })
}

#[cfg(test)]
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

#[cfg(test)]
fn restore_command(sheet: &str, a1: &str, cell: &GridCell) -> Value {
    match cell.kind.as_str() {
        "formula" => json!({"command":"set_formula","sheet":sheet,"a1":a1,"source":cell.input}),
        "blank" => json!({"command":"set_value","sheet":sheet,"a1":a1,"value":{"type":"blank"}}),
        _ => edit_command(sheet, a1, &cell.input),
    }
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
    #[ignore = "requires the authenticated native service; exercised by Qt CI"]
    fn desktop_workflow_service_roundtrip() {
        let directory =
            std::env::temp_dir().join(format!("omasheets-desktop-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("My workbook.omasheets");
        create_workbook(&path).unwrap();
        let document = GridDocument::open(&path, None).unwrap();
        assert_eq!(document.current_sheet().unwrap().rows, 1000);
        assert_eq!(document.current_sheet().unwrap().columns, 26);
        document.set_text(0, 0, "7").unwrap();
        document.set_text(0, 1, "=A1*3").unwrap();
        assert_eq!(document.cell(0, 1).unwrap().display, "21");
        for format in ["csv", "xlsx", "parquet"] {
            let output = directory.join(format!("copy.{format}"));
            let request = document.export_request(&output, format).unwrap();
            let report = desktop_call(&request).unwrap();
            assert!(transfer_summary(&report).contains("File written:"));
            assert!(output.metadata().unwrap().len() > 0);
            let bytes = fs::read(&output).unwrap();
            assert!(desktop_call(&request).is_err());
            assert_eq!(fs::read(&output).unwrap(), bytes);
        }
        drop(document);
        desktop_call(&json!({"kind": "close", "path": path})).unwrap();
        let persisted = fs::read(&path).unwrap();
        assert!(create_workbook(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), persisted);
        let reopened = GridDocument::open(&path, None).unwrap();
        assert_eq!(reopened.cell(0, 1).unwrap().input, "=A1*3");
        assert_eq!(reopened.cell(0, 1).unwrap().display, "21");
        drop(reopened);
        desktop_call(&json!({"kind": "close", "path": path})).unwrap();
        let imported = directory.join("Imported.omasheets");
        desktop_call(
            &json!({"kind": "import_xlsx", "source": directory.join("copy.xlsx"),
            "output": imported, "actor": {"kind": "human", "id": "ci"}}),
        )
        .unwrap();
        let document = GridDocument::open(&imported, None).unwrap();
        assert_eq!(document.cell(0, 1).unwrap().display, "21");
        drop(document);
        desktop_call(&json!({"kind": "close", "path": imported})).unwrap();
        let example = directory.join("Practice.omasheets");
        create_example(&example).unwrap();
        let practice = GridDocument::open(&example, None).unwrap();
        assert_eq!(practice.cell(1, 3).unwrap().display, "36");
        assert_eq!(practice.cell(4, 3).unwrap().display, "66");
        practice.set_text(1, 1, "3").unwrap();
        assert_eq!(practice.cell(1, 3).unwrap().display, "54");
        assert_eq!(practice.cell(4, 3).unwrap().display, "84");
        assert!(create_example(&example).is_err());
        assert_eq!(practice.cell(1, 1).unwrap().display, "3");
        drop(practice);
        desktop_call(&json!({"kind": "close", "path": example})).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rendering_returns_while_io_is_blocked_and_old_generations_cannot_fill_cache() {
        let (client, edit_peer) = connected_pair();
        let (read_client, read_peer) = connected_pair();
        read_client
            .reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (seen, started) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let (notify, notified) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let mut reader = BufReader::new(read_peer.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            seen.send(()).unwrap();
            resume.recv().unwrap();
            writeln!(&read_peer, "{}", json!({"ok":true,"response":{
                "kind":"grid_page","sheet":"sheet-id","row_start":0,"column_start":0,
                "rows":64,"columns":16,"cells":[{"row":0,"column":0,"value":{"type":"number","value":99}}]
            }})).unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap()["row_start"],
                0
            );
            writeln!(&read_peer, "{}", json!({"ok":true,"response":{
                "kind":"grid_page","sheet":"sheet-id","row_start":0,"column_start":0,
                "rows":64,"columns":16,"cells":[{"row":0,"column":0,"value":{"type":"number","value":42}}]
            }})).unwrap();
        });
        let loader = PageLoader::spawn(
            PathBuf::from("test.omasheets"),
            None,
            move || Ok(read_client),
            move || {
                let _ = notify.send(());
            },
        );
        let document = GridDocument {
            name: "Test".into(),
            sheets: vec![SheetInfo {
                id: "sheet-id".into(),
                name: "Sheet".into(),
                rows: 4096,
                columns: 16,
            }],
            state: Mutex::new(DocumentState {
                client,
                path: PathBuf::from("test.omasheets"),
                branch: None,
                current_sheet: 0,
                actor: "test".into(),
                pages: VecDeque::new(),
                loader: Some(loader),
                requests: 0,
                revision: "r0".into(),
                undo: Vec::new(),
                redo: Vec::new(),
            }),
        };
        assert_eq!(document.display_cell(0, 0, || {}).unwrap().kind, "loading");
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        // The peer cannot reply until this thread releases it. All paint reads
        // must return anyway, deduplicating and bounding outstanding requests.
        for row in (0..4096).step_by(64) {
            assert_eq!(
                document.display_cell(row, 0, || {}).unwrap().kind,
                "loading"
            );
        }
        {
            let mut state = document.state.lock().unwrap();
            let loader = state.loader.as_mut().unwrap();
            assert_eq!(loader.pending.len(), MAX_CACHED_PAGES);
            loader.invalidate();
        }
        release.send(()).unwrap();
        notified.recv_timeout(Duration::from_secs(5)).unwrap();
        document.accept_pages().unwrap();
        assert!(document.state.lock().unwrap().pages.is_empty());
        loop {
            let cell = document.display_cell(0, 0, || {}).unwrap();
            if cell.kind != "loading" {
                assert_eq!(cell.display, "42");
                break;
            }
            notified.recv_timeout(Duration::from_secs(5)).unwrap();
            document.accept_pages().unwrap();
        }
        server.join().unwrap();
        drop(edit_peer);
    }

    #[test]
    fn bulk_edit_undo_redo_and_failed_undo_preserve_history() {
        let (client, peer) = connected_pair();
        client
            .reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let server = std::thread::spawn(move || {
            let mut reader = BufReader::new(peer.try_clone().unwrap());
            let mut writer = peer;
            for step in 0..7 {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let response = if step == 0 || step == 5 || step == 6 {
                    assert_eq!(request["kind"], "edit_sheet");
                    assert_eq!(request["action"]["action"], "set_cells");
                    assert_eq!(
                        request["expected_revision"],
                        if step == 0 {
                            "d0"
                        } else if step == 5 {
                            "d4"
                        } else {
                            "d5"
                        }
                    );
                    if step == 6 {
                        json!({"ok":false,"error":{"code":"document_changed","message":"stale"}})
                    } else {
                        json!({"ok":true,"response":{"kind":"sheet_edited","revision":if step==0 {"d1"} else {"d5"},
                        "structural":false,"undo":[{"command":"clear_cell","sheet":"sheet-id","a1":"A1"}],
                        "redo":[{"command":"set_value","sheet":"sheet-id","a1":"A1","value":{"type":"number","value":1}}]}})
                    }
                } else {
                    assert_eq!(request["kind"], "append_batch");
                    assert_eq!(
                        request["expected_revision"],
                        match step {
                            1 => "d1",
                            2 => "d2",
                            _ => "d3",
                        }
                    );
                    assert_eq!(
                        request["commands"][0]["command"],
                        if step == 2 { "set_value" } else { "clear_cell" }
                    );
                    if step == 3 {
                        json!({"ok":false,"error":{"code":"document_changed","message":"stale"}})
                    } else {
                        json!({"ok":true,"response":{"kind":"appended_batch","revision":match step {1=>"d2",2=>"d3",_=>"d4"}}})
                    }
                };
                writeln!(writer, "{response}").unwrap();
            }
        });
        let document = GridDocument {
            name: "Test".into(),
            sheets: vec![SheetInfo {
                id: "sheet-id".into(),
                name: "Sheet".into(),
                rows: 4,
                columns: 4,
            }],
            state: Mutex::new(DocumentState {
                client,
                path: PathBuf::from("test.omasheets"),
                branch: None,
                current_sheet: 0,
                actor: "test".into(),
                pages: VecDeque::new(),
                loader: None,
                requests: 0,
                revision: "d0".into(),
                undo: Vec::new(),
                redo: Vec::new(),
            }),
        };
        document
            .set_matrix(
                0,
                0,
                &[vec!["1".into(), "2".into()], vec!["3".into(), "4".into()]],
            )
            .unwrap();
        document.undo(false).unwrap();
        document.undo(true).unwrap();
        assert!(
            document
                .undo(false)
                .unwrap_err()
                .contains("document_changed")
        );
        assert_eq!(document.state.lock().unwrap().undo.len(), 1);
        document.undo(false).unwrap();
        document.set_text(0, 0, "5").unwrap();
        assert!(document.undo(true).unwrap_err().contains("nothing to redo"));
        assert!(
            document
                .set_text(0, 0, "5")
                .unwrap_err()
                .contains("document_changed")
        );
        assert_eq!(document.state.lock().unwrap().undo.len(), 1);
        server.join().unwrap();
    }

    #[test]
    fn closing_grid_checkpoints_without_closing_shared_document() {
        let (client, peer) = connected_pair();
        let server = std::thread::spawn(move || {
            let mut reader = BufReader::new(peer.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(
                request,
                json!({"kind":"snapshot","path":"test.omasheets","branch":"draft"})
            );
            writeln!(
                &peer,
                "{}",
                json!({"ok":true,"response":{"kind":"snapshotted"}})
            )
            .unwrap();
        });
        drop(GridDocument {
            name: "Test".into(),
            sheets: Vec::new(),
            state: Mutex::new(DocumentState {
                client,
                path: PathBuf::from("test.omasheets"),
                branch: Some("draft".into()),
                current_sheet: 0,
                actor: "test".into(),
                pages: VecDeque::new(),
                loader: None,
                requests: 0,
                revision: "r0".into(),
                undo: Vec::new(),
                redo: Vec::new(),
            }),
        });
        server.join().unwrap();
    }

    fn connected_pair() -> (ServiceClient, UnixStream) {
        let (stream, peer) = UnixStream::pair().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let writer = stream.try_clone().unwrap();
        (
            ServiceClient {
                reader: BufReader::new(stream),
                writer,
                usable: true,
            },
            peer,
        )
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
            assert!(
                client
                    .call(&json!({"edit": 2}))
                    .unwrap_err()
                    .contains("reopen")
            );
            peer.set_nonblocking(true).unwrap();
            let mut byte = [0];
            assert_eq!(
                peer.read(&mut byte).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }

    #[test]
    fn complete_rejection_keeps_connection_usable() {
        let (mut client, mut peer) = connected_pair();
        peer.write_all(
            b"{\"ok\":false,\"error\":{\"code\":\"validation\",\"message\":\"rejected\"}}\n",
        )
        .unwrap();
        assert_eq!(client.call(&json!({})).unwrap_err(), "validation: rejected");
        peer.write_all(b"{\"ok\":true,\"response\":{\"saved\":true}}\n")
            .unwrap();
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
            GridCell {
                display: "8".into(),
                input: "=A1*2".into(),
                kind: "formula".into(),
                presentation: String::new()
            }
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
            ("kind", json!("document")),
            ("sheet", json!("other-sheet")),
            ("row_start", json!(0)),
            ("column_start", json!(0)),
            ("rows", json!(65)),
            ("columns", json!(17)),
            ("cells", Value::Null),
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
        assert!(
            parse_page(&empty, "sheet-id", 64, 16)
                .unwrap()
                .cells
                .is_empty()
        );
    }

    #[test]
    fn edits_use_native_command_types() {
        assert_eq!(edit_command("sheet-id", "A1", "")["command"], "clear_cell");
        assert_eq!(
            edit_command("sheet-id", "A1", "=1+1")["command"],
            "set_formula"
        );
        assert_eq!(
            edit_command("sheet-id", "A1", "12.5")["value"]["type"],
            "number"
        );
        assert_eq!(
            edit_command("sheet-id", "A1", "TRUE")["value"]["type"],
            "boolean"
        );
        assert_eq!(
            edit_command("sheet-id", "A1", "hello")["value"]["type"],
            "text"
        );
    }

    #[test]
    fn literal_text_round_trips_without_coercion() {
        for text in [
            "00123",
            "TRUE",
            "false",
            "=SUM(A1:A2)",
            "'quoted",
            "",
            "hello",
            "<b>text</b>",
        ] {
            let cell = parse_cell(&json!({"value": {"type": "text", "value": text}})).unwrap();
            assert_eq!(cell.display, text);
            let command = edit_command("sheet-id", "A1", &cell.input);
            assert_eq!(command["command"], "set_value");
            assert_eq!(command["value"], json!({"type": "text", "value": text}));
        }
        assert_eq!(
            edit_command("sheet-id", "A1", "'00123")["value"]["value"],
            "00123"
        );
        assert_eq!(
            edit_command("sheet-id", "A1", "'=1+1")["value"]["type"],
            "text"
        );
        assert_eq!(
            edit_command("sheet-id", "A1", "'TRUE")["value"]["type"],
            "text"
        );
    }

    #[test]
    fn undo_retains_formula_and_explicit_blank_inputs() {
        let formula =
            parse_cell(&json!({"formula":"A1+1","value":{"type":"number","value":2}})).unwrap();
        let restored = restore_command("sheet", "B1", &formula);
        assert_eq!(restored["command"], "set_formula");
        assert_eq!(restored["source"], "A1+1");
        let blank = parse_cell(&json!({"value":{"type":"blank"}})).unwrap();
        assert_eq!(
            restore_command("sheet", "B1", &blank)["value"]["type"],
            "blank"
        );
        assert_eq!(
            restore_command("sheet", "B1", &GridCell::default())["command"],
            "clear_cell"
        );
    }

    #[test]
    fn page_and_cache_bounds_fit_the_service_contract() {
        assert!(PAGE_ROWS * PAGE_COLUMNS <= 10_000);
        assert_eq!(MAX_CACHED_PAGES * PAGE_ROWS * PAGE_COLUMNS, 32_768);
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
