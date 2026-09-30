//! Run after building omasheets-app and omasheets-service. No interpreter needed.
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
struct Run {
    root: PathBuf,
    children: Vec<Child>,
}
impl Drop for Run {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "acceptance condition timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn main() {
    let bin = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let service = bin.join("omasheets-service");
    let app = bin.join("omasheets");
    assert!(
        service.is_file() && app.is_file(),
        "Build app and service first"
    );
    let root =
        std::env::temp_dir().join(format!("omasheets-rust-acceptance-{}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let mut run = Run {
        root: root.clone(),
        children: vec![],
    };
    let runtime = root.join("runtime");
    fs::DirBuilder::new().mode(0o700).create(&runtime).unwrap();
    let socket = runtime.join("omasheets/native.sock");
    let command = |args: &[&str]| {
        let mut c = Command::new(&app);
        c.args(args)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("PATH", "/nonexistent");
        c
    };
    run.children.push(
        Command::new(&service)
            .args(["serve", "--runtime-dir"])
            .arg(&runtime)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait(|| UnixStream::connect(&socket).is_ok());
    let document = root.join("acceptance.omasheets");
    let raw = |request: Value| -> Value {
        let out = Command::new(&service)
            .args(["call", "--runtime-dir"])
            .arg(&runtime)
            .arg(request.to_string())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["response"].clone()
    };
    raw(
        json!({"kind":"create","path":document,"name":"Rust migration","actor":{"kind":"human","id":"acceptance"}}),
    );
    raw(
        json!({"kind":"append","path":document,"actor":{"kind":"human","id":"acceptance"},"command":{"command":"add_sheet","name":"Data"}}),
    );
    let summary = raw(json!({"kind":"document","path":document}));
    let sheet = &summary["sheets"][0]["id"];
    // A newly added core sheet has no row/column identities. Allocate the
    // fixture's view before proposing A1, as the real grid does on creation.
    for command in [
        json!({"command":"add_columns","sheet":sheet,"at":0,"count":1}),
        json!({"command":"add_rows","sheet":sheet,"at":0,"count":1,"table":null}),
    ] {
        raw(
            json!({"kind":"append","path":document,"actor":{"kind":"human","id":"acceptance"},"command":command}),
        );
    }
    let session = json!({"schema":1,"session_id":"a".repeat(32),"pid":std::process::id(),"path":document,"selection":{"sheet":sheet,"row":0,"column":0,"rows":1,"columns":1}});
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(runtime.join("omasheets/native-agent-session.json"))
        .unwrap()
        .write_all(session.to_string().as_bytes())
        .unwrap();
    let out = command(&["agent-session", "resource"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let resource: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains(root.to_str().unwrap()));
    let args=json!({"session_id":"a".repeat(32),"expected_revision":resource["overview"]["revision"],"goal":"Enter amount","explanation":"Synthetic parity check","assumptions":[],"evidence":["Fixed acceptance value"],"edits":[{"sheet":sheet,"a1":"A1","value":"42"}]}).to_string();
    let out = command(&[
        "agent-session",
        "call",
        "native_propose",
        "--arguments",
        &args,
    ])
    .output()
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let proposed: Value = serde_json::from_slice(&out.stdout).unwrap();
    let args = json!({"session_id":"a".repeat(32),"branch":proposed["branch"]}).to_string();
    let out = command(&[
        "agent-session",
        "call",
        "native_review",
        "--arguments",
        &args,
    ])
    .output()
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        raw(json!({"kind":"document","path":document}))["revision"],
        resource["overview"]["revision"],
        "proposal must not mutate main"
    );
    println!(
        "PASS: authenticated Rust native resource/propose/review, main unchanged, PATH contains no Python"
    );
    // Stop the explicitly started service; let the app supervise the next instance.
    unsafe {
        libc::kill(run.children[0].id() as i32, libc::SIGTERM);
    }
    run.children[0].wait().unwrap();
    // Rust helper below is also the fake grid: no shell or Python involved.
    let grid = bin.join("examples/grid_fixture");
    assert!(grid.is_file(), "build --example grid_fixture");
    let host = |marker: &Path| {
        let mut c = command(&["--native-host"]);
        c.env("OMASHEETS_GRID", &grid)
            .env("OMASHEETS_NATIVE_SERVICE", &service)
            .env("OMASHEETS_TEST_MARKER", marker)
            .stdout(Stdio::null());
        c.spawn().unwrap()
    };
    let first = root.join("first");
    let second = root.join("second");
    run.children.push(host(&first));
    wait(|| first.exists());
    run.children.push(host(&second));
    wait(|| second.exists());
    fs::write(first.with_extension("close"), b"").unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        UnixStream::connect(&socket).is_ok(),
        "second window must retain service"
    );
    fs::write(second.with_extension("close"), b"").unwrap();
    wait(|| run.children[1].try_wait().unwrap().is_some());
    wait(|| run.children[2].try_wait().unwrap().is_some());
    wait(|| !socket.exists());
    println!(
        "PASS: two Rust-supervised windows share service; owner waits for final window before cleanup"
    );
    // Ensure the fixture marker came from the Rust executable.
    let mut text = String::new();
    fs::File::open(first)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "rust-grid-fixture");
}
