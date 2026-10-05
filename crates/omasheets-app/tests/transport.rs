use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::UnixListener;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
static NONCE: AtomicUsize = AtomicUsize::new(0);
struct Session {
    root: std::path::PathBuf,
}
impl Session {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "omasheets-ipc-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("omasheets"))
            .unwrap();
        let this = Self { root };
        this.write("native.token", b"test-secret\n");
        this.write("native-agent-session.json",json!({"schema":1,"session_id":"a".repeat(32),"pid":std::process::id(),"path":"/private/document.omasheets","selection":{"sheet":"b".repeat(32),"row":0,"column":0,"rows":1,"columns":1}}).to_string().as_bytes());
        this
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.root.join("omasheets").join(name))
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_omasheets"))
            .args(args)
            .env("XDG_RUNTIME_DIR", &self.root)
            .env("PATH", "/nonexistent")
            .output()
            .unwrap()
    }
    fn socket(&self) -> UnixListener {
        UnixListener::bind(self.root.join("omasheets/native.sock")).unwrap()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
#[test]
fn resource_is_path_free_and_authenticates_without_any_interpreter() {
    let session = Session::new();
    let listener = session.socket();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut token = String::new();
        reader.read_line(&mut token).unwrap();
        assert_eq!(token, "test-secret\n");
        let mut request = String::new();
        reader.read_line(&mut request).unwrap();
        let value: Value = serde_json::from_str(&request).unwrap();
        assert_eq!(
            value,
            json!({"kind":"document","path":"/private/document.omasheets"})
        );
        reader
            .get_mut()
            .write_all(b"{\"ok\":true,\"response\":{\"revision\":\"r1\",\"sheets\":[]}}\n")
            .unwrap();
    });
    let output = session.run(&["agent-session", "resource"]);
    server.join().unwrap();
    assert!(output.status.success(), "{output:?}");
    let out = String::from_utf8(output.stdout).unwrap();
    assert!(!out.contains("/private/"));
    assert!(!out.contains("test-secret"));
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["overview"]["revision"],
        "r1"
    );
}
#[test]
fn stale_sessions_and_oversized_reads_never_connect() {
    let session = Session::new();
    let listener = session.socket();
    listener.set_nonblocking(true).unwrap();
    let stale = json!({"session_id":"c".repeat(32)}).to_string();
    assert!(
        !session
            .run(&[
                "agent-session",
                "call",
                "native_overview",
                "--arguments",
                &stale
            ])
            .status
            .success()
    );
    let read=json!({"session_id":"a".repeat(32),"sheet":"b".repeat(32),"row":0,"column":0,"rows":100,"columns":100}).to_string();
    assert!(
        !session
            .run(&["agent-session", "call", "native_read", "--arguments", &read])
            .status
            .success()
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
#[test]
fn ambiguous_proposal_write_is_never_retried() {
    let session = Session::new();
    let listener = session.socket();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut token = String::new();
        reader.read_line(&mut token).unwrap();
        let mut text = String::new();
        reader.read_line(&mut text).unwrap();
        let request: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(request["kind"], "propose_native");
        assert_eq!(request["expected_revision"], "r".repeat(130));
        assert_eq!(
            request["proposal"]["commands"][0]["value"],
            json!({"type":"number","value":15.0})
        );
        assert!(request["proposal"].get("session_id").is_none());
        drop(reader);
        listener
    });
    let args=json!({"session_id":"a".repeat(32),"expected_revision":"r".repeat(130),"goal":"Update","explanation":"Source changed","assumptions":[],"evidence":["A1"],"edits":[{"sheet":"b".repeat(32),"a1":"A1","value":"15"}]}).to_string();
    let output = session.run(&[
        "agent-session",
        "call",
        "native_propose",
        "--arguments",
        &args,
    ]);
    let listener = server.join().unwrap();
    listener.set_nonblocking(true).unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("inspect current state")
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
#[test]
fn service_errors_redact_selected_path() {
    let session = Session::new();
    let listener = session.socket();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        for _ in 0..2 {
            let mut text = String::new();
            reader.read_line(&mut text).unwrap();
        }
        reader.get_mut().write_all(b"{\"ok\":false,\"error\":{\"message\":\"cannot open /private/document.omasheets\"}}\n").unwrap();
    });
    let output = session.run(&["agent-session", "resource"]);
    server.join().unwrap();
    assert!(!output.status.success());
    let err = String::from_utf8(output.stderr).unwrap();
    assert!(!err.contains("/private/"));
    assert!(err.contains("selected workbook"));
}
