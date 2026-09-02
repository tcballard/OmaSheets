#[cfg(feature = "formualizer")]
use formualizer_workbook::{
    CalamineAdapter, LoadStrategy, SpreadsheetReader, Workbook, WorkbookConfig,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
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

fn usage() -> &'static str {
    "usage:\n  omasheets-corpus index ROOT OUTPUT.jsonl\n  omasheets-corpus score MANIFEST.jsonl ROOT OUTPUT.json [--timeout-seconds N] [--require-all]"
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
}

#[derive(Debug, Serialize)]
struct ScoreReport {
    schema: u8,
    engine: &'static str,
    stored_value_comparison: &'static str,
    summary: ScoreSummary,
    entries: Vec<ScoreEntry>,
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

    let mut destination = open_new(output)?;
    let mut identifiers = HashSet::new();
    for relative in paths {
        let digest = sha256(&root.join(&relative))?;
        let mut prefix = 16;
        let identifier = loop {
            let candidate = format!("wb-{}", &digest[..prefix]);
            if identifiers.insert(candidate.clone()) {
                break candidate;
            }
            prefix += 4;
            if prefix > digest.len() {
                return Err("duplicate workbook bytes require an explicit curated manifest".into());
            }
        };
        let path = relative
            .to_str()
            .ok_or_else(|| "corpus paths must be UTF-8".to_string())?
            .replace('\\', "/");
        let entry = ManifestEntry {
            id: identifier,
            path,
            sha256: digest,
        };
        serde_json::to_writer(&mut destination, &entry).map_err(|error| error.to_string())?;
        destination
            .write_all(b"\n")
            .map_err(|error| error.to_string())?;
    }
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
    })
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

fn probe_command(path: &Path) -> bool {
    let (payload, succeeded) = match probe(path) {
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

fn run_probe(path: &Path, timeout: Duration) -> Result<ChildResult, String> {
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let mut child = Command::new(executable)
        .arg("probe")
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

    for entry in entries {
        let result = resolve_entry(&root, &entry).and_then(|path| {
            let result = run_probe(&path, timeout)?;
            resolve_entry(&root, &entry)
                .map_err(|_| "corpus input changed while it was being scored".to_string())?;
            Ok(result)
        });
        match result {
            Ok(child)
                if child.status.success()
                    && !child.stdout.overflowed
                    && !child.stderr.overflowed =>
            {
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
                        reports.push(ScoreEntry {
                            id: entry.id,
                            sha256: entry.sha256.to_ascii_lowercase(),
                            status: "ok",
                            probe: Some(probe),
                            error: None,
                        });
                    }
                    Err(_) => reports.push(ScoreEntry {
                        id: entry.id,
                        sha256: entry.sha256.to_ascii_lowercase(),
                        status: "failed",
                        probe: None,
                        error: Some("probe returned invalid bounded JSON".into()),
                    }),
                }
            }
            Ok(child) => {
                timed_out += usize::from(child.timed_out);
                reports.push(ScoreEntry {
                    id: entry.id,
                    sha256: entry.sha256.to_ascii_lowercase(),
                    status: if child.timed_out {
                        "timed_out"
                    } else {
                        "failed"
                    },
                    probe: None,
                    error: Some(probe_error(&child)),
                });
            }
            Err(error) => reports.push(ScoreEntry {
                id: entry.id,
                sha256: entry.sha256.to_ascii_lowercase(),
                status: "failed",
                probe: None,
                error: Some(error.chars().take(512).collect()),
            }),
        }
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
    let report = ScoreReport {
        schema: 1,
        engine: "formualizer-calamine-0.8.4",
        stored_value_comparison: "not_implemented",
        summary: ScoreSummary {
            files,
            succeeded,
            failed,
            timed_out,
            value_cells_observed: value_cells,
            formula_cells_observed: formula_cells,
            formula_cells_loaded: formulas_loaded,
            formula_parse_rate,
        },
        entries: reports,
    };
    let mut destination = open_new(output)?;
    serde_json::to_writer_pretty(&mut destination, &report).map_err(|error| error.to_string())?;
    destination
        .write_all(b"\n")
        .map_err(|error| error.to_string())?;
    destination.sync_all().map_err(|error| error.to_string())?;
    Ok(!require_all || failed == 0)
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

    #[test]
    fn manifest_ids_and_hashes_are_strict() {
        assert!(valid_id("enron-0001_A"));
        assert!(!valid_id("contains a space"));
        assert!(valid_sha256(&"a".repeat(64)));
        assert!(!valid_sha256(&"g".repeat(64)));
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
}
