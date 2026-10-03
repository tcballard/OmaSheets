//! Native MCP transport; same metadata contract as the existing bridge.
use crate::{Result, agent};
use serde_json::{Value, json};
use std::io::{BufRead, Read, Write};
pub const PROTOCOL: &str = "2026-07-28";
fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
pub fn handle(request: Value) -> Option<Value> {
    if !request.is_object() || request["jsonrpc"] != "2.0" {
        return Some(error(Value::Null, -32600, "Invalid Request"));
    }
    let id = request["id"].clone();
    if id.is_null() {
        return None;
    }
    if !(id.is_string() || id.is_number()) {
        return Some(error(Value::Null, -32600, "Invalid Request id"));
    }
    let params = &request["params"];
    if params["_meta"]["io.modelcontextprotocol/protocolVersion"] != PROTOCOL
        || !params["_meta"]["io.modelcontextprotocol/clientCapabilities"].is_object()
    {
        return Some(error(
            id,
            -32602,
            "Missing or unsupported protocol metadata",
        ));
    }
    let info = json!({"name":"omasheets","version":crate::VERSION});
    let outcome: Result<Value> = (|| match request["method"].as_str() {
        Some("initialize") => Ok(
            json!({"protocolVersion":PROTOCOL,"capabilities":{"tools":{},"resources":{}},"serverInfo":info}),
        ),
        Some("ping") => Ok(json!({})),
        Some("tools/list") => Ok(json!({"tools":agent::tools()})),
        Some("tools/call") => {
            if params.as_object().is_none_or(|o| {
                o.keys()
                    .any(|k| !matches!(k.as_str(), "name" | "arguments" | "_meta"))
            }) {
                return Err("Unknown tool call field".into());
            }
            let result = agent::call(
                params["name"].as_str().ok_or("Missing tool name")?,
                params["arguments"].clone(),
            )?;
            Ok(
                json!({"content":[{"type":"text","text":result.to_string()}],"structuredContent":result,"isError":false}),
            )
        }
        Some("resources/list") => Ok(
            json!({"resources":[{"uri":"omasheets://session","name":"Current native agent session","mimeType":"application/json"}]}),
        ),
        Some("resources/read") => {
            if params["uri"] != "omasheets://session"
                || params
                    .as_object()
                    .is_none_or(|o| o.keys().any(|k| !matches!(k.as_str(), "uri" | "_meta")))
            {
                return Err("Unknown native resource".into());
            }
            Ok(
                json!({"contents":[{"uri":"omasheets://session","mimeType":"application/json","text":agent::resource()?.to_string()}]}),
            )
        }
        _ => Err("Method not found".into()),
    })();
    Some(match outcome {
        Ok(mut result) => {
            result["resultType"] = "complete".into();
            result["_meta"] = json!({"io.modelcontextprotocol/serverInfo":info});
            json!({"jsonrpc":"2.0","id":id,"result":result})
        }
        Err(message) => error(
            id,
            if message == "Method not found" {
                -32601
            } else {
                -32602
            },
            &message,
        ),
    })
}
pub fn serve(input: &mut impl BufRead, output: &mut impl Write) -> Result<()> {
    loop {
        let mut line = Vec::new();
        input
            .take((agent::MAX_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if line.is_empty() {
            return Ok(());
        }
        let response = if line.len() > agent::MAX_BYTES {
            while line.last() != Some(&b'\n') {
                line.clear();
                if input
                    .take(8192)
                    .read_until(b'\n', &mut line)
                    .map_err(|e| e.to_string())?
                    == 0
                {
                    break;
                }
            }
            Some(error(Value::Null, -32700, "Message too large"))
        } else {
            match serde_json::from_slice(&line) {
                Ok(v) => handle(v),
                Err(_) => Some(error(Value::Null, -32700, "Parse error")),
            }
        };
        if let Some(response) = response {
            writeln!(output, "{response}")
                .and_then(|_| output.flush())
                .map_err(|e| e.to_string())?;
        }
    }
}
