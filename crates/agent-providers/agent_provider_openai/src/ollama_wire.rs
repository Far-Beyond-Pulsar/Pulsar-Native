//! The Ollama wire format: request payload, reply parsing and the newline-delimited stream reader.

use agent_chat_core::{ChatRequest, ChatResponse, ToolCall};
use anyhow::anyhow;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

use crate::roles::map_role;

pub(crate) fn build_ollama_payload(request: &ChatRequest, stream: bool) -> Value {
    let messages: Vec<Value> = request.messages.iter().map(|msg| {
        let mut m = serde_json::Map::new();
        m.insert("role".into(), json!(map_role(msg.role)));
        m.insert("content".into(), json!(msg.content));
        if !msg.tool_calls.is_empty() {
            m.insert("tool_calls".into(), json!(msg.tool_calls.iter().map(|c| json!({ "function": { "name": c.name, "arguments": c.arguments_json } })).collect::<Vec<_>>()));
        }
        Value::Object(m)
    }).collect();

    let mut payload = json!({ "model": request.model, "messages": messages, "stream": stream });
    let mut opts = serde_json::Map::new();
    if let Some(t) = request.temperature {
        opts.insert("temperature".into(), json!(t));
    }
    if let Some(p) = request.top_p {
        opts.insert("top_p".into(), json!(p));
    }
    if let Some(m) = request.max_tokens {
        opts.insert("num_predict".into(), json!(m));
    }
    if !opts.is_empty() {
        payload["options"] = Value::Object(opts);
    }
    if request.enable_tool_calls && !request.tools.is_empty() {
        payload["tools"] = json!(request.tools.iter().map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.parameters_json_schema } })).collect::<Vec<_>>());
    }
    payload
}

fn parse_ollama_tc(message: Option<&Value>, next: &mut usize) -> Vec<ToolCall> {
    let Some(calls) = message
        .and_then(|m| m.get("tool_calls"))
        .and_then(|v| v.as_array())
    else {
        return vec![];
    };
    calls
        .iter()
        .filter_map(|call| {
            let func = call.get("function")?;
            let name = func.get("name")?.as_str()?.to_string();
            let args = match func.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
                Some(v) => v.clone(),
                None => json!({}),
            };
            let id = call
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    let i = *next;
                    *next += 1;
                    format!("ollama_{i}")
                });
            Some(ToolCall {
                id,
                name,
                arguments_json: args,
            })
        })
        .collect()
}

pub(crate) fn parse_ollama_response(raw: Value) -> anyhow::Result<ChatResponse> {
    let msg = raw
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let mut n = 1;
    let tcs = parse_ollama_tc(raw.get("message"), &mut n);
    let chunks = msg
        .as_ref()
        .map(|t| {
            t.chars()
                .collect::<Vec<_>>()
                .chunks(20)
                .map(|c| c.iter().collect())
                .collect()
        })
        .unwrap_or_default();
    let fr = raw
        .get("done_reason")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            raw.get("done").and_then(|v| v.as_bool()).and_then(|d| {
                if d {
                    Some("stop".into())
                } else {
                    None
                }
            })
        });
    Ok(ChatResponse {
        assistant_message: msg,
        streamed_text_chunks: chunks,
        tool_calls: tcs,
        finish_reason: fr,
        raw_response: raw,
    })
}

pub(crate) fn read_ollama_stream(
    response: reqwest::blocking::Response,
    on_chunk: &mut dyn FnMut(String),
) -> anyhow::Result<ChatResponse> {
    let mut events = Vec::new();
    let mut chunks = Vec::new();
    let mut full = String::new();
    let mut fr = None;
    let mut tcs = Vec::new();
    let mut n = 1;
    for line in BufReader::new(response).lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let ev: Value = serde_json::from_str(line)?;
        if let Some(e) = ev.get("error").and_then(|v| v.as_str()) {
            return Err(anyhow!("ollama error: {e}"));
        }
        if let Some(msg) = ev.get("message") {
            if let Some(c) = msg.get("content").and_then(|v| v.as_str()) {
                if !c.is_empty() {
                    let ch = c.to_string();
                    full.push_str(&ch);
                    chunks.push(ch.clone());
                    on_chunk(ch);
                }
            }
            tcs.append(&mut parse_ollama_tc(Some(msg), &mut n));
        }
        if fr.is_none() {
            fr = ev
                .get("done_reason")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
        if fr.is_none() && ev.get("done").and_then(|v| v.as_bool()).unwrap_or(false) {
            fr = Some("stop".into());
        }
        let done = ev.get("done").and_then(|v| v.as_bool()).unwrap_or(false);
        events.push(ev);
        if done {
            break;
        }
    }
    Ok(ChatResponse {
        assistant_message: if full.is_empty() { None } else { Some(full) },
        streamed_text_chunks: chunks,
        tool_calls: tcs,
        finish_reason: fr,
        raw_response: Value::Array(events),
    })
}
