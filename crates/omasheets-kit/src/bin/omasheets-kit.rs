//! Explicit native workbook admission and a Rust supervisor for the owned grid.
//!
//! Imported files stay in a durable private working directory. Every window
//! takes the same service lease as the existing desktop launcher, so closing
//! the window that started a service cannot interrupt another open window.

use omasheets_kit::{KitError, ProbeReport, import_xlsx, probe};
use omasheets_service::{Request, Response};
use serde_json::{Value, json};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const USAGE: &str = "usage:\n  omasheets-kit --provenance\n  omasheets-kit probe INPUT\n  omasheets-kit import INPUT.xlsx OUTPUT.omasheets\n  omasheets-kit open INPUT [--working OUTPUT.omasheets] [--runtime-dir DIR]";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_PROBE_BYTES: u64 = 4096;
const MAX_DOCUMENT_BYTES: u64 = 4 * 1024 * 1024;
static INTERRUPTED: AtomicI32 = AtomicI32::new(0);

extern "C" fn remember_interrupt(signal: libc::c_int) {
    // The handler only records the first signal. Process operations, waits
    // and lease cleanup all run in the ordinary supervisor code.
    let _ = INTERRUPTED.compare_exchange(0, signal, Ordering::Relaxed, Ordering::Relaxed);
}

struct InterruptGuard(Vec<(i32, libc::sigaction)>);

impl InterruptGuard {
    fn install() -> Result<Self, String> {
        INTERRUPTED.store(0, Ordering::Relaxed);
        let mut guard = Self(Vec::new());
        for signal in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM] {
            // SAFETY: zero-initialized sigaction structs are populated before
            // use; both pointers remain valid throughout each libc call.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = remember_interrupt as *const () as usize;
            unsafe { libc::sigemptyset(&mut action.sa_mask) };
            if unsafe { libc::sigaction(signal, &action, &mut previous) } != 0 {
                return Err(format!(
                    "cannot install native supervisor signal handler: {}",
                    std::io::Error::last_os_error()
                ));
            }
            guard.0.push((signal, previous));
        }
        Ok(guard)
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        for (signal, previous) in self.0.iter().rev() {
            // SAFETY: previous is the action returned by sigaction for this
            // signal and lives until the call completes.
            unsafe { libc::sigaction(*signal, previous, std::ptr::null_mut()) };
        }
    }
}

fn check_interrupted() -> Result<(), String> {
    match INTERRUPTED.load(Ordering::Relaxed) {
        0 => Ok(()),
        signal => Err(format!("native grid launch interrupted by signal {signal}")),
    }
}

fn current_uid() -> u32 {
    // SAFETY: geteuid has no parameters, side effects or pointer requirements.
    unsafe { libc::geteuid() }
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    if !path.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_dir()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(format!(
            "{} must be a private directory owned by the current user",
            path.display()
        ));
    }
    Ok(())
}

struct Lease(File);

impl Lease {
    fn open(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file()
            || metadata.uid() != current_uid()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(format!(
                "{} must be a private regular lock file",
                path.display()
            ));
        }
        Ok(Self(file))
    }

    fn lock(&self, operation: i32) -> Result<(), String> {
        loop {
            // SAFETY: this descriptor belongs to the live File; flock neither
            // retains a pointer nor closes or transfers ownership of it.
            if unsafe { libc::flock(self.0.as_raw_fd(), operation) } == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(format!("cannot acquire native service lease: {error}"));
            }
        }
    }

    fn lock_interruptible(&self, operation: i32) -> Result<(), String> {
        loop {
            check_interrupted()?;
            // SAFETY: the live File owns this descriptor throughout flock.
            if unsafe { libc::flock(self.0.as_raw_fd(), operation | libc::LOCK_NB) } == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if !matches!(
                error.kind(),
                std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
            ) {
                return Err(format!("cannot acquire native service lease: {error}"));
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

fn read_token(directory: &Path) -> Result<String, String> {
    let path = directory.join("native.token");
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|error| format!("cannot read native service token: {error}"))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err("native service token must be a private same-user regular file".into());
    }
    let mut token = String::new();
    file.take(66)
        .read_to_string(&mut token)
        .map_err(|error| error.to_string())?;
    let token = token.trim();
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("native service token has an invalid format".into());
    }
    Ok(token.to_string())
}

/// Authenticate without opening or mutating a workbook. An unknown request
/// receives invalid_request only after the service accepts its session token.
fn service_ready(directory: &Path) -> Result<bool, String> {
    let socket = directory.join("native.sock");
    let metadata = match fs::symlink_metadata(&socket) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.file_type().is_socket() || metadata.uid() != current_uid() {
        return Err("native endpoint must be a same-user Unix socket".into());
    }
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(format!("cannot connect to native service: {error}")),
    };
    stream
        .set_read_timeout(Some(PROBE_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(PROBE_TIMEOUT))
        .map_err(|error| error.to_string())?;
    writeln!(stream, "{}", read_token(directory)?).map_err(|error| error.to_string())?;
    stream
        .write_all(b"{\"kind\":\"nativekit_readiness_probe\"}\n")
        .map_err(|error| error.to_string())?;
    let mut response = String::new();
    BufReader::new(stream)
        .take(MAX_PROBE_BYTES + 1)
        .read_line(&mut response)
        .map_err(|error| format!("native service did not answer authentication probe: {error}"))?;
    if response.len() as u64 > MAX_PROBE_BYTES || !response.ends_with('\n') {
        return Err("native service authentication response is incomplete or too large".into());
    }
    let envelope: Value = serde_json::from_str(&response).map_err(|error| error.to_string())?;
    if envelope["ok"] == false && envelope["error"]["code"] == "invalid_request" {
        Ok(true)
    } else {
        Err("running native service rejected its authenticated readiness probe".into())
    }
}

/// Read the current document through the service that owns its SQLite/WAL
/// state. A typed document response forces native schema validation and replay;
/// neither the database nor an uncheckpointed WAL is copied here.
fn validate_document(directory: &Path, document: &Path) -> Result<(), String> {
    let socket = directory.join("native.sock");
    let metadata = fs::symlink_metadata(&socket).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_socket() || metadata.uid() != current_uid() {
        return Err("native endpoint must be a same-user Unix socket".into());
    }
    let mut stream = UnixStream::connect(socket).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(STARTUP_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(STARTUP_TIMEOUT))
        .map_err(|error| error.to_string())?;
    writeln!(stream, "{}", read_token(directory)?).map_err(|error| error.to_string())?;
    serde_json::to_writer(
        &mut stream,
        &Request::Document {
            path: document.to_path_buf(),
            branch: None,
        },
    )
    .map_err(|error| error.to_string())?;
    stream.write_all(b"\n").map_err(|error| error.to_string())?;
    let mut response = String::new();
    BufReader::new(stream)
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_line(&mut response)
        .map_err(|error| format!("native service did not validate the workbook: {error}"))?;
    if response.len() as u64 > MAX_DOCUMENT_BYTES || !response.ends_with('\n') {
        return Err("native document response is incomplete or too large".into());
    }
    let envelope: Value = serde_json::from_str(&response).map_err(|error| error.to_string())?;
    if envelope["ok"] != true {
        return Err(format!(
            "native service refused the workbook: {}",
            envelope["error"]
        ));
    }
    match serde_json::from_value::<Response>(envelope["response"].clone()) {
        Ok(Response::Document(_)) => Ok(()),
        _ => Err("native service did not return a valid document summary".into()),
    }
}

/// This guard owns only the process it spawned. It cannot stop a reused service.
struct OwnedService {
    child: Option<Child>,
    directory: PathBuf,
    token: Option<String>,
}

impl OwnedService {
    fn stop_when_alone(&mut self, lease: &Lease) -> Result<(), String> {
        lease.lock(libc::LOCK_UN)?;
        // An interruption may close this grid, but the service still belongs
        // to every peer holding a shared lease. Finish their lifetimes first.
        lease.lock(libc::LOCK_EX)?;
        self.stop()
    }

    fn stop(&mut self) -> Result<(), String> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            // SAFETY: the unreaped Child identifies the process owned here.
            // SIGTERM allows the same shutdown grace as the desktop launcher.
            let sent = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
            if sent != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                child.kill().map_err(|error| error.to_string())?;
            }
            let deadline = Instant::now() + STOP_TIMEOUT;
            while child
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_none()
            {
                if Instant::now() >= deadline {
                    child.kill().map_err(|error| error.to_string())?;
                    child.wait().map_err(|error| error.to_string())?;
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
        // Serialize stale endpoint cleanup with all desktop/grid startups.
        // Preserve any endpoint or token created by a live replacement service.
        let startup = Lease::open(&self.directory.join("grid-service.lock"))?;
        startup.lock(libc::LOCK_EX)?;
        if UnixStream::connect(self.directory.join("native.sock")).is_err()
            && self
                .token
                .as_ref()
                .is_some_and(|owned| read_token(&self.directory).ok().as_ref() == Some(owned))
        {
            for name in ["native.sock", "native.token"] {
                match fs::remove_file(self.directory.join(name)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
        }
        Ok(())
    }
}

impl Drop for OwnedService {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!("omasheets-kit: native service cleanup: {error}");
        }
    }
}

struct LaunchConfig {
    runtime: PathBuf,
    grid: PathBuf,
    service: PathBuf,
}

fn ensure_service(
    config: &LaunchConfig,
    directory: &Path,
    lease: &Lease,
) -> Result<Option<OwnedService>, String> {
    check_interrupted()?;
    if service_ready(directory)? {
        return Ok(None);
    }
    let startup = Lease::open(&directory.join("grid-service.lock"))?;
    startup.lock_interruptible(libc::LOCK_EX)?;
    if service_ready(directory)? {
        return Ok(None);
    }
    let child = Command::new(&config.service)
        .args(["serve", "--runtime-dir"])
        .arg(&config.runtime)
        .env("XDG_RUNTIME_DIR", &config.runtime)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("cannot start native document service: {error}"))?;
    let mut owned = OwnedService {
        child: Some(child),
        directory: directory.to_path_buf(),
        token: None,
    };
    let result = (|| {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let mut detail = None;
        loop {
            check_interrupted()?;
            match service_ready(directory) {
                Ok(true)
                    if owned
                        .child
                        .as_mut()
                        .unwrap()
                        .try_wait()
                        .map_err(|error| error.to_string())?
                        .is_none() =>
                {
                    owned.token = Some(read_token(directory)?);
                    return Ok(());
                }
                Ok(_) => {}
                Err(error) => {
                    detail = Some(error);
                }
            }
            if owned
                .child
                .as_mut()
                .unwrap()
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_some()
                || Instant::now() >= deadline
            {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "native document service did not become ready{}",
            detail.map(|error| format!(": {error}")).unwrap_or_default()
        ))
    })();
    // The guard also acquires the startup lock during cleanup.
    drop(startup);
    match result {
        Ok(()) => Ok(Some(owned)),
        Err(error) => {
            owned.token = read_token(directory).ok();
            owned.stop_when_alone(lease)?;
            Err(error)
        }
    }
}

fn signal_grid(grid: &Child, signal: i32) -> Result<(), String> {
    // SAFETY: process_group(0) made this unreaped child the leader of its own
    // group. No reused service or peer grid can belong to this group.
    if unsafe { libc::kill(-(grid.id() as libc::pid_t), signal) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(format!("cannot interrupt native grid: {error}"))
    }
}

fn supervise_grid(mut grid: Child) -> Result<ExitStatus, String> {
    let result = (|| {
        let mut interrupted = None;
        loop {
            if let Some(status) = grid.try_wait().map_err(|error| error.to_string())? {
                return Ok(status);
            }
            let signal = INTERRUPTED.load(Ordering::Relaxed);
            if signal != 0 && interrupted.is_none() {
                signal_grid(&grid, signal)?;
                interrupted = Some(Instant::now() + STOP_TIMEOUT);
            }
            if interrupted.is_some_and(|deadline| Instant::now() >= deadline) {
                signal_grid(&grid, libc::SIGKILL)?;
                return grid.wait().map_err(|error| error.to_string());
            }
            thread::sleep(Duration::from_millis(25));
        }
    })();
    if result.is_err() {
        let _ = signal_grid(&grid, libc::SIGKILL);
        let _ = grid.wait();
    }
    result
}

fn run_grid_with(
    config: &LaunchConfig,
    document: &Path,
    before_launch: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<ExitStatus, String> {
    let directory = config.runtime.join("omasheets");
    ensure_private_directory(&directory)?;
    let lease = Lease::open(&directory.join("grid-clients.lock"))?;
    lease.lock_interruptible(libc::LOCK_SH)?;
    let mut owned = ensure_service(config, &directory, &lease)?;
    let result = (|| {
        check_interrupted()?;
        before_launch(&directory)?;
        check_interrupted()?;
        let grid = Command::new(&config.grid)
            .arg(document)
            .env("OMASHEETS_DOCUMENT", document)
            .env("XDG_RUNTIME_DIR", &config.runtime)
            .process_group(0)
            .spawn()
            .map_err(|error| format!("cannot launch native grid: {error}"))?;
        supervise_grid(grid)
    })();
    if let Some(service) = owned.as_mut() {
        // The owning supervisor waits for every other launcher lease. A new
        // window cannot race shutdown while this exclusive lease is held.
        service.stop_when_alone(&lease)?;
    } else {
        lease.lock(libc::LOCK_UN)?;
    }
    result
}

#[cfg(test)]
fn run_grid(config: &LaunchConfig, document: &Path) -> Result<ExitStatus, String> {
    run_grid_with(config, document, |_| Ok(()))
}

fn executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn find_executable(variable: &str, name: &str) -> Result<PathBuf, String> {
    if let Some(path) = env::var_os(variable) {
        let path = PathBuf::from(path);
        return executable(&path)
            .then_some(path)
            .ok_or_else(|| format!("{variable} does not name an executable {name}"));
    }
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        let path = parent.join(name);
        if executable(&path) {
            return Ok(path);
        }
    }
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(name);
        if executable(&path) {
            return Ok(path);
        }
    }
    Err(format!(
        "{name} is not installed; install the OmaSheets native bundle"
    ))
}

fn data_directory() -> Result<PathBuf, String> {
    if let Some(base) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(base));
    }
    env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/share"))
        .ok_or_else(|| "neither XDG_DATA_HOME nor HOME is set".into())
}

fn private_working_path(base: &Path, source: &Path) -> Result<PathBuf, String> {
    let directory = base.join("omasheets/native-kit");
    ensure_private_directory(&directory)?;
    let mut nonce = [0_u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut nonce))
        .map_err(|error| error.to_string())?;
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let working = directory.join(nonce);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&working)
        .map_err(|error| error.to_string())?;
    let name = source
        .file_stem()
        .unwrap_or_else(|| std::ffi::OsStr::new("Workbook"));
    let mut filename = name.to_os_string();
    filename.push(".omasheets");
    Ok(working.join(filename))
}

fn kit_error(error: KitError) -> Value {
    match error {
        KitError::Admission(report) => {
            json!({"ok": false, "engine": "omasheets-kit", "error": "workbook is outside the native support contract", "probe": report})
        }
        error => json!({"ok": false, "engine": "omasheets-kit", "error": error.to_string()}),
    }
}

fn open(arguments: &[String]) -> Result<ExitStatus, Value> {
    let as_error = |error: String| json!({"ok": false, "engine": "omasheets-kit", "error": error});
    let _signals = InterruptGuard::install().map_err(as_error)?;
    let mut source = None;
    let mut working = None;
    let mut runtime = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--working" | "--runtime-dir" if index + 1 < arguments.len() => {
                let value = PathBuf::from(&arguments[index + 1]);
                if arguments[index] == "--working" && working.is_none() {
                    working = Some(value);
                } else if arguments[index] == "--runtime-dir" && runtime.is_none() {
                    runtime = Some(value);
                } else {
                    return Err(json!({"ok": false, "error": USAGE}));
                }
                index += 2;
            }
            argument if !argument.starts_with('-') && source.is_none() => {
                source = Some(PathBuf::from(argument));
                index += 1;
            }
            _ => return Err(json!({"ok": false, "error": USAGE})),
        }
    }
    let source = source.ok_or_else(|| json!({"ok": false, "error": USAGE}))?;
    let source = source
        .canonicalize()
        .map_err(|error| as_error(error.to_string()))?;
    let runtime = runtime
        .or_else(|| {
            env::var_os("XDG_RUNTIME_DIR")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .ok_or_else(|| as_error("XDG_RUNTIME_DIR is not set; pass --runtime-dir".into()))?;
    let runtime = runtime
        .canonicalize()
        .map_err(|error| as_error(format!("runtime directory is unavailable: {error}")))?;
    let config = LaunchConfig {
        runtime,
        grid: find_executable("OMASHEETS_GRID", "omasheets-grid").map_err(as_error)?,
        service: find_executable("OMASHEETS_NATIVE_SERVICE", "omasheets-service")
            .map_err(as_error)?,
    };
    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (document, report, private) = match extension.as_str() {
        // The service, rather than a private database copy, validates native
        // opens below. Its live document includes uncheckpointed edits.
        "omasheets" if working.is_none() => (source.clone(), None, false),
        "xlsx" => {
            let private = working.is_none();
            let document = match working {
                Some(path) => path,
                None => private_working_path(&data_directory().map_err(as_error)?, &source)
                    .map_err(as_error)?,
            };
            let report = import_xlsx(&source, &document).map_err(kit_error)?;
            let document = document
                .canonicalize()
                .map_err(|error| as_error(error.to_string()))?;
            (document, Some(report), private)
        }
        "omasheets" => return Err(as_error("--working applies only to an XLSX import".into())),
        _ => {
            return Err(kit_error(KitError::Admission(Box::new(
                probe(&source).map_err(kit_error)?,
            ))));
        }
    };
    run_grid_with(&config, &document, |directory| {
        validate_document(directory, &document)?;
        let report = report.unwrap_or_else(|| ProbeReport {
            format: "omasheets-native-v1".into(),
            source_sha256: None,
            can_import: true,
            reasons: Vec::new(),
            import_manifest: None,
        });
        println!(
            "{}",
            json!({"ok": true, "engine": "omasheets-kit", "source": source, "working_document": document, "source_preserved": extension == "xlsx", "private_working_copy": private, "working_document_retained": true, "native_validation": "authenticated_service", "probe": report})
        );
        std::io::stdout().flush().map_err(|error| error.to_string())
    })
    .map_err(as_error)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let result = match arguments.as_slice() {
        [argument] if argument == "--provenance" => {
            println!("{}", json!({"component": "omasheets-kit", "source_commit": option_env!("OMASHEETS_SOURCE_COMMIT").unwrap_or("development"), "source_sha256": option_env!("OMASHEETS_SOURCE_SHA256").unwrap_or("development")}));
            Ok(0)
        }
        [command, source] if command == "probe" => probe(source).map(|report| { println!("{}", serde_json::to_string(&report).expect("probe report is serializable")); if report.can_import { 0 } else { 2 } }).map_err(kit_error),
        [command, source, output] if command == "import" => import_xlsx(source, output).map(|report| { println!("{}", json!({"ok": true, "engine": "omasheets-kit", "working_document": output, "source_preserved": true, "probe": report})); 0 }).map_err(kit_error),
        [command, rest @ ..] if command == "open" => {
            let result = open(rest).map(|status| status.code().unwrap_or(1) as u8);
            let signal = INTERRUPTED.load(Ordering::Relaxed);
            if signal == 0 { result } else { Ok((128 + signal) as u8) }
        },
        _ => Err(json!({"ok": false, "engine": "omasheets-kit", "error": USAGE})),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let mut bytes = [0_u8; 16];
            File::open("/dev/urandom")
                .unwrap()
                .read_exact(&mut bytes)
                .unwrap();
            let name: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = env::temp_dir().join(format!("omasheets-kit-cli-{name}"));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.path(name);
            fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }

        fn service(&self) -> PathBuf {
            let executable = PathBuf::from(env::args_os().next().unwrap())
                .canonicalize()
                .unwrap();
            self.script("service-stub", &format!(
                "export OMASHEETS_KIT_STUB_RUNTIME={}\nexec {} --exact tests::service_process_stub --nocapture",
                quote(&self.0), quote(&executable),
            ))
        }

        fn config(&self, grid: PathBuf) -> LaunchConfig {
            LaunchConfig {
                runtime: self.0.clone(),
                grid,
                service: self.service(),
            }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
    }

    fn wait_for(mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if predicate() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Only an explicitly isolated subprocess enters this listener. Its
    /// real token handshake lets supervisor tests exercise process ownership.
    #[test]
    fn service_process_stub() {
        let Some(runtime) = env::var_os("OMASHEETS_KIT_STUB_RUNTIME") else {
            return;
        };
        let directory = PathBuf::from(runtime).join("omasheets");
        ensure_private_directory(&directory).unwrap();
        let token = "a".repeat(64);
        let token_path = directory.join("native.token");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(token_path)
            .unwrap();
        writeln!(file, "{token}").unwrap();
        let listener = UnixListener::bind(directory.join("native.sock")).unwrap();
        fs::write(directory.join("stub.pid"), std::process::id().to_string()).unwrap();
        for connection in listener.incoming() {
            let Ok(mut stream) = connection else {
                continue;
            };
            let token = token.clone();
            thread::spawn(move || {
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut presented = String::new();
                if reader.read_line(&mut presented).is_err() || presented.is_empty() {
                    return;
                }
                if presented.trim() != token {
                    let _ =
                        stream.write_all(b"{\"ok\":false,\"error\":{\"code\":\"unauthorized\"}}\n");
                    return;
                }
                let mut request = String::new();
                if reader.read_line(&mut request).is_ok() && !request.is_empty() {
                    let _ = stream
                        .write_all(b"{\"ok\":false,\"error\":{\"code\":\"invalid_request\"}}\n");
                }
            });
        }
    }

    #[test]
    fn imported_working_paths_are_private_durable_and_distinct() {
        let directory = TestDirectory::new();
        let source = directory.path("budget $(touch unwanted).xlsx");
        fs::write(&source, b"source bytes").unwrap();
        let first = private_working_path(&directory.0, &source).unwrap();
        let second = private_working_path(&directory.0, &source).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.extension().unwrap(), "omasheets");
        assert_eq!(
            first
                .parent()
                .unwrap()
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        fs::write(&first, b"durable edits").unwrap();
        assert_eq!(fs::read(source).unwrap(), b"source bytes");
        assert_eq!(fs::read(&first).unwrap(), b"durable edits");
    }

    #[test]
    fn private_directory_and_lock_refuse_broad_permissions_and_symlinks() {
        let directory = TestDirectory::new();
        let broad = directory.path("broad");
        fs::create_dir(&broad).unwrap();
        fs::set_permissions(&broad, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ensure_private_directory(&broad).is_err());
        let linked = directory.path("linked");
        std::os::unix::fs::symlink(&directory.0, &linked).unwrap();
        assert!(ensure_private_directory(&linked).is_err());
        let lock = directory.path("lease");
        std::os::unix::fs::symlink(directory.path("target"), &lock).unwrap();
        assert!(Lease::open(&lock).is_err());
        assert!(!directory.path("target").exists());
    }

    #[test]
    fn grid_argv_and_environment_preserve_literal_workbook_paths() {
        let directory = TestDirectory::new();
        let document = directory.path("budget $(touch unwanted) 'quote'.omasheets");
        fs::write(&document, b"native").unwrap();
        let output = directory.path("grid-arguments");
        let grid = directory.script(
            "grid-stub",
            &format!(
                "printf '%s\\n' \"$1\" \"$OMASHEETS_DOCUMENT\" \"$XDG_RUNTIME_DIR\" > {}",
                quote(&output),
            ),
        );
        let config = directory.config(grid);
        assert!(run_grid(&config, &document).unwrap().success());
        let lines = fs::read_to_string(output).unwrap();
        assert_eq!(
            lines.lines().collect::<Vec<_>>(),
            vec![
                document.to_str().unwrap(),
                document.to_str().unwrap(),
                directory.0.to_str().unwrap()
            ]
        );
        assert!(!directory.path("unwanted").exists());
        assert_eq!(fs::read(document).unwrap(), b"native");
        assert!(!directory.path("omasheets/native.sock").exists());
        assert!(!directory.path("omasheets/native.token").exists());
    }

    #[test]
    fn a_reused_service_survives_grid_exit() {
        let directory = TestDirectory::new();
        let grid = directory.script("grid-stub", "exit 0");
        let config = directory.config(grid);
        ensure_private_directory(&directory.path("omasheets")).unwrap();
        let mut external = Command::new(&config.service)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let ready = wait_for(|| service_ready(&directory.path("omasheets")).unwrap_or(false));
        let result = if ready {
            run_grid(&config, &directory.path("book.omasheets"))
        } else {
            Err("stub did not start".into())
        };
        let alive = external.try_wait().unwrap().is_none();
        external.kill().unwrap();
        external.wait().unwrap();
        assert!(ready);
        assert!(result.unwrap().success());
        assert!(alive);
    }

    #[test]
    fn failed_grid_start_stops_only_its_owned_service() {
        let directory = TestDirectory::new();
        let config = directory.config(directory.path("missing-grid"));
        let error = run_grid(&config, &directory.path("book.omasheets")).unwrap_err();
        assert!(error.contains("cannot launch native grid"));
        assert!(!directory.path("omasheets/native.sock").exists());
        assert!(!directory.path("omasheets/native.token").exists());
    }

    #[test]
    fn early_service_exit_is_reported_without_starting_the_grid() {
        let directory = TestDirectory::new();
        let ran = directory.path("grid-ran");
        let grid = directory.script("grid-stub", &format!("touch {}", quote(&ran)));
        let mut config = directory.config(grid);
        config.service = directory.script("failed-service", "exit 9");
        let error = run_grid(&config, &directory.path("book.omasheets")).unwrap_err();
        assert!(error.contains("did not become ready"));
        assert!(!ran.exists());
    }

    #[test]
    fn owning_supervisor_waits_for_other_grid_leases_before_shutdown() {
        let directory = TestDirectory::new();
        let owner_ready = directory.path("owner-ready");
        let peer_ready = directory.path("peer-ready");
        let owner_close = directory.path("owner-close");
        let peer_close = directory.path("peer-close");
        let owner_exited = directory.path("owner-exited");
        let owner_grid = directory.script(
            "owner-grid",
            &format!(
                "touch {}\nwhile [ ! -f {} ]; do sleep 0.02; done\ntouch {}",
                quote(&owner_ready),
                quote(&owner_close),
                quote(&owner_exited),
            ),
        );
        let peer_grid = directory.script(
            "peer-grid",
            &format!(
                "touch {}\nwhile [ ! -f {} ]; do sleep 0.02; done",
                quote(&peer_ready),
                quote(&peer_close),
            ),
        );
        let owner_config = directory.config(owner_grid);
        let peer_config = directory.config(peer_grid);
        let owner_document = directory.path("owner.omasheets");
        let peer_document = directory.path("peer.omasheets");
        let (sent, received) = mpsc::channel();
        let owner = thread::spawn(move || {
            let result = run_grid(&owner_config, &owner_document);
            let _ = sent.send(result);
        });
        let owner_started = wait_for(|| owner_ready.exists());
        let peer = thread::spawn(move || run_grid(&peer_config, &peer_document));
        let peer_started = wait_for(|| peer_ready.exists());
        fs::write(&owner_close, b"close").unwrap();
        let owner_window_exited = wait_for(|| owner_exited.exists());
        let owner_still_waiting = received.recv_timeout(Duration::from_millis(100)).is_err();
        let service_still_alive = service_ready(&directory.path("omasheets")).unwrap_or(false);
        // Always release the peer before assertions to avoid a stranded owner.
        fs::write(&peer_close, b"close").unwrap();
        let peer_status = peer.join().unwrap();
        owner.join().unwrap();
        let owner_status = received.recv_timeout(Duration::from_secs(1));
        assert!(owner_started && peer_started && owner_window_exited);
        assert!(owner_still_waiting && service_still_alive);
        assert!(peer_status.unwrap().success());
        assert!(owner_status.unwrap().unwrap().success());
        assert!(!directory.path("omasheets/native.sock").exists());
    }
}
