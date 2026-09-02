use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Server {
    child: Child,
    runtime: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.runtime);
    }
}

fn start_server() -> Server {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let runtime =
        std::env::temp_dir().join(format!("omasheets-runtime-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&runtime).unwrap();
    std::fs::set_permissions(
        &runtime,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_omasheets-service"))
        .args(["serve", "--runtime-dir", runtime.to_str().unwrap()])
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let socket = runtime.join("omasheets/native.sock");
    let started = Instant::now();
    while !socket.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "service did not start"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    Server { child, runtime }
}

fn call(server: &Server, request: &str) -> (i32, serde_json::Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_omasheets-service"))
        .args([
            "call",
            "--runtime-dir",
            server.runtime.to_str().unwrap(),
            request,
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let value = serde_json::from_str(text.trim()).unwrap_or_else(
        |_| serde_json::json!({ "stderr": String::from_utf8_lossy(&output.stderr) }),
    );
    (output.status.code().unwrap_or(-1), value)
}

#[test]
fn the_cli_drives_the_service_through_the_socket_with_a_session_token() {
    let server = start_server();
    let document = server.runtime.join("plan.omasheets");
    let path = document.to_str().unwrap();

    let (code, created) = call(
        &server,
        &format!(
            r#"{{"kind":"create","path":"{path}","name":"Plan","actor":{{"kind":"human","id":"tom"}}}}"#
        ),
    );
    assert_eq!(code, 0, "{created}");
    assert_eq!(created["ok"], true);
    assert_eq!(created["response"]["kind"], "created");

    let (code, appended) = call(
        &server,
        &format!(
            r#"{{"kind":"append","path":"{path}","actor":{{"kind":"human","id":"tom"}},"command":{{"command":"add_sheet","name":"Model"}}}}"#
        ),
    );
    assert_eq!(code, 0, "{appended}");
    assert_eq!(appended["response"]["kind"], "appended");

    let (code, summary) = call(
        &server,
        &format!(r#"{{"kind":"document","path":"{path}"}}"#),
    );
    assert_eq!(code, 0);
    assert_eq!(summary["response"]["sheets"][0]["name"], "Model");
    assert_eq!(summary["response"]["event_count"], 2);

    // A refusal is a JSON error with a stable code and exit status 2.
    let (code, refused) = call(
        &server,
        &format!(
            r#"{{"kind":"append","path":"{path}","actor":{{"kind":"agent","id":"planner"}},"command":{{"command":"add_sheet","name":"Nope"}}}}"#
        ),
    );
    assert_eq!(code, 2);
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"]["code"], "agent_on_main");

    // A request the client cannot even form fails before the socket.
    let (code, invalid) = call(&server, r#"{"kind":"document"}"#);
    assert_eq!(code, 1, "{invalid}");
    assert!(
        invalid["stderr"]
            .as_str()
            .unwrap()
            .contains("missing field `path`")
    );

    // Without the session token the socket answers nothing but a refusal.
    let socket = server.runtime.join("omasheets/native.sock");
    let stream = UnixStream::connect(&socket).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    writeln!(writer, "not-the-token").unwrap();
    writeln!(writer, r#"{{"kind":"document","path":"{path}"}}"#).unwrap();
    let mut answer = String::new();
    reader.read_line(&mut answer).unwrap();
    let answer: serde_json::Value = serde_json::from_str(answer.trim()).unwrap();
    assert_eq!(answer["error"]["code"], "unauthorized");
    let mut more = String::new();
    assert_eq!(
        reader.read_line(&mut more).unwrap(),
        0,
        "connection closes after a bad token"
    );

    // Token and socket are private to the user.
    use std::os::unix::fs::PermissionsExt;
    let token = std::fs::metadata(server.runtime.join("omasheets/native.token")).unwrap();
    assert_eq!(token.permissions().mode() & 0o777, 0o600);
    let directory = std::fs::metadata(server.runtime.join("omasheets")).unwrap();
    assert_eq!(directory.permissions().mode() & 0o777, 0o700);
}
