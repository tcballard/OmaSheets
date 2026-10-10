//! Exercise the real CLI entry point with a subprocess service backed by the
//! production document library. Socket/process tests remain mandatory in CI.

use omasheets_core::{Actor, ActorKind, Command as WorkbookCommand, Literal, Operation};
use omasheets_kit::probe;
use omasheets_service::{Request, Response, Service};
use serde_json::{Value, json};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "omasheets-kit-launcher-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
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
        self.script(
            "service",
            &format!(
                "export OMASHEETS_KIT_TEST_RUNTIME={}\nexec {} --exact service_process --nocapture",
                quote(&self.0),
                quote(&executable)
            ),
        )
    }

    fn cli(&self, name: &str, document: &Path, grid: &Path, service: &Path) -> Running {
        let mut command = Command::new(env!("CARGO_BIN_EXE_omasheets-kit"));
        command
            .arg("open")
            .arg(document)
            .arg("--runtime-dir")
            .arg(&self.0)
            .env("OMASHEETS_GRID", grid)
            .env("OMASHEETS_NATIVE_SERVICE", service)
            .stdin(Stdio::null())
            .stdout(File::create(self.path(&format!("{name}.json"))).unwrap())
            .stderr(File::create(self.path(&format!("{name}.error"))).unwrap());
        Running::spawn(&mut command)
    }

    fn grid(&self, name: &str) -> PathBuf {
        self.script(
            name,
            &format!(
                "printf '%s' \"$$\" > {}\ntrap 'exit 0' INT HUP TERM\nwhile [ ! -f {} ]; do sleep 0.02; done",
                quote(&self.path(&format!("{name}.pid"))),
                quote(&self.path(&format!("{name}.close")))
            ),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // On a failed assertion also stop grids/services isolated from the
        // test's own process group, so a failed CI test leaves no child behind.
        if let Ok(entries) = fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                if entry.path().extension().is_some_and(|value| value == "pid")
                    && let Some(pid) = read_pid(&entry.path())
                {
                    signal_group(pid, libc::SIGKILL);
                }
            }
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Running(Child);

impl Running {
    fn spawn(command: &mut Command) -> Self {
        Self(command.process_group(0).spawn().unwrap())
    }

    fn finish(&mut self) -> ExitStatus {
        let mut status = None;
        assert!(wait_for(|| {
            status = self.0.try_wait().unwrap();
            status.is_some()
        }));
        status.unwrap()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            signal_group(self.0.id() as i32, libc::SIGKILL);
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

fn signal_group(pid: i32, signal: i32) {
    // SAFETY: each pid recorded here leads an isolated test child group.
    unsafe { libc::kill(-pid, signal) };
}

fn read_pid(path: &Path) -> Option<i32> {
    fs::read_to_string(path).ok()?.parse().ok()
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 checks existence without sending a signal.
    unsafe { libc::kill(pid, 0) == 0 }
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

fn call(fixture: &Fixture, request: Value) -> Result<Value, String> {
    let token = fs::read_to_string(fixture.path("omasheets/native.token"))
        .map_err(|error| error.to_string())?;
    let mut stream = UnixStream::connect(fixture.path("omasheets/native.sock"))
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    writeln!(stream, "{}\n{request}", token.trim()).unwrap();
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&response).map_err(|error| error.to_string())
}

fn append(service: &mut Service, path: &Path, command: WorkbookCommand) -> Operation {
    match service
        .handle(Request::Append {
            path: path.to_path_buf(),
            branch: None,
            actor: Actor::new(ActorKind::Human, "launcher-test"),
            command,
        })
        .unwrap()
    {
        Response::Appended(event) => event.operation,
        _ => panic!("expected append"),
    }
}

fn create_native(path: &Path) {
    let mut service = Service::default();
    service
        .handle(Request::Create {
            path: path.to_path_buf(),
            name: "Live workbook".into(),
            actor: Actor::new(ActorKind::Human, "launcher-test"),
        })
        .unwrap();
    let Operation::AddSheet { sheet, .. } = append(
        &mut service,
        path,
        WorkbookCommand::AddSheet {
            name: "Data".into(),
        },
    ) else {
        panic!("expected sheet");
    };
    append(
        &mut service,
        path,
        WorkbookCommand::AddColumns {
            sheet,
            count: 1,
            at: 0,
        },
    );
    append(
        &mut service,
        path,
        WorkbookCommand::AddRows {
            sheet,
            count: 1,
            at: 0,
            table: None,
        },
    );
    append(
        &mut service,
        path,
        WorkbookCommand::SetValue {
            sheet,
            a1: "A1".into(),
            value: Literal::Number(3.0),
        },
    );
    service.close_all().unwrap();
}

/// The CLI starts this isolated subprocess instead of an installed service
/// binary. Real production Service requests exercise SQLite, WAL and replay.
#[test]
fn service_process() {
    let Some(runtime) = env::var_os("OMASHEETS_KIT_TEST_RUNTIME") else {
        return;
    };
    let runtime = PathBuf::from(runtime);
    let directory = runtime.join("omasheets");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })
        .unwrap();
    let token = "a".repeat(64);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(directory.join("native.token"))
        .unwrap();
    writeln!(file, "{token}").unwrap();
    let listener = UnixListener::bind(directory.join("native.sock")).unwrap();
    fs::write(runtime.join("service.pid"), std::process::id().to_string()).unwrap();
    let service = Arc::new(Mutex::new(Service::default()));
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let service = Arc::clone(&service);
        let token = token.clone();
        let directory = directory.clone();
        thread::spawn(move || {
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut presented = String::new();
            if reader.read_line(&mut presented).is_err() || presented.trim() != token {
                let _ = writeln!(
                    stream,
                    "{}",
                    json!({"ok": false, "error": {"code": "unauthorized"}})
                );
                return;
            }
            let mut request = String::new();
            if reader.read_line(&mut request).is_err() || request.is_empty() {
                return;
            }
            let envelope = match serde_json::from_str::<Request>(&request) {
                Ok(request) => {
                    if matches!(request, Request::Document { .. }) {
                        fs::write(
                            directory.join("document-validated"),
                            request
                                .document_key()
                                .unwrap()
                                .as_os_str()
                                .as_encoded_bytes(),
                        )
                        .unwrap();
                    }
                    match service.lock().unwrap().handle(request) {
                        Ok(response) => json!({"ok": true, "response": response}),
                        Err(error) => json!({"ok": false, "error": error}),
                    }
                }
                Err(_) => json!({"ok": false, "error": {"code": "invalid_request"}}),
            };
            let _ = writeln!(stream, "{envelope}");
        });
    }
}

#[test]
fn native_open_validates_live_wal_through_the_existing_service() {
    let fixture = Fixture::new();
    let document = fixture.path("book.omasheets");
    create_native(&document);
    let service = fixture.service();
    let mut external = Running::spawn(
        Command::new(&service)
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    );
    assert!(wait_for(|| call(
        &fixture,
        json!({"kind": "nativekit_readiness_probe"})
    )
    .is_ok()));
    let summary = call(&fixture, json!({"kind": "document", "path": document})).unwrap();
    let sheet = summary["response"]["sheets"][0]["id"].clone();
    let edited = call(&fixture, json!({"kind": "append", "path": document, "actor": {"kind": "human", "id": "live-edit"}, "command": {"command": "set_value", "sheet": sheet, "a1": "A1", "value": {"type": "number", "value": 42.0}}})).unwrap();
    assert_eq!(edited["ok"], true, "{edited}");
    assert!(
        !probe(&document).unwrap().can_import,
        "the fixture must reproduce an active WAL"
    );
    let wal = fixture.path("book.omasheets-wal");
    let before_database = fs::read(&document).unwrap();
    let before_wal = fs::read(&wal).unwrap();
    fs::remove_file(fixture.path("omasheets/document-validated")).unwrap();
    let grid = fixture.script(
        "grid",
        &format!("touch {}", quote(&fixture.path("grid-ran"))),
    );
    let mut cli = fixture.cli("open", &document, &grid, &service);
    assert!(
        cli.finish().success(),
        "{}",
        fs::read_to_string(fixture.path("open.error")).unwrap()
    );
    assert!(fixture.path("grid-ran").exists());
    assert!(fixture.path("omasheets/document-validated").exists());
    assert!(external.0.try_wait().unwrap().is_none());
    assert_eq!(fs::read(&document).unwrap(), before_database);
    assert_eq!(fs::read(&wal).unwrap(), before_wal);
    let value = call(
        &fixture,
        json!({"kind": "cell", "path": document, "sheet": sheet, "a1": "A1"}),
    )
    .unwrap();
    assert_eq!(
        value["response"]["value"],
        json!({"type": "number", "value": 42.0})
    );
    let report: Value =
        serde_json::from_slice(&fs::read(fixture.path("open.json")).unwrap()).unwrap();
    assert_eq!(report["native_validation"], "authenticated_service");
}

#[test]
fn native_open_refuses_an_invalid_database_before_starting_the_grid() {
    let fixture = Fixture::new();
    let document = fixture.path("invalid.omasheets");
    fs::write(&document, b"not a native database").unwrap();
    let grid = fixture.script(
        "grid",
        &format!("touch {}", quote(&fixture.path("grid-ran"))),
    );
    let mut cli = fixture.cli("open", &document, &grid, &fixture.service());
    assert_eq!(cli.finish().code(), Some(2));
    assert!(!fixture.path("grid-ran").exists());
    assert!(!fixture.path("omasheets/native.sock").exists());
    assert!(!fixture.path("omasheets/native.token").exists());
    assert_eq!(fs::read(&document).unwrap(), b"not a native database");
    let error: Value =
        serde_json::from_slice(&fs::read(fixture.path("open.error")).unwrap()).unwrap();
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("refused the workbook")
    );
}

#[test]
fn interrupted_owner_stops_its_grid_and_waits_for_the_peer_lease() {
    for signal in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM] {
        let fixture = Fixture::new();
        let document = fixture.path("book.omasheets");
        create_native(&document);
        let service = fixture.service();
        let mut owner = fixture.cli("owner", &document, &fixture.grid("owner-grid"), &service);
        assert!(wait_for(|| fixture.path("owner-grid.pid").exists()));
        let mut peer = fixture.cli("peer", &document, &fixture.grid("peer-grid"), &service);
        assert!(wait_for(|| fixture.path("peer-grid.pid").exists()));
        let owner_grid = read_pid(&fixture.path("owner-grid.pid")).unwrap();
        let peer_grid = read_pid(&fixture.path("peer-grid.pid")).unwrap();
        // Match terminal delivery: the CLI's foreground process group receives
        // the signal. Its grid must be stopped by the supervisor's forwarding.
        signal_group(owner.0.id() as i32, signal);
        assert!(wait_for(|| !alive(owner_grid)));
        assert!(owner.0.try_wait().unwrap().is_none());
        assert!(alive(peer_grid));
        assert!(
            call(&fixture, json!({"kind": "document", "path": document})).unwrap()["ok"] == true
        );
        fs::write(fixture.path("peer-grid.close"), b"close").unwrap();
        assert!(peer.finish().success());
        assert_eq!(owner.finish().code(), Some(128 + signal));
        assert!(!fixture.path("omasheets/native.sock").exists());
        assert!(!fixture.path("omasheets/native.token").exists());
        assert!(!alive(read_pid(&fixture.path("service.pid")).unwrap()));
    }
}

#[test]
fn interruption_during_service_startup_reaps_the_owned_process() {
    let fixture = Fixture::new();
    let document = fixture.path("book.omasheets");
    create_native(&document);
    let service = fixture.script(
        "slow-service",
        &format!(
            "printf '%s' \"$$\" > {}\nexec sleep 30",
            quote(&fixture.path("service.pid"))
        ),
    );
    let grid = fixture.script(
        "grid",
        &format!("touch {}", quote(&fixture.path("grid-ran"))),
    );
    let mut cli = fixture.cli("open", &document, &grid, &service);
    assert!(wait_for(|| fixture.path("service.pid").exists()));
    signal_group(cli.0.id() as i32, libc::SIGINT);
    assert_eq!(cli.finish().code(), Some(130));
    assert!(!alive(read_pid(&fixture.path("service.pid")).unwrap()));
    assert!(!fixture.path("grid-ran").exists());
}
