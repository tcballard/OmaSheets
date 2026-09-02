#[cfg(feature = "formualizer")]
use formualizer_workbook::{
    CalamineAdapter, LoadStrategy, SpreadsheetReader, Workbook, WorkbookConfig,
};
use omasheets_xlsx::{ImportLimits, ScoreReport as OwnedReport, import_xlsx};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const MAX_CORPUS_FILES: usize = 1_000;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_INPUT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PROBE_STDOUT: usize = 1024 * 1024;
const MAX_PROBE_STDERR: usize = 64 * 1024;
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const MAX_TIMEOUT_SECONDS: u64 = 300;
#[cfg(unix)]
const MAX_PROBE_ADDRESS_SPACE_BYTES: libc::rlim_t = 2 * 1024 * 1024 * 1024;

fn usage() -> &'static str {
    "usage:\n  omasheets-corpus index ROOT OUTPUT.jsonl\n  omasheets-corpus verify MANIFEST.jsonl ROOT\n  omasheets-corpus score MANIFEST.jsonl ROOT OUTPUT.json [--timeout-seconds N] [--require-all]"
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestEntry {
    id: String,
    path: String,
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProbeReport {
    schema: u8,
    engine: String,
    sheet_count: usize,
    value_cells_observed: Option<u64>,
    formula_cells_observed: Option<u64>,
    value_slots_loaded: Option<u64>,
    formula_cells_loaded: Option<u64>,
    timing_ms: ProbeTiming,
    /// Peak resident set of the probe process, from `getrusage` on Unix.
    #[serde(default)]
    peak_rss_bytes: Option<u64>,
}

/// One workbook through the owned M0 engine, run in its own bounded child.
#[derive(Debug, Serialize, Deserialize)]
struct OwnedProbe {
    schema: u8,
    report: OwnedReport,
    timing_ms: f64,
    #[serde(default)]
    peak_rss_bytes: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProbeTiming {
    open: f64,
    load: f64,
    evaluate: f64,
    total: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProbeFailure {
    schema: u8,
    error: String,
}

#[derive(Debug, Serialize)]
struct ScoreEntry {
    id: String,
    sha256: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe: Option<ProbeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    owned_status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    owned: Option<OwnedProbe>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owned_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct ScoreSummary {
    files: usize,
    succeeded: usize,
    failed: usize,
    timed_out: usize,
    value_cells_observed: Option<u64>,
    formula_cells_observed: Option<u64>,
    formula_cells_loaded: Option<u64>,
    formula_parse_rate: Option<f64>,
    peak_rss_bytes_max: Option<u64>,
}

/// Aggregate of the owned-engine lane. `opened` counts workbooks the owned
/// importer accepted; parse, comparison and match rates are over formula
/// cells, and every rate is `None` until at least one workbook opened.
#[derive(Debug, Default, Serialize)]
struct OwnedSummary {
    files: usize,
    opened: usize,
    failed: usize,
    timed_out: usize,
    formula_cells_observed: u64,
    formula_cells_loaded: u64,
    formula_cells_compared: u64,
    stored_values_matched: u64,
    stored_values_mismatched: u64,
    unsupported_formulas: u64,
    formula_parse_rate: Option<f64>,
    comparison_coverage: Option<f64>,
    stored_value_match_rate: Option<f64>,
    /// Unsupported function names across the corpus with formula-cell counts
    /// and the number of workbooks naming each, bounded like the per-file map.
    unsupported_functions: BTreeMap<String, FunctionMiss>,
    unsupported_reasons: BTreeMap<String, u64>,
    /// Workbooks the importer opened only after skipping sheet entries that
    /// have no worksheet part, and how many such entries it skipped in all.
    workbooks_with_skipped_sheets: u64,
    skipped_sheets: u64,
    peak_rss_bytes_max: Option<u64>,
}

#[derive(Debug, Default, Serialize)]
struct FunctionMiss {
    formula_cells: u64,
    workbooks: u64,
}

#[derive(Debug, Serialize)]
struct ScoreReport {
    schema: u8,
    engine: &'static str,
    owned_engine: &'static str,
    stored_value_comparison: &'static str,
    summary: ScoreSummary,
    owned_summary: OwnedSummary,
    entries: Vec<ScoreEntry>,
}

#[derive(Debug, Serialize)]
struct VerifyEntry {
    id: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct VerifyReport {
    schema: u8,
    files: usize,
    verified: usize,
    failed: usize,
    entries: Vec<VerifyEntry>,
}

fn ratio(numerator: u64, denominator: u64) -> Option<f64> {
    (denominator > 0).then(|| numerator as f64 / denominator as f64)
}

struct Captured {
    bytes: Vec<u8>,
    overflowed: bool,
}

struct ChildResult {
    status: ExitStatus,
    stdout: Captured,
    stderr: Captured,
    timed_out: bool,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn relative_xlsx(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if path.extension().and_then(|item| item.to_str()) != Some("xlsx") {
        return Err("corpus entries must name lowercase .xlsx files".into());
    }
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("corpus entry paths must stay below the declared root".into());
    }
    Ok(path.to_path_buf())
}

fn sha256(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("corpus input must be a regular file".into());
    }
    if metadata.len() > MAX_INPUT_BYTES {
        return Err("corpus input exceeds the 512 MiB limit".into());
    }
    let mut input = File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn open_new(path: &Path) -> Result<File, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
        .open(path)
        .map_err(|error| format!("refusing to replace output: {error}"))
}

fn manifest_identifier(
    digest: &str,
    identifiers: &mut HashSet<String>,
    digests: &mut HashSet<String>,
) -> Result<String, String> {
    if !digests.insert(digest.to_string()) {
        return Err("duplicate workbook bytes require an explicit curated manifest".into());
    }
    for prefix in (16..=60).step_by(4) {
        let candidate = format!("wb-{}", &digest[..prefix]);
        if identifiers.insert(candidate.clone()) {
            return Ok(candidate);
        }
    }
    Err("SHA-256 prefix collision requires an explicit curated manifest".into())
}

fn walk_xlsx(root: &Path, current: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            walk_xlsx(root, &path, output)?;
        } else if kind.is_file() && path.extension().and_then(|item| item.to_str()) == Some("xlsx")
        {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "corpus path escaped its root".to_string())?
                .to_path_buf();
            output.push(relative);
            if output.len() > MAX_CORPUS_FILES {
                return Err(format!("corpus exceeds the {MAX_CORPUS_FILES}-file limit"));
            }
        }
    }
    Ok(())
}

fn index_command(root: &Path, output: &Path) -> Result<(), String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    if !root.is_dir() {
        return Err("corpus root must be a directory".into());
    }
    let mut paths = Vec::new();
    walk_xlsx(&root, &root, &mut paths)?;
    paths.sort();
    if paths.is_empty() {
        return Err("corpus root contains no .xlsx files".into());
    }

    let mut identifiers = HashSet::new();
    let mut digests = HashSet::new();
    let mut manifest = Vec::with_capacity(paths.len());
    for relative in paths {
        let digest = sha256(&root.join(&relative))?;
        let identifier = manifest_identifier(&digest, &mut identifiers, &mut digests)?;
        let path = relative
            .to_str()
            .ok_or_else(|| "corpus paths must be UTF-8".to_string())?
            .replace('\\', "/");
        manifest.push(ManifestEntry {
            id: identifier,
            path,
            sha256: digest,
        });
    }

    let mut payload = Vec::new();
    for entry in manifest {
        serde_json::to_writer(&mut payload, &entry).map_err(|error| error.to_string())?;
        payload.push(b'\n');
    }
    if payload.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("generated manifest exceeds the 1 MiB limit".into());
    }
    let mut destination = open_new(output)?;
    destination
        .write_all(&payload)
        .map_err(|error| error.to_string())?;
    destination.sync_all().map_err(|error| error.to_string())?;
    Ok(())
}

fn read_manifest(path: &Path) -> Result<Vec<ManifestEntry>, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
        return Err("manifest must be a regular file no larger than 1 MiB".into());
    }
    let reader = BufReader::new(File::open(path).map_err(|error| error.to_string())?);
    let mut entries = Vec::new();
    let mut identifiers = HashSet::new();
    let mut paths = HashSet::new();
    for (line_number, line) in reader.lines().enumerate() {
        let line = line.map_err(|error| error.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        if entries.len() >= MAX_CORPUS_FILES {
            return Err(format!(
                "manifest exceeds the {MAX_CORPUS_FILES}-file limit"
            ));
        }
        let entry: ManifestEntry = serde_json::from_str(&line)
            .map_err(|error| format!("invalid manifest line {}: {error}", line_number + 1))?;
        if !valid_id(&entry.id) || !valid_sha256(&entry.sha256) {
            return Err(format!("invalid manifest line {}", line_number + 1));
        }
        relative_xlsx(&entry.path)?;
        if !identifiers.insert(entry.id.clone()) || !paths.insert(entry.path.clone()) {
            return Err("manifest IDs and paths must be unique".into());
        }
        entries.push(entry);
    }
    if entries.is_empty() {
        return Err("manifest contains no workbooks".into());
    }
    Ok(entries)
}

#[cfg(feature = "formualizer")]
fn probe(path: &Path) -> Result<ProbeReport, String> {
    let started = Instant::now();
    let adapter = CalamineAdapter::open_path(path).map_err(|error| error.to_string())?;
    let opened_ms = started.elapsed().as_secs_f64() * 1000.0;
    let loaded_at = Instant::now();
    let (mut workbook, adapter_stats) = Workbook::from_reader_with_adapter_stats(
        adapter,
        LoadStrategy::EagerAll,
        WorkbookConfig::ephemeral().with_span_evaluation(true),
    )
    .map_err(|error| error.to_string())?;
    let loaded_ms = loaded_at.elapsed().as_secs_f64() * 1000.0;
    let sheet_count = workbook.sheet_names().len();
    let evaluated_at = Instant::now();
    workbook.evaluate_all().map_err(|error| error.to_string())?;
    let evaluated_ms = evaluated_at.elapsed().as_secs_f64() * 1000.0;
    let stats = adapter_stats.unwrap_or_default();
    Ok(ProbeReport {
        schema: 1,
        engine: "formualizer-calamine-0.8.4".into(),
        sheet_count,
        value_cells_observed: stats.value_cells_observed,
        formula_cells_observed: stats.formula_cells_observed,
        value_slots_loaded: stats.value_slots_handed_to_engine,
        formula_cells_loaded: stats.formula_cells_handed_to_engine,
        timing_ms: ProbeTiming {
            open: opened_ms,
            load: loaded_ms,
            evaluate: evaluated_ms,
            total: started.elapsed().as_secs_f64() * 1000.0,
        },
        peak_rss_bytes: peak_rss_bytes(),
    })
}

fn owned_probe(path: &Path) -> Result<OwnedProbe, String> {
    let started = Instant::now();
    let imported = import_xlsx(path, ImportLimits::default()).map_err(|error| error.to_string())?;
    Ok(OwnedProbe {
        schema: 1,
        report: imported.report(),
        timing_ms: started.elapsed().as_secs_f64() * 1000.0,
        peak_rss_bytes: peak_rss_bytes(),
    })
}

/// Peak resident set size of this process. Linux reports `ru_maxrss` in
/// kibibytes and macOS in bytes.
#[cfg(unix)]
fn peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    let maxrss = u64::try_from(usage.ru_maxrss).ok()?;
    let scale = if cfg!(target_os = "macos") { 1 } else { 1024 };
    Some(maxrss.saturating_mul(scale))
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> Option<u64> {
    None
}

#[cfg(not(feature = "formualizer"))]
fn probe(_path: &Path) -> Result<ProbeReport, String> {
    Err("the Formualizer probe is disabled in this build".into())
}

fn sanitize_error(error: impl ToString, path: &Path) -> String {
    let raw = error.to_string();
    let redacted = raw.replace(path.to_string_lossy().as_ref(), "<input>");
    redacted.chars().take(512).collect()
}

#[cfg(unix)]
fn apply_probe_resource_limits() -> Result<(), String> {
    let limit = libc::rlimit {
        rlim_cur: MAX_PROBE_ADDRESS_SPACE_BYTES,
        rlim_max: MAX_PROBE_ADDRESS_SPACE_BYTES,
    };
    let result = unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) };
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "failed to apply the probe address-space limit: {}",
            io::Error::last_os_error()
        ))
    }
}

#[cfg(not(unix))]
fn apply_probe_resource_limits() -> Result<(), String> {
    Ok(())
}

fn emit_probe<T: Serialize>(result: Result<T, String>, path: &Path) -> bool {
    let (payload, succeeded) = match result {
        Ok(report) => (serde_json::to_string(&report), true),
        Err(error) => (
            serde_json::to_string(&ProbeFailure {
                schema: 1,
                error: sanitize_error(error, path),
            }),
            false,
        ),
    };
    match payload {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!("omasheets-corpus: {error}");
            return false;
        }
    }
    succeeded
}

fn probe_command(path: &Path) -> bool {
    emit_probe(
        apply_probe_resource_limits().and_then(|()| probe(path)),
        path,
    )
}

fn owned_probe_command(path: &Path) -> bool {
    emit_probe(
        apply_probe_resource_limits().and_then(|()| owned_probe(path)),
        path,
    )
}

fn read_capped(mut input: impl Read, maximum: usize) -> Captured {
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut overflowed = false;
    loop {
        match input.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let available = maximum.saturating_sub(retained.len());
                retained.extend_from_slice(&buffer[..count.min(available)]);
                overflowed |= count > available;
            }
        }
    }
    Captured {
        bytes: retained,
        overflowed,
    }
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> io::Result<(ExitStatus, bool)> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status, false));
        }
        if Instant::now() >= deadline {
            child.kill()?;
            return child.wait().map(|status| (status, true));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_probe(command: &str, path: &Path, timeout: Duration) -> Result<ChildResult, String> {
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let mut child = Command::new(executable)
        .arg(command)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "probe stdout unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "probe stderr unavailable".to_string())?;
    let stdout_reader = thread::spawn(move || read_capped(stdout, MAX_PROBE_STDOUT));
    let stderr_reader = thread::spawn(move || read_capped(stderr, MAX_PROBE_STDERR));
    let (status, timed_out) =
        wait_with_timeout(&mut child, timeout).map_err(|error| error.to_string())?;
    Ok(ChildResult {
        status,
        stdout: stdout_reader
            .join()
            .map_err(|_| "probe stdout reader failed".to_string())?,
        stderr: stderr_reader
            .join()
            .map_err(|_| "probe stderr reader failed".to_string())?,
        timed_out,
    })
}

fn resolve_entry(root: &Path, entry: &ManifestEntry) -> Result<PathBuf, String> {
    let relative = relative_xlsx(&entry.path)?;
    let mut unresolved = root.to_path_buf();
    for component in relative.components() {
        unresolved.push(component.as_os_str());
        if fs::symlink_metadata(&unresolved)
            .map_err(|_| "corpus input is unavailable".to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("corpus inputs cannot traverse symbolic links".into());
        }
    }
    let candidate = unresolved
        .canonicalize()
        .map_err(|_| "corpus input is unavailable".to_string())?;
    if !candidate.starts_with(root) {
        return Err("corpus input escaped its declared root".into());
    }
    let observed =
        sha256(&candidate).map_err(|_| "corpus input could not be hashed".to_string())?;
    if !observed.eq_ignore_ascii_case(&entry.sha256) {
        return Err("corpus input does not match its manifest SHA-256".into());
    }
    Ok(candidate)
}

fn probe_error(result: &ChildResult) -> String {
    if result.timed_out {
        return "probe timed out and was terminated".into();
    }
    if result.stdout.overflowed || result.stderr.overflowed {
        return "probe output exceeded its bound".into();
    }
    if let Ok(failure) = serde_json::from_slice::<ProbeFailure>(&result.stdout.bytes) {
        return failure.error;
    }
    let stderr = String::from_utf8_lossy(&result.stderr.bytes);
    if !stderr.trim().is_empty() {
        return stderr.trim().chars().take(512).collect();
    }
    format!("probe exited with {}", result.status)
}

fn score_command(
    manifest: &Path,
    root: &Path,
    output: &Path,
    timeout: Duration,
    require_all: bool,
) -> Result<bool, String> {
    let entries = read_manifest(manifest)?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    if !root.is_dir() {
        return Err("corpus root must be a directory".into());
    }
    let mut reports = Vec::with_capacity(entries.len());
    let mut succeeded = 0;
    let mut timed_out = 0;
    let mut value_cells = Some(0_u64);
    let mut formula_cells = Some(0_u64);
    let mut formulas_loaded = Some(0_u64);
    let mut peak_rss = None;
    let mut owned_summary = OwnedSummary::default();

    for entry in entries {
        let sha256 = entry.sha256.to_ascii_lowercase();
        let candidate = resolve_entry(&root, &entry);
        let run_lane = |command: &str| -> Result<ChildResult, String> {
            let path = candidate.clone()?;
            let result = run_probe(command, &path, timeout)?;
            resolve_entry(&root, &entry)
                .map_err(|_| "corpus input changed while it was being scored".to_string())?;
            Ok(result)
        };

        let (status, probe_report, error) = match run_lane("probe") {
            Ok(child) if child_completed(&child) => {
                match serde_json::from_slice::<ProbeReport>(&child.stdout.bytes) {
                    Ok(probe) => {
                        succeeded += 1;
                        value_cells = value_cells
                            .zip(probe.value_cells_observed)
                            .map(|(total, observed)| total + observed);
                        formula_cells = formula_cells
                            .zip(probe.formula_cells_observed)
                            .map(|(total, observed)| total + observed);
                        formulas_loaded = formulas_loaded
                            .zip(probe.formula_cells_loaded)
                            .map(|(total, observed)| total + observed);
                        peak_rss = peak_rss.max(probe.peak_rss_bytes);
                        ("ok", Some(probe), None)
                    }
                    Err(_) => (
                        "failed",
                        None,
                        Some("probe returned invalid bounded JSON".to_string()),
                    ),
                }
            }
            Ok(child) => {
                timed_out += usize::from(child.timed_out);
                (
                    if child.timed_out {
                        "timed_out"
                    } else {
                        "failed"
                    },
                    None,
                    Some(probe_error(&child)),
                )
            }
            Err(error) => ("failed", None, Some(error.chars().take(512).collect())),
        };

        let (owned_status, owned, owned_error) = match run_lane("probe-owned") {
            Ok(child) if child_completed(&child) => {
                match serde_json::from_slice::<OwnedProbe>(&child.stdout.bytes) {
                    Ok(owned) => {
                        accumulate_owned(&mut owned_summary, &owned);
                        ("ok", Some(owned), None)
                    }
                    Err(_) => {
                        owned_summary.failed += 1;
                        (
                            "failed",
                            None,
                            Some("owned probe returned invalid bounded JSON".to_string()),
                        )
                    }
                }
            }
            Ok(child) => {
                owned_summary.failed += 1;
                owned_summary.timed_out += usize::from(child.timed_out);
                (
                    if child.timed_out {
                        "timed_out"
                    } else {
                        "failed"
                    },
                    None,
                    Some(probe_error(&child)),
                )
            }
            Err(error) => {
                owned_summary.failed += 1;
                ("failed", None, Some(error.chars().take(512).collect()))
            }
        };

        reports.push(ScoreEntry {
            id: entry.id,
            sha256,
            status,
            probe: probe_report,
            error,
            owned_status,
            owned,
            owned_error,
        });
    }

    let files = reports.len();
    let failed = files - succeeded;
    if succeeded == 0 {
        value_cells = None;
        formula_cells = None;
        formulas_loaded = None;
    }
    let formula_parse_rate = formula_cells
        .zip(formulas_loaded)
        .and_then(|(observed, loaded)| (observed > 0).then_some(loaded as f64 / observed as f64));
    owned_summary.files = files;
    finish_owned(&mut owned_summary);
    let owned_failed = owned_summary.failed;
    let report = ScoreReport {
        schema: 2,
        engine: "formualizer-calamine-0.8.4",
        owned_engine: omasheets_xlsx::ENGINE_NAME,
        stored_value_comparison: "owned-m0",
        summary: ScoreSummary {
            files,
            succeeded,
            failed,
            timed_out,
            value_cells_observed: value_cells,
            formula_cells_observed: formula_cells,
            formula_cells_loaded: formulas_loaded,
            formula_parse_rate,
            peak_rss_bytes_max: peak_rss,
        },
        owned_summary,
        entries: reports,
    };
    let mut destination = open_new(output)?;
    serde_json::to_writer_pretty(&mut destination, &report).map_err(|error| error.to_string())?;
    destination
        .write_all(b"\n")
        .map_err(|error| error.to_string())?;
    destination.sync_all().map_err(|error| error.to_string())?;
    Ok(!require_all || (failed == 0 && owned_failed == 0))
}

fn child_completed(child: &ChildResult) -> bool {
    child.status.success() && !child.stdout.overflowed && !child.stderr.overflowed
}

fn accumulate_owned(summary: &mut OwnedSummary, owned: &OwnedProbe) {
    let report = &owned.report;
    summary.opened += 1;
    summary.formula_cells_observed += report.formula_cells_observed as u64;
    summary.formula_cells_loaded += report.formula_cells_loaded as u64;
    summary.formula_cells_compared += report.formula_cells_compared as u64;
    summary.stored_values_matched += report.stored_values_matched as u64;
    summary.stored_values_mismatched += report.stored_values_mismatched as u64;
    summary.unsupported_formulas += report.unsupported_formulas as u64;
    for (name, cells) in &report.unsupported_functions {
        if summary.unsupported_functions.len() >= omasheets_xlsx::MAX_REPORTED_FUNCTIONS
            && !summary.unsupported_functions.contains_key(name)
        {
            continue;
        }
        let miss = summary
            .unsupported_functions
            .entry(name.clone())
            .or_default();
        miss.formula_cells += *cells as u64;
        miss.workbooks += 1;
    }
    for (reason, cells) in &report.unsupported_reasons {
        *summary
            .unsupported_reasons
            .entry(reason.clone())
            .or_default() += *cells as u64;
    }
    if !report.skipped_sheets.is_empty() {
        summary.workbooks_with_skipped_sheets += 1;
        summary.skipped_sheets += report.skipped_sheets.len() as u64;
    }
    summary.peak_rss_bytes_max = summary.peak_rss_bytes_max.max(owned.peak_rss_bytes);
}

fn finish_owned(summary: &mut OwnedSummary) {
    if summary.opened == 0 {
        return;
    }
    summary.formula_parse_rate =
        ratio(summary.formula_cells_loaded, summary.formula_cells_observed);
    summary.comparison_coverage = ratio(
        summary.formula_cells_compared,
        summary.formula_cells_observed,
    );
    summary.stored_value_match_rate = ratio(
        summary.stored_values_matched,
        summary.formula_cells_compared,
    );
}

fn verify_command(manifest: &Path, root: &Path) -> Result<bool, String> {
    let entries = read_manifest(manifest)?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    if !root.is_dir() {
        return Err("corpus root must be a directory".into());
    }
    let mut report = VerifyReport {
        schema: 1,
        files: entries.len(),
        verified: 0,
        failed: 0,
        entries: Vec::with_capacity(entries.len()),
    };
    for entry in entries {
        let (status, error) = match resolve_entry(&root, &entry) {
            Ok(_) => {
                report.verified += 1;
                ("ok", None)
            }
            Err(error) => {
                report.failed += 1;
                ("failed", Some(error))
            }
        };
        report.entries.push(VerifyEntry {
            id: entry.id,
            status,
            error,
        });
    }
    let payload = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    println!("{payload}");
    Ok(report.failed == 0)
}

fn parse_score_options(arguments: &[String]) -> Result<(u64, bool), String> {
    let mut timeout = DEFAULT_TIMEOUT_SECONDS;
    let mut require_all = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--require-all" => {
                require_all = true;
                index += 1;
            }
            "--timeout-seconds" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or_else(|| usage().to_string())?;
                timeout = value
                    .parse::<u64>()
                    .map_err(|_| "invalid timeout".to_string())?;
                if !(1..=MAX_TIMEOUT_SECONDS).contains(&timeout) {
                    return Err(format!(
                        "timeout must be between 1 and {MAX_TIMEOUT_SECONDS} seconds"
                    ));
                }
                index += 2;
            }
            _ => return Err(usage().into()),
        }
    }
    Ok((timeout, require_all))
}

fn run(arguments: Vec<String>) -> Result<bool, String> {
    match arguments.as_slice() {
        [command, root, output] if command == "index" => {
            index_command(Path::new(root), Path::new(output))?;
            Ok(true)
        }
        [command, path] if command == "probe" => Ok(probe_command(Path::new(path))),
        [command, path] if command == "probe-owned" => Ok(owned_probe_command(Path::new(path))),
        [command, manifest, root] if command == "verify" => {
            verify_command(Path::new(manifest), Path::new(root))
        }
        [command, manifest, root, output, options @ ..] if command == "score" => {
            let (seconds, require_all) = parse_score_options(options)?;
            score_command(
                Path::new(manifest),
                Path::new(root),
                Path::new(output),
                Duration::from_secs(seconds),
                require_all,
            )
        }
        _ => Err(usage().into()),
    }
}

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(error) => {
            eprintln!("omasheets-corpus: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!(
            "omasheets-corpus-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        directory
    }

    #[test]
    fn manifest_ids_and_hashes_are_strict() {
        assert!(valid_id("enron-0001_A"));
        assert!(!valid_id("contains a space"));
        assert!(valid_sha256(&"a".repeat(64)));
        assert!(!valid_sha256(&"g".repeat(64)));
    }

    #[test]
    fn generated_ids_remain_valid_when_digest_prefixes_collide() {
        let mut identifiers = HashSet::new();
        let mut digests = HashSet::new();
        let first_digest = "a".repeat(64);
        let second_digest = format!("{}{}", "a".repeat(16), "b".repeat(48));

        let first = manifest_identifier(&first_digest, &mut identifiers, &mut digests).unwrap();
        let second = manifest_identifier(&second_digest, &mut identifiers, &mut digests).unwrap();

        assert!(valid_id(&first));
        assert!(valid_id(&second));
        assert_ne!(first, second);
        assert_eq!(first.len(), 19);
        assert_eq!(second.len(), 23);
    }

    #[test]
    fn corpus_paths_cannot_escape_or_change_format() {
        assert!(relative_xlsx("nested/book.xlsx").is_ok());
        assert!(relative_xlsx("../book.xlsx").is_err());
        assert!(relative_xlsx("book.XLSX").is_err());
        assert!(relative_xlsx("book.ods").is_err());
    }

    #[test]
    fn score_options_are_bounded_and_order_independent() {
        let options = vec![
            "--require-all".to_string(),
            "--timeout-seconds".to_string(),
            "12".to_string(),
        ];
        assert_eq!(parse_score_options(&options).unwrap(), (12, true));
        assert!(parse_score_options(&["--timeout-seconds".into(), "0".into()]).is_err());
        assert!(parse_score_options(&["--unknown".into()]).is_err());
    }

    #[test]
    fn errors_are_bounded_and_redact_the_input_path() {
        let path = Path::new("/private/corpus/book.xlsx");
        let error = format!("failed to open {}: {}", path.display(), "x".repeat(700));
        let sanitized = sanitize_error(error, path);
        assert!(!sanitized.contains("/private/corpus"));
        assert!(sanitized.contains("<input>"));
        assert_eq!(sanitized.chars().count(), 512);
    }

    #[test]
    fn verify_reports_digest_drift_per_entry_without_stopping() {
        let root = test_directory("verify");
        fs::write(root.join("good.xlsx"), b"frozen workbook").unwrap();
        fs::write(root.join("drift.xlsx"), b"changed workbook").unwrap();
        let good = sha256(&root.join("good.xlsx")).unwrap();
        let manifest = root.join("manifest.jsonl");
        fs::write(
            &manifest,
            format!(
                "{{\"id\":\"good\",\"path\":\"good.xlsx\",\"sha256\":\"{good}\"}}\n\
                 {{\"id\":\"drift\",\"path\":\"drift.xlsx\",\"sha256\":\"{}\"}}\n\
                 {{\"id\":\"missing\",\"path\":\"missing.xlsx\",\"sha256\":\"{good}\"}}\n",
                "0".repeat(64)
            ),
        )
        .unwrap();

        assert!(!verify_command(&manifest, &root).unwrap());
        assert_eq!(
            resolve_entry(
                &root.canonicalize().unwrap(),
                &ManifestEntry {
                    id: "drift".into(),
                    path: "drift.xlsx".into(),
                    sha256: "0".repeat(64),
                },
            )
            .unwrap_err(),
            "corpus input does not match its manifest SHA-256"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn owned_lane_aggregates_misses_by_cells_and_workbooks() {
        let mut summary = OwnedSummary::default();
        let make = |functions: &[(&str, usize)], loaded: usize| OwnedProbe {
            schema: 1,
            report: OwnedReport {
                schema: 2,
                engine: omasheets_xlsx::ENGINE_NAME.into(),
                date_system: "1900".into(),
                source_sha256: "a".repeat(64),
                sheets: 1,
                formula_cells_observed: 10,
                formula_cells_loaded: loaded,
                formula_cells_compared: loaded,
                stored_values_matched: loaded.saturating_sub(1),
                stored_values_mismatched: usize::from(loaded > 0),
                unsupported_formulas: 10 - loaded,
                unsupported_functions: functions
                    .iter()
                    .map(|(name, cells)| (name.to_string(), *cells))
                    .collect(),
                unsupported_reasons: BTreeMap::from([(
                    "unsupported_function".to_string(),
                    10 - loaded,
                )]),
                skipped_sheets: if loaded == 8 {
                    vec!["Module1".to_string(), "Module2".to_string()]
                } else {
                    Vec::new()
                },
            },
            timing_ms: 1.0,
            peak_rss_bytes: Some(4096),
        };
        accumulate_owned(&mut summary, &make(&[("TODAY", 3), ("OFFSET", 1)], 6));
        accumulate_owned(&mut summary, &make(&[("TODAY", 2)], 8));
        summary.files = 3;
        summary.failed = 1;
        finish_owned(&mut summary);

        assert_eq!(summary.opened, 2);
        assert_eq!(summary.formula_cells_observed, 20);
        assert_eq!(summary.formula_cells_loaded, 14);
        assert_eq!(summary.unsupported_formulas, 6);
        assert_eq!(summary.unsupported_functions["TODAY"].formula_cells, 5);
        assert_eq!(summary.unsupported_functions["TODAY"].workbooks, 2);
        assert_eq!(summary.unsupported_functions["OFFSET"].workbooks, 1);
        assert_eq!(summary.unsupported_reasons["unsupported_function"], 6);
        assert_eq!(summary.workbooks_with_skipped_sheets, 1);
        assert_eq!(summary.skipped_sheets, 2);
        assert_eq!(summary.formula_parse_rate, Some(0.7));
        assert_eq!(summary.comparison_coverage, Some(0.7));
        assert_eq!(summary.stored_value_match_rate, Some(12.0 / 14.0));
        assert_eq!(summary.peak_rss_bytes_max, Some(4096));
    }

    #[test]
    fn peak_rss_is_reported_on_unix() {
        let peak = peak_rss_bytes();
        if cfg!(unix) {
            assert!(peak.is_some_and(|bytes| bytes > 0));
        }
    }

    #[test]
    fn index_rejects_duplicate_bytes_before_creating_output() {
        let root = test_directory("duplicate");
        fs::write(root.join("a.xlsx"), b"same workbook").unwrap();
        fs::write(root.join("b.xlsx"), b"same workbook").unwrap();
        let output = root.join("manifest.jsonl");

        let error = index_command(&root, &output).unwrap_err();

        assert!(error.contains("duplicate workbook bytes"));
        assert!(!output.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_preflights_every_input_before_creating_output() {
        let root = test_directory("preflight");
        fs::write(root.join("a.xlsx"), b"small workbook").unwrap();
        File::create(root.join("b.xlsx"))
            .unwrap()
            .set_len(MAX_INPUT_BYTES + 1)
            .unwrap();
        let output = root.join("manifest.jsonl");

        let error = index_command(&root, &output).unwrap_err();

        assert!(error.contains("512 MiB"));
        assert!(!output.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
