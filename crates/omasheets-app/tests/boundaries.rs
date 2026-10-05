use omasheets_app::{agent, mcp};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Cursor;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
fn rpc(method: &str, params: Value) -> Value {
    let mut params = params;
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":mcp::PROTOCOL,"io.modelcontextprotocol/clientCapabilities":{}});
    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
}
#[test]
fn agent_surface_cannot_approve_export_or_send_raw_requests() {
    let tools = agent::tools();
    let names: Vec<_> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "native_overview",
            "native_read",
            "native_lineage",
            "native_propose",
            "native_review"
        ]
    );
    for name in [
        "approve",
        "merge",
        "append_batch",
        "export_xlsx",
        "native_approve",
        "call",
    ] {
        assert!(
            agent::call(name, json!({}))
                .unwrap_err()
                .contains("Unknown native tool")
        );
    }
}
#[test]
fn schema_rejects_extra_authority_wrong_types_and_out_of_bounds() {
    let schema = agent::tools()[1]["inputSchema"].clone();
    let good = json!({"session_id":"a".repeat(32),"sheet":"b".repeat(32),"row":0,"column":0,"rows":10,"columns":10});
    agent::validate(&good, &schema).unwrap();
    for (key, bad) in [
        ("path", json!("/tmp/victim")),
        ("actor", json!({"kind":"human"})),
        ("row", json!(-1)),
        ("row", json!(true)),
        ("column", json!(16384)),
        ("rows", json!(0)),
        ("session_id", json!(format!("{}\n", "a".repeat(32)))),
    ] {
        let mut value = good.clone();
        value[key] = bad;
        assert!(agent::validate(&value, &schema).is_err(), "{value}");
    }
}
#[test]
fn edits_preserve_text_formulas_booleans_and_nonfinite_values() {
    for (text, expected) in [
        ("", json!({"command":"clear_cell"})),
        (
            "=SUM(A1:A2)",
            json!({"command":"set_formula","source":"=SUM(A1:A2)"}),
        ),
        (
            "'001",
            json!({"command":"set_value","value":{"type":"text","value":"001"}}),
        ),
        (
            "FALSE",
            json!({"command":"set_value","value":{"type":"boolean","value":false}}),
        ),
        (
            "1.5",
            json!({"command":"set_value","value":{"type":"number","value":1.5}}),
        ),
        (
            "NaN",
            json!({"command":"set_value","value":{"type":"text","value":"NaN"}}),
        ),
    ] {
        let mut expected = expected;
        expected["sheet"] = "s".into();
        expected["a1"] = "A1".into();
        assert_eq!(
            agent::edit(&json!({"sheet":"s","a1":"A1","value":text})),
            expected
        );
    }
}
#[test]
fn mcp_rejects_missing_metadata_and_forbidden_tools() {
    assert_eq!(
        mcp::handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap()["error"]["code"],
        -32602
    );
    assert!(mcp::handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
    let response =
        mcp::handle(rpc("tools/call", json!({"name":"approve","arguments":{}}))).unwrap();
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Unknown native tool")
    );
    assert_eq!(
        mcp::handle(rpc("initialize", json!({}))).unwrap()["result"]["protocolVersion"],
        mcp::PROTOCOL
    );
}
#[test]
fn oversized_mcp_frame_is_drained_before_next_request() {
    let mut input = vec![b'x'; agent::MAX_BYTES + 100];
    input.push(b'\n');
    input.extend_from_slice(rpc("tools/list", json!({})).to_string().as_bytes());
    input.push(b'\n');
    let mut out = Vec::new();
    mcp::serve(&mut Cursor::new(input), &mut out).unwrap();
    let results: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["error"]["code"], -32700);
    assert_eq!(results[1]["result"]["tools"].as_array().unwrap().len(), 5);
}
#[test]
fn private_context_rejects_symlinks_permissions_and_oversize() {
    let dir = std::env::temp_dir().join(format!("omasheets-context-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("context");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(b"private").unwrap();
    assert_eq!(agent::private_file(&path, 7).unwrap(), b"private");
    assert!(agent::private_file(&path, 6).is_err());
    let link = dir.join("link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(agent::private_file(&link, 100).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(agent::private_file(&path, 100).is_err());
    fs::remove_dir_all(dir).unwrap();
}
