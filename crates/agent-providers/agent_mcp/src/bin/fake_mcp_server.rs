//! Test helper for `agent_mcp`'s integration tests: a hand-written stdio MCP
//! server with scripted misbehaviour. Not for production use.
//!
//! Modes (first argument): `ok` (default), `crash-on-call` (exits when a tool
//! is called), `hang-on-call` (never answers a tool call), `die-on-init`.
//! Tools: `echo {text}`, `read_thing {}`, `fail {}` (returns isError).

use std::io::{BufRead, Write};

use serde_json::{json, Value};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "ok".into());
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (Some(method), Some(id)) = (msg.get("method").and_then(Value::as_str), msg.get("id")) else {
            continue;
        };
        let reply = |result: Value| json!({"jsonrpc":"2.0","id":id,"result":result});
        let response = match method {
            "initialize" => {
                if mode == "die-on-init" {
                    std::process::exit(3);
                }
                reply(json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake", "version": "0"}
                }))
            }
            "tools/list" => reply(json!({"tools": [
                {"name": "echo", "description": "Echo text back", "inputSchema": {
                    "type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}},
                {"name": "read_thing", "description": "Pretend to read something", "inputSchema": {"type": "object"}},
                {"name": "fail", "description": "Always reports an error", "inputSchema": {"type": "object"}}
            ]})),
            "tools/call" => {
                match mode.as_str() {
                    "crash-on-call" => std::process::exit(7),
                    "hang-on-call" => continue,
                    _ => {}
                }
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                match params.get("name").and_then(Value::as_str) {
                    Some("echo") => reply(json!({"content": [{
                        "type": "text",
                        "text": params["arguments"]["text"].as_str().unwrap_or("")
                    }]})),
                    Some("fail") => reply(json!({"isError": true, "content": [{"type":"text","text":"it failed"}]})),
                    _ => reply(json!({"content": [{"type":"text","text":"thing"}]})),
                }
            }
            _ => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"nope"}}),
        };
        let _ = writeln!(out, "{response}");
        let _ = out.flush();
    }
}
