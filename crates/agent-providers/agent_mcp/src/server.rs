//! Expose the engine's agent tools as an MCP server over stdio so external
//! agents (Claude Code, Cursor, ...) can drive them.
//!
//! JSON-RPC subset: `initialize`, `ping`, `tools/list`, `tools/call`
//! (notifications are accepted and ignored). Framing is newline-delimited
//! JSON, as the MCP stdio transport specifies; the process's stdout carries
//! protocol messages only.
//!
//! Which tools are exposed is decided with the same [`ToolClassifier`] the
//! agent runtime uses: read-class tools only, unless `allow_write` is set.
//! Tools can be added individually with `extra_allowed`. Tools that are
//! themselves MCP proxies (`mcp__*`) are never exposed, so two editors cannot
//! be chained into a loop.

use std::{
    collections::BTreeSet,
    io::{BufRead, Write},
    panic::{catch_unwind, AssertUnwindSafe},
};

use agent_chat_tools::{ToolContext, ToolRegistry};
use agent_runtime::{is_mcp_tool, ToolClass, ToolClassifier};
use serde_json::{json, Value};

use crate::client::PROTOCOL_VERSION;

#[derive(Clone, Debug)]
pub struct ServerOptions {
    pub server_name: String,
    /// Expose write-class tools too. Off by default.
    pub allow_write: bool,
    /// Tools exposed regardless of class (e.g. a search tool you trust).
    pub extra_allowed: BTreeSet<String>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            server_name: "pulsar-engine".into(),
            allow_write: false,
            extra_allowed: BTreeSet::new(),
        }
    }
}

/// Names of the tools this server would expose, sorted.
pub fn exposed_tools(
    registry: &ToolRegistry,
    classifier: &ToolClassifier,
    opts: &ServerOptions,
) -> Vec<String> {
    registry
        .names()
        .into_iter()
        .filter(|name| !is_mcp_tool(name))
        .filter(|name| {
            opts.allow_write
                || opts.extra_allowed.contains(*name)
                || classifier.classify(name) == ToolClass::Read
        })
        .map(str::to_string)
        .collect()
}

/// Serve until `input` reaches EOF.
pub fn serve<R: BufRead, W: Write>(
    registry: &ToolRegistry,
    ctx: &ToolContext,
    classifier: &ToolClassifier,
    opts: &ServerOptions,
    input: R,
    mut output: W,
) -> std::io::Result<()> {
    let exposed: BTreeSet<String> = exposed_tools(registry, classifier, opts).into_iter().collect();
    for line in input.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Err(_) => Some(error_reply(Value::Null, -32700, "parse error")),
            Ok(msg) => handle(&msg, registry, ctx, opts, &exposed),
        };
        if let Some(reply) = reply {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
    Ok(())
}

fn error_reply(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn handle(
    msg: &Value,
    registry: &ToolRegistry,
    ctx: &ToolContext,
    opts: &ServerOptions,
    exposed: &BTreeSet<String>,
) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str)?;
    // Notifications carry no id and get no reply.
    let id = msg.get("id")?.clone();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": opts.server_name, "version": env!("CARGO_PKG_VERSION")},
                "instructions": if opts.allow_write {
                    "Pulsar engine agent tools (read and write)."
                } else {
                    "Pulsar engine agent tools (read-only subset)."
                }
            })
        }
        "ping" => json!({}),
        "tools/list" => {
            let tools: Vec<Value> = registry
                .definitions()
                .into_iter()
                .filter(|d| exposed.contains(&d.name))
                .map(|d| {
                    json!({
                        "name": d.name,
                        "description": d.description,
                        "inputSchema": d.parameters_schema,
                    })
                })
                .collect();
            json!({"tools": tools})
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error_reply(id, -32602, "tools/call needs a tool name"));
            };
            if !exposed.contains(name) {
                return Some(error_reply(
                    id,
                    -32602,
                    &format!("unknown or unavailable tool: {name}"),
                ));
            }
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let outcome = catch_unwind(AssertUnwindSafe(|| registry.execute(name, args, ctx)))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("tool panicked")));
            match outcome {
                Ok(value) => {
                    let text = match &value {
                        Value::String(s) => s.clone(),
                        other => serde_json::to_string_pretty(other).unwrap_or_default(),
                    };
                    let mut result = json!({
                        "content": [{"type": "text", "text": text}],
                        "isError": false
                    });
                    if value.is_object() {
                        result["structuredContent"] = value;
                    }
                    result
                }
                Err(err) => json!({
                    "content": [{"type": "text", "text": err.to_string()}],
                    "isError": true
                }),
            }
        }
        other => {
            return Some(error_reply(id, -32601, &format!("method not found: {other}")));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}
