//! The JSON-RPC side of MCP: `initialize`, `ping`, `tools/list`, `tools/call`
//! and the notifications a client sends. Transport-free: [`handle`] takes a
//! request body and returns the response body, so the same code answers HTTP
//! and the tests.
//!
//! The server is stateless: it keeps no session, offers no server-initiated
//! stream and no `listChanged` notification (a client asks `tools/list` again),
//! which the Streamable HTTP transport allows. What a client may want to watch
//! (the engine's log) is a cursor-based tool, `logs_wait`, that works with every
//! client.

use serde_json::{Value, json};

use crate::catalog;
use crate::tool::ToolResult;

/// Protocol revisions the server speaks, newest first.
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// What the server tells a client about itself and how to use it.
pub const INSTRUCTIONS: &str = "The Pulsar engine's editor. Tools are grouped by prefix: \
`build_*` (configurations, start, stop, status, output), `logs_*` (query the engine log, wait \
for new lines), `tasks_*`, `problems_list`, `plugins_*`, `viewport_*` (camera, screenshot) and \
`level_editor_*` (the open level: objects, components, selection, undo, save, play). A call \
that fails returns isError with what to do instead. Tools marked destructive can delete or \
overwrite a person's work: confirm with the person first.";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// What to send back for a request body.
#[derive(Debug, PartialEq)]
pub enum Reply {
    /// A notification or a response: nothing to say (HTTP 202).
    None,
    /// A JSON body (HTTP 200).
    Json(String),
}

fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Answer a request body: one JSON-RPC message or a batch of them.
pub fn handle(body: &str) -> Reply {
    let value: Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(parse) => {
            return Reply::Json(error(Value::Null, PARSE_ERROR, format!("not JSON: {parse}")).to_string());
        }
    };
    match value {
        Value::Array(batch) if batch.is_empty() => {
            Reply::Json(error(Value::Null, INVALID_REQUEST, "an empty batch").to_string())
        }
        Value::Array(batch) => {
            let answers: Vec<Value> = batch.iter().filter_map(handle_one).collect();
            if answers.is_empty() {
                Reply::None
            } else {
                Reply::Json(Value::Array(answers).to_string())
            }
        }
        single => handle_one(&single).map_or(Reply::None, |answer| Reply::Json(answer.to_string())),
    }
}

/// One message; `None` for a notification or a client's response.
fn handle_one(message: &Value) -> Option<Value> {
    let Some(object) = message.as_object() else {
        return Some(error(Value::Null, INVALID_REQUEST, "a message is a JSON object"));
    };
    let method = object.get("method").and_then(Value::as_str);
    let id = object.get("id").cloned();
    let Some(method) = method else {
        // A response to a request the server never sent: ignore it.
        return if id.is_some() && (object.contains_key("result") || object.contains_key("error")) {
            None
        } else {
            Some(error(id.unwrap_or(Value::Null), INVALID_REQUEST, "no `method`"))
        };
    };
    // No id: a notification, which gets no answer whatever it is.
    let id = id?;
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => ok(id, initialize(&params)),
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, json!({ "tools": catalog::list().iter().map(|t| t.to_json()).collect::<Vec<_>>() })),
        "tools/call" => match call_tool(&params) {
            Ok(result) => ok(id, result.to_json()),
            Err(message) => error(id, INVALID_PARAMS, message),
        },
        other => error(id, METHOD_NOT_FOUND, format!("unknown method `{other}`")),
    })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|version| SUPPORTED_VERSIONS.contains(version))
        .unwrap_or(SUPPORTED_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "pulsar-engine",
            "title": "Pulsar Engine",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

/// `tools/call`: `Err` is a protocol error (unknown tool, malformed
/// arguments); a tool's own failure is an `isError` result.
fn call_tool(params: &Value) -> Result<ToolResult, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("`tools/call` needs a `name`")?;
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(value) if value.is_object() => value.clone(),
        Some(_) => return Err("`arguments` must be a JSON object".into()),
    };
    let tool = catalog::list()
        .into_iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| format!("unknown tool `{name}`; call tools/list for the tools there are"))?;
    if let Some(missing) = missing_required(&tool.input_schema, &arguments) {
        return Ok(ToolResult::error(format!(
            "`{name}` needs the argument `{missing}`: {}",
            tool.input_schema["properties"][&missing]["description"]
                .as_str()
                .unwrap_or("see its input schema in tools/list")
        )));
    }
    tracing::debug!(tool = name, "mcp tool call");
    Ok(catalog::call(name, &arguments.to_string())
        .unwrap_or_else(|| ToolResult::error(format!("the tool `{name}` is no longer available"))))
}

/// The first `required` property the arguments lack.
fn missing_required(schema: &Value, arguments: &Value) -> Option<String> {
    schema
        .get("required")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find(|key| arguments.get(*key).is_none_or(Value::is_null))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(body: &str) -> Value {
        match handle(body) {
            Reply::Json(text) => serde_json::from_str(&text).unwrap(),
            Reply::None => Value::Null,
        }
    }

    #[test]
    fn initialize_negotiates_a_version_and_lists_tools_capability() {
        let answer = reply(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
        );
        assert_eq!(answer["id"], 1);
        assert_eq!(answer["result"]["protocolVersion"], "2025-03-26");
        assert!(answer["result"]["capabilities"]["tools"].is_object());
        // An unknown revision is answered with the newest the server speaks.
        let answer = reply(r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#);
        assert_eq!(answer["result"]["protocolVersion"], SUPPORTED_VERSIONS[0]);
        assert_eq!(answer["id"], "a");
    }

    #[test]
    fn notifications_and_responses_get_no_answer() {
        assert_eq!(handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), Reply::None);
        assert_eq!(handle(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#), Reply::None);
    }

    #[test]
    fn malformed_messages_are_protocol_errors() {
        assert_eq!(reply("not json")["error"]["code"], PARSE_ERROR);
        assert_eq!(reply("[]")["error"]["code"], INVALID_REQUEST);
        assert_eq!(reply(r#"{"jsonrpc":"2.0","id":1,"method":"nope"}"#)["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(
            reply(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"no_such_tool"}}"#)["error"]["code"],
            INVALID_PARAMS
        );
        assert_eq!(
            reply(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"x","arguments":[1]}}"#)["error"]["code"],
            INVALID_PARAMS
        );
    }

    #[test]
    fn a_batch_answers_each_request_and_skips_notifications() {
        let answer = reply(
            r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":2,"method":"ping"}]"#,
        );
        let answers = answer.as_array().unwrap();
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[1]["id"], 2);
    }

    #[test]
    fn missing_required_names_the_first_absent_argument() {
        let schema = json!({"type":"object","properties":{"a":{"description":"x"}},"required":["a","b"]});
        assert_eq!(missing_required(&schema, &json!({"a": 1})), Some("b".into()));
        assert_eq!(missing_required(&schema, &json!({"a": 1, "b": null})), Some("b".into()));
        assert_eq!(missing_required(&schema, &json!({"a": 1, "b": 2})), None);
    }
}
