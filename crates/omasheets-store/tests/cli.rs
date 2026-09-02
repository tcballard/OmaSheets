use omasheets_core::{
    Actor, ActorKind, Command, DocumentId, Literal, ObjectId, Operation, Severity,
};
use omasheets_store::Store;
use std::path::PathBuf;
use std::process::Command as Process;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "omasheets-cli-{}-{nonce}.omasheets",
        std::process::id()
    ))
}

fn cli(arguments: &[&str]) -> (i32, String, String) {
    let output = Process::new(env!("CARGO_BIN_EXE_omasheets-store"))
        .args(arguments)
        .output()
        .expect("binary runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn cli_drives_replay_check_branch_diff_and_merge() {
    let path = temp_path();
    let file = path.to_str().unwrap();
    {
        let human = Actor::new(ActorKind::Human, "tom");
        let mut store = Store::create(
            &path,
            DocumentId(ObjectId::from_seed("cli")),
            "Plan",
            human.clone(),
            1,
        )
        .unwrap();
        let main = store.branch_id("main").unwrap();
        let event = store
            .append(
                main,
                human.clone(),
                2,
                Command::AddSheet {
                    name: "Model".into(),
                },
            )
            .unwrap();
        let Operation::AddSheet { sheet, .. } = event.operation else {
            unreachable!()
        };
        store
            .append(
                main,
                human.clone(),
                3,
                Command::AddColumns {
                    sheet,
                    count: 1,
                    at: 0,
                },
            )
            .unwrap();
        store
            .append(
                main,
                human.clone(),
                4,
                Command::AddRows {
                    sheet,
                    count: 3,
                    at: 0,
                    table: None,
                },
            )
            .unwrap();
        store
            .append(
                main,
                human.clone(),
                5,
                Command::SetValue {
                    sheet,
                    a1: "A1".into(),
                    value: Literal::Number(4.0),
                },
            )
            .unwrap();
        store
            .append(
                main,
                human.clone(),
                6,
                Command::SetFormula {
                    sheet,
                    a1: "A2".into(),
                    source: "=A1<10".into(),
                },
            )
            .unwrap();
        store
            .append(
                main,
                human.clone(),
                7,
                Command::AddCheck {
                    name: "small".into(),
                    sheet,
                    a1: "A2".into(),
                    severity: Severity::Error,
                    message: "A1 must stay below 10".into(),
                },
            )
            .unwrap();
        store.close().unwrap();
    }

    // close() wrote a snapshot, so the first replay loads snapshot plus tail.
    let (code, stdout, _) = cli(&["replay", file]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("\"path\": \"snapshot_plus_tail\""));
    assert!(stdout.contains("\"events_replayed\": 0"));
    assert!(stdout.contains("\"event_count\": 7"));

    let (code, stdout, _) = cli(&["check", file]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("\"passed\": true"));

    let (code, stdout, _) = cli(&[
        "branch",
        file,
        "agent-work",
        "--from",
        "main",
        "--actor",
        "tom",
    ]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("\"branch\": \"agent-work\""));

    // An agent edit on the branch through the library breaks the check.
    {
        let mut store = Store::open(&path).unwrap();
        let branch = store.branch_id("agent-work").unwrap();
        let sheet = store.document(branch).unwrap().sheets()[0];
        store
            .append(
                branch,
                Actor::new(ActorKind::Agent, "planner"),
                8,
                Command::SetValue {
                    sheet,
                    a1: "A1".into(),
                    value: Literal::Number(50.0),
                },
            )
            .unwrap();
        store.close().unwrap();
    }
    let (code, stdout, _) = cli(&["check", file, "--branch", "agent-work"]);
    assert_eq!(code, 2, "{stdout}");
    assert!(stdout.contains("\"passed\": false"));

    let (code, stdout, _) = cli(&["diff", file, "agent-work"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("\"operation\": \"set_value\""));
    assert!(stdout.contains("\"conflicts\": []"));

    let (code, stdout, stderr) = cli(&["merge", file, "agent-work", "--approved-by", "tom"]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stderr.contains("error-severity checks failed"));

    // Fix on the branch, then the human-approved merge lands.
    {
        let mut store = Store::open(&path).unwrap();
        let branch = store.branch_id("agent-work").unwrap();
        let sheet = store.document(branch).unwrap().sheets()[0];
        store
            .append(
                branch,
                Actor::new(ActorKind::Agent, "planner"),
                9,
                Command::SetValue {
                    sheet,
                    a1: "A1".into(),
                    value: Literal::Number(6.0),
                },
            )
            .unwrap();
        store.close().unwrap();
    }
    let (code, stdout, stderr) = cli(&["merge", file, "agent-work", "--approved-by", "tom"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("\"replayed\""));
    let (code, stdout, _) = cli(&["replay", file]);
    assert_eq!(code, 0);
    assert!(stdout.contains("\"path\": \"snapshot_plus_tail\""));

    let (code, _, stderr) = cli(&["merge", file, "agent-work", "--approved-by", ""]);
    assert_eq!(code, 1);
    assert!(stderr.contains("actor names"));
    let (code, _, stderr) = cli(&["bogus", file]);
    assert_eq!(code, 1);
    assert!(stderr.contains("usage"));
    let (code, _, stderr) = cli(&["replay", "/nonexistent/file.omasheets"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("not an OmaSheets store"));

    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{file}{suffix}"));
    }
}
