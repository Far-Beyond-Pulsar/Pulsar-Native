use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatMessage, ChatProvider, ChatRequest, ChatResponse,
    ChatRole, ModelDescriptor, ProviderAvailability, ProviderEnvironment, ProviderKind,
    ProviderMetadata, ToolCall,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

const OLLAMA_CHAT_URL: &str = "http://localhost:11434/api/chat";
const OLLAMA_MODELS_URL: &str = "http://localhost:11434/api/tags";
const OLLAMA_SHOW_URL: &str = "http://localhost:11434/api/show";
const OLLAMA_MODEL_ENV: &str = "PULSAR_OLLAMA_MODEL";

pub struct OllamaProvider {
    client: Client,
}

impl OllamaProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn default_models() -> Vec<ModelDescriptor> {
        vec![]
    }

    fn resolve_model(request_model: &str, env: &dyn ProviderEnvironment) -> String {
        if let Some(env_model) = env.get_env(OLLAMA_MODEL_ENV) {
            let trimmed = env_model.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }

        request_model.to_string()
    }

    fn map_role(role: ChatRole) -> &'static str {
        match role {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::Tool => "tool",
        }
    }

    fn build_request_payload(model: &str, request: &ChatRequest, stream: bool) -> Value {
        let messages = request
            .messages
            .iter()
            .map(|message: &ChatMessage| {
                let mut msg = serde_json::Map::new();
                msg.insert("role".to_string(), json!(Self::map_role(message.role)));
                msg.insert("content".to_string(), json!(message.content));

                if !message.tool_calls.is_empty() {
                    let ollama_calls = message
                        .tool_calls
                        .iter()
                        .map(|call| {
                            json!({
                                "id": call.id,
                                "function": {
                                    "name": call.name,
                                    "arguments": call.arguments_json,
                                }
                            })
                        })
                        .collect::<Vec<_>>();
                    msg.insert("tool_calls".to_string(), Value::Array(ollama_calls));
                }

                Value::Object(msg)
            })
            .collect::<Vec<_>>();

        let mut payload = json!({
            "model": model,
            "messages": messages,
            "stream": stream,
        });

        let mut options = serde_json::Map::new();
        if let Some(temperature) = request.temperature {
            options.insert("temperature".to_string(), json!(temperature));
        }
        if let Some(top_p) = request.top_p {
            options.insert("top_p".to_string(), json!(top_p));
        }
        if let Some(max_tokens) = request.max_tokens {
            options.insert("num_predict".to_string(), json!(max_tokens));
        }
        if !options.is_empty() {
            payload["options"] = Value::Object(options);
        }

        if request.enable_tool_calls && !request.tools.is_empty() {
            let tools = request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters_json_schema,
                        }
                    })
                })
                .collect::<Vec<_>>();
            payload["tools"] = Value::Array(tools);
        }

        payload
    }

    fn parse_tool_calls(message: Option<&Value>, next_call_index: &mut usize) -> Vec<ToolCall> {
        let Some(message) = message else {
            return Vec::new();
        };
        let Some(calls) = message.get("tool_calls").and_then(|value| value.as_array()) else {
            return Vec::new();
        };

        calls
            .iter()
            .filter_map(|call| {
                let function = call.get("function")?;
                let name = function.get("name")?.as_str()?.to_string();

                let arguments_json = match function.get("arguments") {
                    Some(Value::String(raw)) => {
                        serde_json::from_str::<Value>(raw).unwrap_or_else(|_| json!({}))
                    }
                    Some(value) => value.clone(),
                    None => json!({}),
                };

                let id = call
                    .get("id")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| {
                        let generated = format!("ollama_call_{}", *next_call_index);
                        *next_call_index += 1;
                        generated
                    });

                Some(ToolCall {
                    id,
                    name,
                    arguments_json,
                })
            })
            .collect()
    }

    /// Query `/api/show` for a single model to get its actual context length.
    /// Returns 0 (unknown) on any error — callers treat 0 as "use fallback".
    fn fetch_model_context_tokens(&self, model_id: &str) -> u32 {
        let Ok(resp) = self
            .client
            .post(OLLAMA_SHOW_URL)
            .json(&json!({"model": model_id}))
            .send()
        else {
            return 0;
        };
        let Ok(body) = resp.json::<Value>() else {
            return 0;
        };

        // 1. Try model_info.*.context_length (varies by architecture key)
        if let Some(info) = body.get("model_info").and_then(|v| v.as_object()) {
            for (_key, val) in info {
                if _key.ends_with(".context_length") {
                    if let Some(n) = val.as_u64() {
                        return n as u32;
                    }
                }
            }
        }

        // 2. Try parsing `num_ctx` from the parameters string
        if let Some(params) = body.get("parameters").and_then(|v| v.as_str()) {
            for line in params.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("num_ctx") {
                    if let Ok(n) = rest.trim().parse::<u32>() {
                        return n;
                    }
                }
            }
        }

        0
    }

    fn fetch_models_from_api(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        let response = self
            .client
            .get(OLLAMA_MODELS_URL)
            .header("Content-Type", "application/json")
            .send()
            .context("failed to call Ollama models API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!("Ollama models API returned {}: {}", status, body));
        }

        let raw: Value = response
            .json()
            .context("invalid JSON from Ollama models API")?;

        Ok(raw
            .get("models")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let id = item.get("name")?.as_str()?.to_string();
                        let context_tokens = self.fetch_model_context_tokens(&id);
                        Some(ModelDescriptor {
                            id: Box::leak(id.clone().into_boxed_str()),
                            label: Box::leak(id.into_boxed_str()),
                            supports_tools: true,
                            context_tokens,
                            compact_model: None,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default())
    }

    fn parse_response(raw_response: Value) -> ChatResponse {
        let assistant_message = raw_response
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .map(|content| content.to_string())
            .filter(|content| !content.is_empty());

        let mut next_call_index = 1usize;
        let tool_calls = Self::parse_tool_calls(raw_response.get("message"), &mut next_call_index);

        let streamed_text_chunks = assistant_message
            .as_ref()
            .map(|text| {
                text.chars()
                    .collect::<Vec<_>>()
                    .chunks(20)
                    .map(|chunk| chunk.iter().collect::<String>())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let finish_reason = raw_response
            .get("done_reason")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .or_else(|| {
                raw_response
                    .get("done")
                    .and_then(|value| value.as_bool())
                    .and_then(|done| if done { Some("stop".to_string()) } else { None })
            });

        ChatResponse {
            assistant_message,
            streamed_text_chunks,
            tool_calls,
            finish_reason,
            raw_response,
        }
    }

    fn read_stream_response(
        response: reqwest::blocking::Response,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let mut raw_events = Vec::new();
        let mut streamed_text_chunks = Vec::new();
        let mut assistant_message = String::new();
        let mut finish_reason = None;
        let mut tool_calls = Vec::new();
        let mut next_call_index = 1usize;

        // Accumulate thinking content from models that emit a separate `thinking`
        // field (e.g. qwen3). Flushed as `<think>…</think>` before the first
        // content chunk so the upstream tag-detection in streaming.rs picks it up.
        let mut thinking_buf = String::new();
        let mut thinking_flushed = false;

        let reader = BufReader::new(response);
        for line in reader.lines() {
            let line = line.context("failed reading Ollama streaming response line")?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let event: Value =
                serde_json::from_str(line).context("invalid JSON event in Ollama stream")?;

            if let Some(err) = event.get("error").and_then(|value| value.as_str()) {
                return Err(anyhow!("Ollama streaming API returned error: {}", err));
            }

            if let Some(message) = event.get("message") {
                // Accumulate thinking tokens (qwen3 and similar thinking models).
                if let Some(thinking) = message.get("thinking").and_then(|v| v.as_str()) {
                    if !thinking.is_empty() {
                        thinking_buf.push_str(thinking);
                    }
                }

                if let Some(content) = message.get("content").and_then(|value| value.as_str()) {
                    if !content.is_empty() {
                        // Flush accumulated thinking as a tagged block before first content.
                        if !thinking_flushed && !thinking_buf.is_empty() {
                            let block = format!("<think>{}</think>", thinking_buf);
                            on_chunk(block);
                            thinking_flushed = true;
                        }

                        let chunk = content.to_string();
                        assistant_message.push_str(&chunk);
                        streamed_text_chunks.push(chunk.clone());
                        on_chunk(chunk);
                    }
                }

                let mut calls = Self::parse_tool_calls(Some(message), &mut next_call_index);
                tool_calls.append(&mut calls);
            }

            if finish_reason.is_none() {
                finish_reason = event
                    .get("done_reason")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string());
            }
            if finish_reason.is_none()
                && event
                    .get("done")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
            {
                finish_reason = Some("stop".to_string());
            }

            let done = event
                .get("done")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            raw_events.push(event);
            if done {
                break;
            }
        }

        Ok(ChatResponse {
            assistant_message: if assistant_message.is_empty() {
                None
            } else {
                Some(assistant_message)
            },
            streamed_text_chunks,
            tool_calls,
            finish_reason,
            raw_response: Value::Array(raw_events),
        })
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for OllamaProvider {
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "ollama",
            display_name: "Ollama",
            endpoint: OLLAMA_CHAT_URL,
            kind: ProviderKind::Local,
        }
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        self.fetch_models_from_api()
            .ok()
            .filter(|models| !models.is_empty())
            .unwrap_or_else(Self::default_models)
    }

    fn availability(&self, _env: &dyn ProviderEnvironment) -> ProviderAvailability {
        ProviderAvailability::ready()
    }

    fn auth_methods(&self) -> Vec<AuthMethod> {
        Vec::new()
    }

    fn authenticate(
        &self,
        _method: AuthMethod,
        _host: &mut dyn AuthHost,
    ) -> anyhow::Result<AuthResult> {
        Ok(AuthResult::Cancelled)
    }

    fn list_models_api(&self, _token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        let models = self.fetch_models_from_api()?;

        if models.is_empty() {
            Ok(Self::default_models())
        } else {
            Ok(models)
        }
    }

    fn chat_completion(&self, token: &str, request: &ChatRequest) -> anyhow::Result<ChatResponse> {
        let env = agent_chat_core::ProcessEnvironment;
        let _ = token;
        let model = Self::resolve_model(&request.model, &env);
        let payload = Self::build_request_payload(&model, request, false);

        let response = self
            .client
            .post(OLLAMA_CHAT_URL)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .context("failed to call Ollama chat API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!("Ollama API returned {}: {}", status, body));
        }

        let raw_response: Value = response.json().context("invalid JSON from Ollama API")?;
        Ok(Self::parse_response(raw_response))
    }

    fn chat_completion_streaming(
        &self,
        token: &str,
        request: &ChatRequest,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let env = agent_chat_core::ProcessEnvironment;
        let _ = token;
        let model = Self::resolve_model(&request.model, &env);
        let payload = Self::build_request_payload(&model, request, true);

        let response = self
            .client
            .post(OLLAMA_CHAT_URL)
            .header("Content-Type", "application/json")
            .header("Accept", "application/x-ndjson")
            .json(&payload)
            .send()
            .context("failed to call Ollama streaming chat API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!(
                "Ollama streaming API returned {}: {}",
                status,
                body
            ));
        }

        Self::read_stream_response(response, on_chunk)
    }
}
