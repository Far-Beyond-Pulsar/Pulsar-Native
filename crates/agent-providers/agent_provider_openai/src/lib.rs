use agent_chat_core::{
    ChatMessage, ChatProvider, ChatRequest, ChatResponse, ChatRole, ConfigField, ModelDescriptor,
    ProviderConfig, ProviderCrate, ProviderEntry, ProviderKind, ToolCall,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

// ── Provider entries ────────────────────────────────────────────────────────

struct Entry {
    id: &'static str,
    display_name: &'static str,
    kind: ProviderKind,
    endpoint: Option<&'static str>,
    use_ollama_protocol: bool,
}

const ENTRIES: &[Entry] = &[
    Entry { id: "openai", display_name: "OpenAI", kind: ProviderKind::Cloud, endpoint: Some("https://api.openai.com/v1"), use_ollama_protocol: false },
    Entry { id: "groq", display_name: "Groq", kind: ProviderKind::Cloud, endpoint: Some("https://api.groq.com/openai/v1"), use_ollama_protocol: false },
    Entry { id: "together", display_name: "Together AI", kind: ProviderKind::Cloud, endpoint: Some("https://api.together.xyz/v1"), use_ollama_protocol: false },
    Entry { id: "mistral", display_name: "Mistral AI", kind: ProviderKind::Cloud, endpoint: Some("https://api.mistral.ai/v1"), use_ollama_protocol: false },
    Entry { id: "deepseek", display_name: "DeepSeek", kind: ProviderKind::Cloud, endpoint: Some("https://api.deepseek.com/v1"), use_ollama_protocol: false },
    Entry { id: "fireworks", display_name: "Fireworks AI", kind: ProviderKind::Cloud, endpoint: Some("https://api.fireworks.ai/inference/v1"), use_ollama_protocol: false },
    Entry { id: "perplexity", display_name: "Perplexity", kind: ProviderKind::Cloud, endpoint: Some("https://api.perplexity.ai"), use_ollama_protocol: false },
    Entry { id: "xai", display_name: "xAI Grok", kind: ProviderKind::Cloud, endpoint: Some("https://api.x.ai/v1"), use_ollama_protocol: false },
    Entry { id: "openrouter", display_name: "OpenRouter", kind: ProviderKind::Cloud, endpoint: Some("https://openrouter.ai/api/v1"), use_ollama_protocol: false },
    Entry { id: "cohere", display_name: "Cohere", kind: ProviderKind::Cloud, endpoint: Some("https://api.cohere.com/v2"), use_ollama_protocol: false },
    Entry { id: "azure_openai", display_name: "Azure OpenAI", kind: ProviderKind::Cloud, endpoint: None, use_ollama_protocol: false },
    Entry { id: "ollama", display_name: "Ollama", kind: ProviderKind::Local, endpoint: Some("http://localhost:11434"), use_ollama_protocol: true },
    Entry { id: "lm_studio", display_name: "LM Studio", kind: ProviderKind::Local, endpoint: Some("http://localhost:1234/v1"), use_ollama_protocol: false },
    Entry { id: "llama_cpp", display_name: "llama.cpp", kind: ProviderKind::Local, endpoint: Some("http://localhost:8080/v1"), use_ollama_protocol: false },
    Entry { id: "vllm", display_name: "vLLM", kind: ProviderKind::Local, endpoint: Some("http://localhost:8000/v1"), use_ollama_protocol: false },
    Entry { id: "custom_openai", display_name: "Custom OpenAI Compatible", kind: ProviderKind::Local, endpoint: None, use_ollama_protocol: false },
];

fn entry_config_fields(entry: &Entry, is_ollama: bool) -> Vec<ConfigField> {
    let mut fields = Vec::new();
    if entry.endpoint.is_none() {
        fields.push(ConfigField {
            key: "endpoint_url",
            label: "Endpoint URL",
            description: if is_ollama { "Ollama server URL" } else { "API base URL" },
            sensitive: false,
            required: true,
            placeholder: if is_ollama { Some("http://localhost:11434") } else { Some("https://api.example.com/v1") },
        });
    }
    if entry.kind == ProviderKind::Cloud {
        fields.push(ConfigField {
            key: "api_key",
            label: "API Key",
            description: format!("{} API key", entry.display_name).leak(),
            sensitive: true,
            required: true,
            placeholder: None,
        });
    }
    fields
}

// ── ProviderCrate ──────────────────────────────────────────────────────────

pub struct OpenAiProviderCrate;

impl ProviderCrate for OpenAiProviderCrate {
    fn entries(&self) -> Vec<ProviderEntry> {
        ENTRIES
            .iter()
            .map(|e| {
                let is_ollama = e.id == "ollama" || e.id == "custom_openai";
                ProviderEntry {
                    id: e.id,
                    display_name: e.display_name,
                    kind: e.kind,
                    default_endpoint: e.endpoint,
                    config_fields: entry_config_fields(e, is_ollama),
                }
            })
            .collect()
    }

    fn create(&self, id: &str, config: ProviderConfig) -> anyhow::Result<Box<dyn ChatProvider>> {
        let entry = ENTRIES
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| anyhow!("unknown provider id: {id}"))?;

        let endpoint = match entry.endpoint {
            Some(ep) => ep.to_string(),
            None => config.require("endpoint_url")?.to_string(),
        };

        let api_key = config.get("api_key").unwrap_or_default().to_string();
        let chat_url = build_chat_url(&endpoint, entry.use_ollama_protocol);
        let models_url = build_models_url(&chat_url, entry.use_ollama_protocol);

        let config_fields = entry_config_fields(entry, entry.use_ollama_protocol);

        Ok(Box::new(OpenAiChatProvider {
            client: Client::new(),
            id: id.to_string(),
            display_name: entry.display_name.to_string(),
            chat_url,
            models_url,
            api_key,
            is_ollama: entry.use_ollama_protocol,
        }))
    }
}

// ── URL helpers ─────────────────────────────────────────────────────────────

fn build_chat_url(endpoint: &str, ollama: bool) -> String {
    let t = endpoint.trim_end_matches('/');
    if ollama {
        if t.ends_with("/api/chat") {
            t.to_string()
        } else {
            format!("{t}/api/chat")
        }
    } else if t.ends_with("/chat/completions") {
        t.to_string()
    } else {
        format!("{t}/chat/completions")
    }
}

fn build_models_url(endpoint: &str, _ollama: bool) -> String {
    let t = endpoint.trim_end_matches('/');
    if t.ends_with("/chat/completions") {
        t.trim_end_matches("/chat/completions").to_string() + "/models"
    } else if t.ends_with("/api/chat") {
        t.trim_end_matches("/api/chat").to_string() + "/api/tags"
    } else {
        format!("{t}/models")
    }
}

// ── ChatProvider ────────────────────────────────────────────────────────────

struct OpenAiChatProvider {
    client: Client,
    id: String,
    display_name: String,
    chat_url: String,
    models_url: String,
    api_key: String,
    is_ollama: bool,
}

impl ChatProvider for OpenAiChatProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        if self.is_ollama {
            return self.ollama_models();
        }

        let mut req = self.client.get(&self.models_url).header("Content-Type", "application/json");
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let resp = req.send().with_context(|| format!("fetch models from {}", self.models_url))?;
        if !resp.status().is_success() {
            return Err(anyhow!("models API {}: {}", resp.status(), resp.text().unwrap_or_default()));
        }
        let body: Value = resp.json()?;
        let models = body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m.get("id")?.as_str()?.to_string();
                        let label = m.get("display_name").or_else(|| m.get("name")).or_else(|| m.get("id")).and_then(|v| v.as_str()).unwrap_or(&id).to_string();
                        Some(ModelDescriptor { id, label, supports_tools: true, context_tokens: 0, compact_model: None })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(models)
    }

    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let payload = if self.is_ollama {
            build_ollama_payload(&request, false)
        } else {
            build_openai_payload(&request, false)?
        };

        let body = self.send(&payload)?;
        if self.is_ollama {
            parse_ollama_response(body)
        } else {
            parse_non_stream_response(body)
        }
    }

    fn chat_streaming(&self, request: ChatRequest, on_chunk: &mut dyn FnMut(String)) -> anyhow::Result<ChatResponse> {
        let payload = if self.is_ollama {
            build_ollama_payload(&request, true)
        } else {
            build_openai_payload(&request, true)?
        };

        let mut req = self.client.post(&self.chat_url).header("Content-Type", "application/json").json(&payload);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        if self.is_ollama {
            req = req.header("Accept", "application/x-ndjson");
        } else {
            req = req.header("Accept", "text/event-stream");
        }

        let response = req.send().with_context(|| format!("chat {}", self.chat_url))?;
        if !response.status().is_success() {
            return Err(anyhow!("chat API {}: {}", response.status(), response.text().unwrap_or_default()));
        }

        if self.is_ollama {
            read_ollama_stream(response, on_chunk)
        } else {
            read_stream_response(response, on_chunk)
        }
    }
}

impl OpenAiChatProvider {
    fn send(&self, payload: &Value) -> anyhow::Result<Value> {
        let mut req = self.client.post(&self.chat_url).header("Content-Type", "application/json").json(payload);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let resp = req.send().with_context(|| format!("chat {}", self.chat_url))?;
        if !resp.status().is_success() {
            return Err(anyhow!("chat API {}: {}", resp.status(), resp.text().unwrap_or_default()));
        }
        Ok(resp.json()?)
    }

    fn ollama_models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        let resp = self.client.get(&self.models_url).send().with_context(|| format!("ollama tags from {}", self.models_url))?;
        if !resp.status().is_success() {
            return Err(anyhow!("ollama tags API {}: {}", resp.status(), resp.text().unwrap_or_default()));
        }
        let body: Value = resp.json()?;
        let models = body
            .get("models")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let name = m.get("name").and_then(|v| v.as_str())?.to_string();
                        Some(ModelDescriptor { id: name.clone(), label: name, supports_tools: true, context_tokens: 0, compact_model: None })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(models)
    }
}

// ── OpenAI request/response ─────────────────────────────────────────────────

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

// ── Ollama request/response ─────────────────────────────────────────────────

fn build_ollama_payload(request: &ChatRequest, stream: bool) -> Value {
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
    if let Some(t) = request.temperature { opts.insert("temperature".into(), json!(t)); }
    if let Some(p) = request.top_p { opts.insert("top_p".into(), json!(p)); }
    if let Some(m) = request.max_tokens { opts.insert("num_predict".into(), json!(m)); }
    if !opts.is_empty() { payload["options"] = Value::Object(opts); }
    if request.enable_tool_calls && !request.tools.is_empty() {
        payload["tools"] = json!(request.tools.iter().map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.parameters_json_schema } })).collect::<Vec<_>>());
    }
    payload
}

fn parse_ollama_tc(message: Option<&Value>, next: &mut usize) -> Vec<ToolCall> {
    let Some(calls) = message.and_then(|m| m.get("tool_calls")).and_then(|v| v.as_array()) else { return vec![] };
    calls.iter().filter_map(|call| {
        let func = call.get("function")?;
        let name = func.get("name")?.as_str()?.to_string();
        let args = match func.get("arguments") { Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({})), Some(v) => v.clone(), None => json!({}) };
        let id = call.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| { let i = *next; *next += 1; format!("ollama_{i}") });
        Some(ToolCall { id, name, arguments_json: args })
    }).collect()
}

fn parse_ollama_response(raw: Value) -> anyhow::Result<ChatResponse> {
    let msg = raw.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_str()).map(|s| s.to_string()).filter(|s| !s.is_empty());
    let mut n = 1;
    let tcs = parse_ollama_tc(raw.get("message"), &mut n);
    let chunks = msg.as_ref().map(|t| t.chars().collect::<Vec<_>>().chunks(20).map(|c| c.iter().collect()).collect()).unwrap_or_default();
    let fr = raw.get("done_reason").and_then(|v| v.as_str()).map(|s| s.to_string())
        .or_else(|| raw.get("done").and_then(|v| v.as_bool()).and_then(|d| if d { Some("stop".into()) } else { None }));
    Ok(ChatResponse { assistant_message: msg, streamed_text_chunks: chunks, tool_calls: tcs, finish_reason: fr, raw_response: raw })
}

fn read_ollama_stream(response: reqwest::blocking::Response, on_chunk: &mut dyn FnMut(String)) -> anyhow::Result<ChatResponse> {
    let mut events = Vec::new();
    let mut chunks = Vec::new();
    let mut full = String::new();
    let mut fr = None;
    let mut tcs = Vec::new();
    let mut n = 1;
    for line in BufReader::new(response).lines() {
        let line = line?; let line = line.trim();
        if line.is_empty() { continue; }
        let ev: Value = serde_json::from_str(line)?;
        if let Some(e) = ev.get("error").and_then(|v| v.as_str()) { return Err(anyhow!("ollama error: {e}")); }
        if let Some(msg) = ev.get("message") {
            if let Some(c) = msg.get("content").and_then(|v| v.as_str()) {
                if !c.is_empty() { let ch = c.to_string(); full.push_str(&ch); chunks.push(ch.clone()); on_chunk(ch); }
            }
            tcs.append(&mut parse_ollama_tc(Some(msg), &mut n));
        }
        if fr.is_none() { fr = ev.get("done_reason").and_then(|v| v.as_str()).map(|s| s.to_string()); }
        if fr.is_none() && ev.get("done").and_then(|v| v.as_bool()).unwrap_or(false) { fr = Some("stop".into()); }
        let done = ev.get("done").and_then(|v| v.as_bool()).unwrap_or(false);
        events.push(ev);
        if done { break; }
    }
    Ok(ChatResponse { assistant_message: if full.is_empty() { None } else { Some(full) }, streamed_text_chunks: chunks, tool_calls: tcs, finish_reason: fr, raw_response: Value::Array(events) })
}

// ── Shared ──────────────────────────────────────────────────────────────────

fn map_role(role: ChatRole) -> &'static str {
    match role { ChatRole::System => "system", ChatRole::User => "user", ChatRole::Assistant => "assistant", ChatRole::Tool => "tool", ChatRole::AgentEvent => "system" }
}
