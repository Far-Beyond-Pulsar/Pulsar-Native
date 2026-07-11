use agent_chat_core::{
    ChatMessage, ChatProvider, ChatRequest, ChatResponse, ChatRole,
    ConfigField, ModelDescriptor, ProviderConfig, ProviderCrate, ProviderEntry,
    ProviderKind, ToolCall,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

fn models_url(base: &str) -> String {
    format!("{}/models", base.trim_end_matches('/'))
}

struct Entry {
    id: &'static str,
    display_name: &'static str,
    endpoint: &'static str, // base URL without path
}

const ENTRIES: &[Entry] = &[
    Entry { id: "opencode_go", display_name: "OpenCode Go", endpoint: "https://opencode.ai/go/v1" },
    Entry { id: "opencode_zen", display_name: "OpenCode Zen", endpoint: "https://opencode.ai/zen/v1" },
];

/// Pick the correct API endpoint for a given model ID.
fn endpoint_for_model(base: &str, model: &str) -> String {
    let m = model.to_lowercase();
    if m.starts_with("gpt-") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        format!("{}/responses", base.trim_end_matches('/'))
    } else if m.starts_with("claude-") || m.starts_with("sonnet") || m.starts_with("opus")
        || m.starts_with("haiku") || m.starts_with("fable") || m.starts_with("qwen")
    {
        format!("{}/messages", base.trim_end_matches('/'))
    } else {
        format!("{}/chat/completions", base.trim_end_matches('/'))
    }
}

pub struct OpenCodeProviderCrate;

impl ProviderCrate for OpenCodeProviderCrate {
    fn entries(&self) -> Vec<ProviderEntry> {
        ENTRIES.iter().map(|e| {
            ProviderEntry {
                id: e.id,
                display_name: e.display_name,
                kind: ProviderKind::Cloud,
                default_endpoint: Some(e.endpoint),
                config_fields: vec![
                    ConfigField {
                        key: "api_key",
                        label: "API Key",
                        description: format!("Your {} API key (get it at https://opencode.ai/auth)", e.display_name).leak(),
                        sensitive: true,
                        required: true,
                        placeholder: None,
                    },
                ],
            }
        }).collect()
    }

    fn create(&self, id: &str, config: ProviderConfig) -> anyhow::Result<Box<dyn ChatProvider>> {
        let entry = ENTRIES.iter().find(|e| e.id == id)
            .ok_or_else(|| anyhow!("unknown provider: {id}"))?;
        let api_key = config.get("api_key").unwrap_or_default().to_string();
        Ok(Box::new(OpenCodeChatProvider {
            client: Client::new(),
            id: id.to_string(),
            display_name: entry.display_name.to_string(),
            base_endpoint: entry.endpoint.to_string(),
            api_key,
        }))
    }
}

struct OpenCodeChatProvider {
    client: Client,
    id: String,
    display_name: String,
    base_endpoint: String,
    api_key: String,
}

impl ChatProvider for OpenCodeChatProvider {
    fn id(&self) -> &str { &self.id }
    fn display_name(&self) -> &str { &self.display_name }

    fn validate_config(&self) -> anyhow::Result<()> {
        if self.api_key.is_empty() {
            return Err(anyhow::anyhow!("API key is required"));
        }
        let murl = models_url(&self.base_endpoint);
        let mut req = self.client.get(&murl)
            .header("Content-Type", "application/json");
        req = req.header("Authorization", format!("Bearer {}", self.api_key));
        match req.send() {
            Ok(resp) => match resp.status().as_u16() {
                401 | 403 => Err(anyhow::anyhow!("API key is invalid or expired")),
                _ => Ok(()),
            },
            Err(e) => {
                tracing::warn!(error = %e, "validate_config connection failed, assuming valid");
                Ok(())
            }
        }
    }

    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        let murl = models_url(&self.base_endpoint);
        let mut req = self.client.get(&murl)
            .header("Content-Type", "application/json");
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let resp = match req.send() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "models fetch failed");
                return Ok(vec![]);
            }
        };
        if !resp.status().is_success() {
            tracing::warn!(status = %resp.status(), "models API error");
            return Ok(vec![]);
        }
        let text = match resp.text() {
            Ok(t) => t,
            Err(_) => return Ok(vec![]),
        };
        let body: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, response = %text, "models JSON parse failed");
                return Ok(vec![]);
            }
        };
        let arr: Vec<Value> = match body {
            Value::Array(ref a) => a.clone(),
            Value::Object(_) => body.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default(),
            _ => vec![],
        };
        Ok(arr.iter().filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            let label = m.get("name").or_else(|| m.get("id")).and_then(|v| v.as_str()).unwrap_or(&id).to_string();
            Some(ModelDescriptor { id, label, supports_tools: true, context_tokens: 0, compact_model: None })
        }).collect())
    }

    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let url = endpoint_for_model(&self.base_endpoint, &request.model);
        if url.contains("/messages") {
            let payload = build_anthropic_payload(&request, false);
            let body = self.send(&url, &payload)?;
            parse_anthropic_response(body)
        } else if url.contains("/responses") {
            let payload = build_openai_payload(&request, false)?;
            let body = self.send(&url, &payload)?;
            parse_non_stream_response(body)
        } else {
            let payload = build_openai_payload(&request, false)?;
            let body = self.send(&url, &payload)?;
            parse_non_stream_response(body)
        }
    }

    fn chat_streaming(&self, request: ChatRequest, on_chunk: &mut dyn FnMut(String)) -> anyhow::Result<ChatResponse> {
        let url = endpoint_for_model(&self.base_endpoint, &request.model);
        let payload = if url.contains("/messages") {
            build_anthropic_payload(&request, true)
        } else {
            build_openai_payload(&request, true)?
        };
        let accept = if url.contains("/messages") { "application/json" } else { "text/event-stream" };
        let mut req = self.client.post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", accept)
            .json(&payload);
        if !self.api_key.is_empty() {
            if url.contains("/messages") {
                req = req.header("x-api-key", &self.api_key);
                req = req.header("anthropic-version", "2023-06-01");
            } else {
                req = req.header("Authorization", format!("Bearer {}", self.api_key));
            }
        }
        let response = req.send().with_context(|| format!("chat {}", url))?;
        if !response.status().is_success() {
            return Err(anyhow!("chat API {}: {}", response.status(), response.text().unwrap_or_default()));
        }
        if url.contains("/messages") {
            read_anthropic_stream(response, on_chunk)
        } else {
            read_stream_response(response, on_chunk)
        }
    }
}

impl OpenCodeChatProvider {
    fn send(&self, url: &str, payload: &Value) -> anyhow::Result<Value> {
        let mut req = self.client.post(url)
            .header("Content-Type", "application/json")
            .json(payload);
        if !self.api_key.is_empty() {
            if url.contains("/messages") {
                req = req.header("x-api-key", &self.api_key);
                req = req.header("anthropic-version", "2023-06-01");
            } else {
                req = req.header("Authorization", format!("Bearer {}", self.api_key));
            }
        }
        let resp = req.send().with_context(|| format!("chat {}", url))?;
        if !resp.status().is_success() {
            return Err(anyhow!("chat API {}: {}", resp.status(), resp.text().unwrap_or_default()));
        }
        Ok(resp.json()?)
    }
}

// ── Shared request/response logic (same as agent_provider_openai) ────────────

fn build_openai_payload(request: &ChatRequest, stream: bool) -> anyhow::Result<Value> {
    let messages: Vec<Value> = request.messages.iter().map(|msg| {
        let mut m = json!({ "role": map_role(msg.role), "content": msg.content });
        if let Some(tid) = &msg.tool_call_id { m["tool_call_id"] = json!(tid); }
        if !msg.tool_calls.is_empty() {
            m["tool_calls"] = json!(msg.tool_calls.iter().map(|c| json!({
                "id": c.id, "type": "function",
                "function": { "name": c.name, "arguments": c.arguments_json.to_string() }
            })).collect::<Vec<_>>());
        }
        m
    }).collect();

    let tools: Vec<Value> = if request.enable_tool_calls {
        request.tools.iter().map(|t| json!({
            "type": "function",
            "function": { "name": t.name, "description": t.description, "parameters": t.parameters_json_schema }
        })).collect()
    } else { vec![] };

    let mut payload = json!({ "model": request.model, "messages": messages, "stream": stream });
    if let Some(t) = request.temperature { payload["temperature"] = json!(t); }
    if let Some(p) = request.top_p { payload["top_p"] = json!(p); }
    if let Some(m) = request.max_tokens { payload["max_tokens"] = json!(m); }
    if !tools.is_empty() { payload["tools"] = Value::Array(tools); }
    Ok(payload)
}

fn parse_assistant_message(raw: &Value) -> Option<String> {
    raw.get("choices")?.as_array()?.first()?.get("message")?.get("content")?.as_str().map(|s| s.to_string())
}

fn parse_tool_calls(raw: &Value) -> Vec<ToolCall> {
    raw.get("choices").and_then(|c| c.as_array()).and_then(|c| c.first())
        .and_then(|c| c.get("message")).and_then(|m| m.get("tool_calls")).and_then(|t| t.as_array())
        .map(|calls| calls.iter().filter_map(|call| {
            let id = call.get("id")?.as_str()?.to_string();
            let func = call.get("function")?;
            let name = func.get("name")?.as_str()?.to_string();
            let args = match func.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.clone())),
                Some(v) => v.clone(), None => json!({}),
            };
            Some(ToolCall { id, name, arguments_json: args })
        }).collect()).unwrap_or_default()
}

fn parse_non_stream_response(raw: Value) -> anyhow::Result<ChatResponse> {
    let msg = parse_assistant_message(&raw);
    let tcs = parse_tool_calls(&raw);
    let chunks = msg.as_ref().map(|t| t.chars().collect::<Vec<_>>().chunks(20).map(|c| c.iter().collect()).collect()).unwrap_or_default();
    let fr = raw.get("choices").and_then(|c| c.as_array()).and_then(|c| c.first()).and_then(|c| c.get("finish_reason")).and_then(|v| v.as_str()).map(|s| s.to_string());
    Ok(ChatResponse { assistant_message: msg, streamed_text_chunks: chunks, tool_calls: tcs, finish_reason: fr, raw_response: raw })
}

fn parse_stream_tool_calls(events: &[Value]) -> Vec<ToolCall> {
    #[derive(Default)] struct P { id: Option<String>, name: Option<String>, args: String }
    let mut ps: Vec<P> = Vec::new();
    for event in events {
        if let Some(choice) = event.get("choices").and_then(|c| c.as_array()).and_then(|c| c.first()) {
            if let Some(delta) = choice.get("delta") {
                if let Some(tc_arr) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tc_arr {
                        let idx = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(ps.len() as u64) as usize;
                        while ps.len() <= idx { ps.push(P::default()); }
                        if let Some(id) = tc.get("id").and_then(|v| v.as_str()) { ps[idx].id = Some(id.to_string()); }
                        if let Some(func) = tc.get("function") {
                            if let Some(n) = func.get("name").and_then(|v| v.as_str()) { ps[idx].name = Some(n.to_string()); }
                            if let Some(a) = func.get("arguments").and_then(|v| v.as_str()) { ps[idx].args.push_str(a); }
                        }
                    }
                }
            }
        }
    }
    ps.into_iter().filter_map(|p| {
        let id = p.id?; let name = p.name?;
        let args = if p.args.trim().is_empty() { json!({}) } else { serde_json::from_str(&p.args).unwrap_or_else(|_| Value::String(p.args)) };
        Some(ToolCall { id, name, arguments_json: args })
    }).collect()
}

fn read_stream_response(response: reqwest::blocking::Response, on_chunk: &mut dyn FnMut(String)) -> anyhow::Result<ChatResponse> {
    let mut events = Vec::new();
    let mut chunks = Vec::new();
    let mut full = String::new();
    let mut fr = None;
    for line in BufReader::new(response).lines() {
        let line = line?; let line = line.trim();
        if line.is_empty() { continue; }
        let Some(data) = line.strip_prefix("data:") else { continue; };
        let data = data.trim();
        if data == "[DONE]" { break; }
        let ev: Value = serde_json::from_str(data)?;
        if let Some(choice) = ev.get("choices").and_then(|c| c.as_array()).and_then(|c| c.first()) {
            if fr.is_none() { fr = choice.get("finish_reason").and_then(|v| v.as_str()).map(|s| s.to_string()); }
            if let Some(delta) = choice.get("delta") {
                if let Some(c) = delta.get("content").and_then(|v| v.as_str()) {
                    if !c.is_empty() { let ch = c.to_string(); full.push_str(&ch); chunks.push(ch.clone()); on_chunk(ch); }
                } else if let Some(parts) = delta.get("content").and_then(|v| v.as_array()) {
                    for part in parts { if let Some(t) = part.get("text").and_then(|v| v.as_str()) { if !t.is_empty() { let ch = t.to_string(); full.push_str(&ch); chunks.push(ch.clone()); on_chunk(ch); } } }
                }
            }
        }
        events.push(ev);
        if fr.is_some() { break; }
    }
    let tcs = parse_stream_tool_calls(&events);
    Ok(ChatResponse { assistant_message: if full.is_empty() { None } else { Some(full) }, streamed_text_chunks: chunks, tool_calls: tcs, finish_reason: fr, raw_response: Value::Array(events) })
}

// ── Anthropic Messages API ──────────────────────────────────────────────────

fn build_anthropic_payload(request: &ChatRequest, stream: bool) -> Value {
    let messages: Vec<Value> = request.messages.iter().map(|msg| {
        json!({
            "role": anthropic_role(msg.role),
            "content": [{"type": "text", "text": msg.content}],
        })
    }).collect();

    let mut payload = json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": request.max_tokens.unwrap_or(4096) as u64,
        "stream": stream,
    });

    if request.enable_tool_calls && !request.tools.is_empty() {
        payload["tools"] = json!(request.tools.iter().map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters_json_schema,
            })
        }).collect::<Vec<_>>());
    }

    payload
}

fn anthropic_role(role: ChatRole) -> &'static str {
    match role {
        ChatRole::System => "system",
        ChatRole::User => "user",
        ChatRole::Assistant | ChatRole::Tool => "assistant",
        ChatRole::AgentEvent => "system",
    }
}

fn parse_anthropic_response(raw: Value) -> anyhow::Result<ChatResponse> {
    let content = raw.get("content").and_then(|c| c.as_array());
    let assistant_message = content.and_then(|arr| {
        arr.iter()
            .filter_map(|block| block.get("text").and_then(|v| v.as_str()))
            .collect::<Vec<_>>()
            .join("")
            .into()
    });

    let finish_reason = raw.get("stop_reason").and_then(|v| v.as_str()).map(|s| s.to_string());
    let text_chunks = assistant_message.as_ref().map(|t| t.chars().collect::<Vec<_>>().chunks(20).map(|c| c.iter().collect()).collect()).unwrap_or_default();

    Ok(ChatResponse {
        assistant_message,
        streamed_text_chunks: text_chunks,
        tool_calls: vec![],
        finish_reason,
        raw_response: raw,
    })
}

fn read_anthropic_stream(response: reqwest::blocking::Response, on_chunk: &mut dyn FnMut(String)) -> anyhow::Result<ChatResponse> {
    let mut full_text = String::new();
    let mut text_chunks = Vec::new();
    let mut finish_reason = None;

    for line in BufReader::new(response).lines() {
        let line = line?;
        if !line.starts_with("data: ") { continue; }
        let data = &line[6..];
        if data == "[DONE]" { break; }

        let ev: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };

        match ev.get("type").and_then(|v| v.as_str()) {
            Some("content_block_delta") => {
                if let Some(text) = ev.get("delta").and_then(|d| d.get("text")).and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        let ch = text.to_string();
                        full_text.push_str(&ch);
                        text_chunks.push(ch.clone());
                        on_chunk(ch);
                    }
                }
            }
            Some("message_stop") => {
                finish_reason = Some("stop".to_string());
            }
            _ => {}
        }
    }

    Ok(ChatResponse {
        assistant_message: if full_text.is_empty() { None } else { Some(full_text) },
        streamed_text_chunks: text_chunks,
        tool_calls: vec![],
        finish_reason,
        raw_response: Value::Null,
    })
}

fn map_role(role: ChatRole) -> &'static str {
    match role { ChatRole::System => "system", ChatRole::User => "user", ChatRole::Assistant => "assistant", ChatRole::Tool => "tool", ChatRole::AgentEvent => "system" }
}
