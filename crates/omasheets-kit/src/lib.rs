//! Owned spreadsheet sessions for OmaSheets, without LibreOfficeKit.
//!
//! Native documents reuse the production document service. XLSX opens are
//! strict conversions into a new native working document: the source is never
//! modified, and unsupported content is refused before publishing the working
//! file. [`probe`] reports that admission decision without publishing a file.

mod admission;
pub mod jobs;

use omasheets_core::{Actor, ActorKind, Command};
use omasheets_service::spreadsheet::{Action, EditResult};
use omasheets_service::{
    GridPage, Request, Response, Service, ServiceError, SheetSummary, XlsxExportManifest,
    XlsxImportManifest,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_SOURCE_BYTES: u64 = omasheets_service::MAX_NATIVE_IMPORT_BYTES;
const MAX_HISTORY: usize = 32;
const MAX_HISTORY_BYTES: usize = 8 * 1024 * 1024;
static NONCE: AtomicU64 = AtomicU64::new(0);

/// An explicit capability report. The import manifest describes the staged
/// conversion, including any cached-only or omitted formulas and values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeReport {
    pub format: String,
    pub source_sha256: Option<String>,
    pub can_import: bool,
    pub reasons: Vec<String>,
    pub import_manifest: Option<XlsxImportManifest>,
}

#[derive(Debug)]
pub enum KitError {
    Admission(Box<ProbeReport>),
    Io(String),
    Service(ServiceError),
    Invalid(String),
}

impl fmt::Display for KitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission(report) => write!(
                f,
                "Workbook cannot be opened natively: {}",
                report.reasons.join("; ")
            ),
            Self::Io(message) | Self::Invalid(message) => f.write_str(message),
            Self::Service(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for KitError {}
impl From<std::io::Error> for KitError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
impl From<ServiceError> for KitError {
    fn from(error: ServiceError) -> Self {
        Self::Service(error)
    }
}

fn clock() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn human() -> Actor {
    Actor::new(ActorKind::Human, "omasheets-kit")
}
fn service() -> Service {
    Service::new(clock)
}
fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn source_bytes(path: &Path) -> Result<(PathBuf, Vec<u8>, String), KitError> {
    let path = path.canonicalize()?;
    let file = File::open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(KitError::Invalid("Choose a regular workbook file".into()));
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(KitError::Invalid(format!(
            "Workbook exceeds the {MAX_SOURCE_BYTES}-byte native import limit"
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(KitError::Invalid(
            "Workbook grew beyond the native import limit".into(),
        ));
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    Ok((path, bytes, digest))
}

struct PrivateDirectory(PathBuf);
impl PrivateDirectory {
    fn create(parent: &Path) -> Result<Self, KitError> {
        for _ in 0..32 {
            let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
            let mut random = [0_u8; 16];
            File::open("/dev/urandom")?.read_exact(&mut random)?;
            let token = random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let path = parent.join(format!(
                ".omasheets-kit-{}-{nonce}-{token}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(KitError::Io(
            "Could not allocate a private workbook conversion directory".into(),
        ))
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct StagedWorkbook {
    // The service is closed before this is returned, so only the directory
    // owns transient files and there are no live WAL or snapshot writes.
    directory: PrivateDirectory,
    native: PathBuf,
    report: ProbeReport,
}

fn stage_xlsx(source: &Path, parent: &Path) -> Result<StagedWorkbook, KitError> {
    let (_, bytes, source_sha256) = source_bytes(source)?;
    let mut report = ProbeReport {
        format: "xlsx".into(),
        source_sha256: Some(source_sha256),
        can_import: false,
        reasons: admission::inspect(&bytes),
        import_manifest: None,
    };
    let directory = PrivateDirectory::create(parent)?;
    let snapshot = directory.0.join("source.xlsx");
    let native = directory.0.join("workbook.omasheets");
    if report.reasons.is_empty() {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&snapshot)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        let mut service = service();
        let response = service.handle(Request::ImportXlsx {
            source: snapshot,
            output: native.clone(),
            actor: human(),
            name: source
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_owned),
        });
        match response {
            Ok(Response::ImportedXlsx(mut manifest)) => {
                // A probe never exposes a private temporary path as a usable file.
                manifest.output = PathBuf::new();
                admission::check_manifest(&manifest, &mut report.reasons);
                report.import_manifest = Some(manifest);
            }
            Ok(_) => {
                return Err(KitError::Invalid(
                    "Unexpected native import response".into(),
                ));
            }
            Err(error) => report
                .reasons
                .push(format!("{}: {}", error.code, error.message)),
        }
        service.close_all()?;
    }
    report.can_import = report.reasons.is_empty();
    Ok(StagedWorkbook {
        directory,
        native,
        report,
    })
}

/// Inspect a workbook without publishing a conversion or altering the source.
/// Legacy XLS, macro-enabled XLSM and ODS are explicit unsupported capabilities.
pub fn probe(source: impl AsRef<Path>) -> Result<ProbeReport, KitError> {
    let source = source.as_ref();
    match extension(source).as_str() {
        "xlsx" => Ok(stage_xlsx(source, &std::env::temp_dir())?.report.clone()),
        "omasheets" => {
            let source = source.canonicalize()?;
            if !source.is_file() {
                return Err(KitError::Invalid(
                    "Choose a regular native workbook file".into(),
                ));
            }
            let wal = source.with_file_name(format!(
                "{}-wal",
                source
                    .file_name()
                    .expect("regular file has a name")
                    .to_string_lossy()
            ));
            if wal.metadata().is_ok_and(|metadata| metadata.len() != 0) {
                return Ok(ProbeReport { format: "omasheets-native-v1".into(), source_sha256: None,
                    can_import: false, reasons: vec!["Close the active native workbook before probing it; its uncheckpointed WAL is not copied".into()], import_manifest: None });
            }
            // Opening a Store can update SQLite/snapshot bookkeeping. Validate
            // a private copy, so this advertised read-only operation never
            // changes the source database or creates source sidecars.
            let directory = PrivateDirectory::create(&std::env::temp_dir())?;
            let copy = directory.0.join("probe.omasheets");
            // Native event logs can grow beyond the XLSX import byte budget.
            // Copy and hash on disk, without allocating the database in memory.
            fs::copy(&source, &copy)?;
            if wal.metadata().is_ok_and(|metadata| metadata.len() != 0) {
                return Err(KitError::Invalid(
                    "The native workbook became active during its probe; close it and retry".into(),
                ));
            }
            let mut hasher = Sha256::new();
            let mut file = File::open(&copy)?;
            let mut buffer = [0_u8; 8192];
            loop {
                let length = file.read(&mut buffer)?;
                if length == 0 {
                    break;
                }
                hasher.update(&buffer[..length]);
            }
            let hash = format!("{:x}", hasher.finalize());
            drop(file);
            let mut session = WorkbookSession::open_native(&copy)?;
            // Also force replay and a bounded summary rather than treating the
            // extension as validation.
            session.sheets()?;
            session.close()?;
            Ok(ProbeReport {
                format: "omasheets-native-v1".into(),
                source_sha256: Some(hash),
                can_import: true,
                reasons: Vec::new(),
                import_manifest: None,
            })
        }
        other => {
            let (_, _, hash) = source_bytes(source)?;
            Ok(ProbeReport {
                format: other.into(),
                source_sha256: Some(hash),
                can_import: false,
                reasons: vec![format!(
                    "Native {} loading is not implemented; supported formats are XLSX and .omasheets",
                    if other.is_empty() {
                        "unknown-format"
                    } else {
                        other
                    }
                )],
                import_manifest: None,
            })
        }
    }
}

fn new_native_path(path: &Path) -> Result<PathBuf, KitError> {
    if extension(path) != "omasheets" {
        return Err(KitError::Invalid(
            "Choose a new .omasheets working file".into(),
        ));
    }
    let name = path
        .file_name()
        .ok_or_else(|| KitError::Invalid("Working file needs a filename".into()))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let output = parent.canonicalize()?.join(name);
    match fs::symlink_metadata(&output) {
        Ok(_) => Err(KitError::Invalid(
            "The native working file already exists; it will not be replaced".into(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(output),
        Err(error) => Err(error.into()),
    }
}

/// Strictly convert XLSX to a new durable native working file and close it.
pub fn import_xlsx(
    source: impl AsRef<Path>,
    working: impl AsRef<Path>,
) -> Result<ProbeReport, KitError> {
    let session = WorkbookSession::open_xlsx(source, working)?;
    let report = session
        .import_report
        .clone()
        .expect("XLSX sessions have an admission report");
    session.close()?;
    Ok(report)
}

#[derive(Clone)]
struct EditRecord {
    sheet: String,
    undo: Vec<Command>,
    redo: Vec<Command>,
    bytes: usize,
}

/// A native spreadsheet session. Edits persist immediately through the owned
/// service. `dirty` tracks edits since opening or the last successful XLSX-copy
/// save; writing a recovery snapshot does not clear that flag.
pub struct WorkbookSession {
    service: Service,
    working_path: PathBuf,
    import_report: Option<ProbeReport>,
    dirty: bool,
    undo: Vec<EditRecord>,
    redo: Vec<EditRecord>,
}

impl WorkbookSession {
    pub fn open_native(path: impl AsRef<Path>) -> Result<Self, KitError> {
        let path = path.as_ref().canonicalize()?;
        if !path.is_file() || extension(&path) != "omasheets" {
            return Err(KitError::Invalid(
                "Choose a regular .omasheets document".into(),
            ));
        }
        let mut service = service();
        service.handle(Request::Open { path: path.clone() })?;
        Ok(Self {
            service,
            working_path: path,
            import_report: None,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
        })
    }

    pub fn open_xlsx(
        source: impl AsRef<Path>,
        working: impl AsRef<Path>,
    ) -> Result<Self, KitError> {
        Self::open_xlsx_checked(source, working, None)
    }

    /// Supply the hash from a previous probe to reject a workbook changed
    /// between admission review and opening.
    pub fn open_xlsx_checked(
        source: impl AsRef<Path>,
        working: impl AsRef<Path>,
        expected_source_sha256: Option<&str>,
    ) -> Result<Self, KitError> {
        let source = source.as_ref();
        if extension(source) != "xlsx" {
            return Err(KitError::Invalid(
                "XLSX conversion requires a .xlsx source; open .omasheets documents directly"
                    .into(),
            ));
        }
        let working = new_native_path(working.as_ref())?;
        let staged = stage_xlsx(source, working.parent().expect("new path has a parent"))?;
        if !staged.report.can_import {
            return Err(KitError::Admission(Box::new(staged.report.clone())));
        }
        if let Some(expected) = expected_source_sha256
            && staged.report.source_sha256.as_deref() != Some(expected)
        {
            return Err(KitError::Invalid(
                "The source workbook changed after its admission probe".into(),
            ));
        }
        let (_, _, current_hash) = source_bytes(source)?;
        if staged.report.source_sha256.as_deref() != Some(current_hash.as_str()) {
            return Err(KitError::Invalid(
                "The source workbook changed during conversion; reopen it".into(),
            ));
        }
        // Both files sit on the destination filesystem. A hard link publishes
        // atomically and refuses even a dangling symlink at the destination.
        fs::hard_link(&staged.native, &working)?;
        let opened = Self::open_native(&working);
        let mut session = match opened {
            Ok(session) => session,
            Err(error) => {
                let _ = fs::remove_file(&working);
                return Err(error);
            }
        };
        let expected_digest = staged
            .report
            .import_manifest
            .as_ref()
            .map(|m| m.document_digest.as_str());
        let digest = session.document()?.digest;
        if expected_digest != Some(digest.as_str()) {
            session.close()?;
            let _ = fs::remove_file(&working);
            return Err(KitError::Invalid(
                "Published workbook failed native replay verification".into(),
            ));
        }
        let mut report = staged.report.clone();
        if let Some(manifest) = &mut report.import_manifest {
            manifest.output = working.clone();
        }
        session.import_report = Some(report);
        // Keep the directory alive through publication/replay verification.
        drop(staged.directory);
        Ok(session)
    }

    pub fn working_path(&self) -> &Path {
        &self.working_path
    }
    pub fn import_report(&self) -> Option<&ProbeReport> {
        self.import_report.as_ref()
    }
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn document(&mut self) -> Result<omasheets_service::DocumentSummary, KitError> {
        match self.service.handle(Request::Document {
            path: self.working_path.clone(),
            branch: None,
        })? {
            Response::Document(document) => Ok(document),
            _ => Err(KitError::Invalid("Unexpected document response".into())),
        }
    }
    pub fn sheets(&mut self) -> Result<Vec<SheetSummary>, KitError> {
        Ok(self.document()?.sheets)
    }
    /// Resolve a cell's inputs through the owned core, including stable
    /// references projected into the current sheet/cell view.
    pub fn lineage(&mut self, sheet: &str, a1: &str) -> Result<serde_json::Value, KitError> {
        match self.service.handle(Request::NativeLineage {
            path: self.working_path.clone(),
            sheet: sheet.into(),
            a1: a1.into(),
        })? {
            Response::NativeLineage(lineage) => Ok(lineage),
            _ => Err(KitError::Invalid("Unexpected lineage response".into())),
        }
    }
    pub fn revision(&mut self) -> Result<String, KitError> {
        match self.service.handle(Request::Revision {
            path: self.working_path.clone(),
            branch: None,
        })? {
            Response::Revision { revision } => Ok(revision),
            _ => Err(KitError::Invalid("Unexpected revision response".into())),
        }
    }
    pub fn viewport(
        &mut self,
        sheet: &str,
        row_start: usize,
        column_start: usize,
        rows: usize,
        columns: usize,
    ) -> Result<GridPage, KitError> {
        match self.service.handle(Request::GridPage {
            path: self.working_path.clone(),
            branch: None,
            sheet: sheet.into(),
            row_start,
            column_start,
            rows,
            columns,
        })? {
            Response::GridPage(page) => Ok(page),
            _ => Err(KitError::Invalid("Unexpected viewport response".into())),
        }
    }
    pub fn sheet_view(&mut self, sheet: &str) -> Result<serde_json::Value, KitError> {
        match self.service.handle(Request::SheetView {
            path: self.working_path.clone(),
            sheet: sheet.into(),
        })? {
            Response::SheetView(view) => Ok(view),
            _ => Err(KitError::Invalid(
                "Unexpected sheet presentation response".into(),
            )),
        }
    }
    pub fn edit(
        &mut self,
        sheet: &str,
        expected_revision: &str,
        action: Action,
    ) -> Result<EditResult, KitError> {
        let response = self.service.handle(Request::EditSheet {
            path: self.working_path.clone(),
            sheet: sheet.into(),
            expected_revision: expected_revision.into(),
            action,
        })?;
        let Response::SheetEdited(result) = response else {
            return Err(KitError::Invalid("Unexpected edit response".into()));
        };
        if !result.redo.is_empty() {
            self.dirty = true;
            self.redo.clear();
            if result.structural {
                self.undo.clear();
            } else if !result.undo.is_empty() {
                let bytes = serde_json::to_vec(&(&result.undo, &result.redo))
                    .map_err(|e| KitError::Invalid(e.to_string()))?
                    .len();
                self.undo.push(EditRecord {
                    sheet: result.selected_sheet.clone(),
                    undo: result.undo.clone(),
                    redo: result.redo.clone(),
                    bytes,
                });
                while self.undo.len() > MAX_HISTORY
                    || self.undo.iter().map(|r| r.bytes).sum::<usize>() > MAX_HISTORY_BYTES
                {
                    self.undo.remove(0);
                }
            }
        }
        Ok(result)
    }
    fn replay_edit(&mut self, expected_revision: &str, redo: bool) -> Result<String, KitError> {
        let record = (if redo { &self.redo } else { &self.undo })
            .last()
            .cloned()
            .ok_or_else(|| {
                KitError::Invalid(
                    if redo {
                        "Nothing to redo"
                    } else {
                        "Nothing to undo"
                    }
                    .into(),
                )
            })?;
        let commands = if redo {
            record.redo.clone()
        } else {
            record.undo.clone()
        };
        let response = self.service.handle(Request::AppendBatch {
            path: self.working_path.clone(),
            branch: None,
            actor: human(),
            commands,
            expected_digest: None,
            expected_revision: Some(expected_revision.into()),
        })?;
        let Response::AppendedBatch { revision, .. } = response else {
            return Err(KitError::Invalid("Unexpected undo response".into()));
        };
        if redo {
            self.redo.pop();
            self.undo.push(record);
        } else {
            self.undo.pop();
            self.redo.push(record);
        }
        self.dirty = true;
        Ok(revision)
    }
    pub fn undo(&mut self, expected_revision: &str) -> Result<String, KitError> {
        self.replay_edit(expected_revision, false)
    }
    pub fn redo(&mut self, expected_revision: &str) -> Result<String, KitError> {
        self.replay_edit(expected_revision, true)
    }
    pub fn undo_sheet(&self) -> Option<&str> {
        self.undo.last().map(|r| r.sheet.as_str())
    }

    pub fn snapshot(&mut self) -> Result<String, KitError> {
        match self.service.handle(Request::Snapshot {
            path: self.working_path.clone(),
            branch: None,
        })? {
            Response::Snapshot { digest } => Ok(digest),
            _ => Err(KitError::Invalid("Unexpected snapshot response".into())),
        }
    }

    /// Save a new XLSX copy. Refuses overwrite and formulas whose stable
    /// references cannot be projected faithfully to current A1 coordinates.
    pub fn save_xlsx_copy(
        &mut self,
        output: impl AsRef<Path>,
    ) -> Result<XlsxExportManifest, KitError> {
        let output = output.as_ref();
        if extension(output) != "xlsx" {
            return Err(KitError::Invalid("Choose a new .xlsx copy filename".into()));
        }
        let sheets = self.sheets()?;
        let total = sheets.iter().try_fold(0usize, |total, sheet| {
            sheet
                .rows
                .checked_mul(sheet.columns)
                .and_then(|count| total.checked_add(count))
        });
        if total.is_none_or(|count| count > omasheets_service::MAX_XLSX_EXPORT_CELLS) {
            return Err(KitError::Invalid(
                "Workbook exceeds the bounded XLSX-copy export limit".into(),
            ));
        }
        for sheet in &sheets {
            for row in (0..sheet.rows).step_by(64) {
                for column in (0..sheet.columns).step_by(128) {
                    let page = self.viewport(
                        &sheet.id.to_string(),
                        row,
                        column,
                        (sheet.rows - row).min(64),
                        (sheet.columns - column).min(128),
                    )?;
                    if page
                        .cells
                        .iter()
                        .any(|cell| !cell.formula_projection_error.is_empty())
                    {
                        return Err(KitError::Invalid("A formula's stable references cannot be preserved in XLSX; the copy was not written".into()));
                    }
                }
            }
        }
        let response = self.service.handle(Request::ExportXlsxStrict {
            path: self.working_path.clone(),
            branch: None,
            output: output.to_path_buf(),
        })?;
        let Response::ExportedXlsx(manifest) = response else {
            return Err(KitError::Invalid("Unexpected XLSX-copy response".into()));
        };
        if manifest.formula_cells_flattened != 0 {
            // This defensive path is reachable only if the service's contract
            // changes: the projection check above refuses such formulas first.
            let _ = fs::remove_file(&manifest.output);
            return Err(KitError::Invalid(
                "XLSX export flattened a formula; the incomplete copy was removed".into(),
            ));
        }
        self.dirty = false;
        Ok(manifest)
    }
    pub fn close(mut self) -> Result<(), KitError> {
        self.service.close_all().map_err(Into::into)
    }
}
impl Drop for WorkbookSession {
    fn drop(&mut self) {
        let _ = self.service.close_all();
    }
}
