use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatMessage, ChatProvider, ChatRequest, ChatResponse,
    ChatRole, ModelDescriptor, ProviderAvailability, ProviderEnvironment, ProviderKind,
    ProviderMetadata, ToolCall,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

const LMSTUDIO_CHAT_URL: &str = "http://localhost:1234/v1/chat/completions";
const LMSTUDIO_MODELS_URL: &str = "http://localhost:1234/v1/models";
const LMSTUDIO_MODEL_ENV: &str = "PULSAR_LMSTUDIO_MODEL";

pub struct LmStudioProvider {
    client: Client,
}

impl LmStudioProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn default_models() -> Vec<ModelDescriptor> {
        vec![]
    }

    fn resolve_model(request_model: &str, env: &dyn ProviderEnvironment) -> String {
        if let Some(env_model) = env.get_env(LMSTUDIO_MODEL_ENV) {
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
                json!({
                    "role": Self::map_role(message.role),
                    "content": message.content,
                })
            })
            .collect::<Vec<_>>();

        let mut payload = json!({
            "model": model,
            "messages": messages,
            "stream": stream,
        });

        if let Some(temperature) = request.temperature {
            payload["temperature"] = json!(temperature);
        }
        if let Some(top_p) = request.top_p {
            payload["top_p"] = json!(top_p);
        }
        if let Some(max_tokens) = request.max_tokens {
            payload["max_tokens"] = json!(max_tokens);
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
                        let generated = format!("lmstudio_call_{}", *next_call_index);
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

    fn fetch_models_from_api(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        let response = self
            .client
            .get(LMSTUDIO_MODELS_URL)
            .header("Content-Type", "application/json")
            .send()
            .context("failed to call LM Studio models API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!("LM Studio models API returned {}: {}", status, body));
        }

        let raw: Value = response
            .json()
            .context("invalid JSON from LM Studio models API")?;

        Ok(raw
            .get("data")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let id = item.get("id")?.as_str()?.to_string();
                        Some(ModelDescriptor {
                            id: Box::leak(id.clone().into_boxed_str()),
                            label: Box::leak(id.into_boxed_str()),
                            supports_tools: true,
                            context_tokens: 0, // LM Studio doesn't expose this via API
                            compact_model: None,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default())
    }

    fn parse_response(raw_response: Value) -> ChatResponse {
        let assistant_message = raw_response
            .get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .map(|content| content.to_string())
            .filter(|content| !content.is_empty());

        let mut next_call_index = 1usize;
        let tool_calls = Self::parse_tool_calls(
            raw_response
                .get("choices")
                .and_then(|c| c.as_array())
                .and_then(|c| c.first())
                .and_then(|c| c.get("message")),
            &mut next_call_index,
        );

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
            .get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

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
        let mut tool_calls_by_index: std::collections::BTreeMap<usize, ToolCall> = std::collections::BTreeMap::new();
        let mut next_call_index = 0usize;

        let reader = BufReader::new(response);
        for line in reader.lines() {
            let line = line.context("failed reading LM Studio streaming response line")?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }

            let event: Value =
                serde_json::from_str(data).context("invalid JSON event in LM Studio stream")?;

            if let Some(choice) = event
                .get("choices")
                .and_then(|choices| choices.as_array())
                .and_then(|choices| choices.first())
            {
                if finish_reason.is_none() {
                    finish_reason = choice
                        .get("finish_reason")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }

                if let Some(delta) = choice.get("delta") {
                    if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
                        if !content.is_empty() {
                            let chunk = content.to_string();
                            assistant_message.push_str(&chunk);
                            streamed_text_chunks.push(chunk.clone());
                            on_chunk(chunk);
                        }
                    }

                    // Handle incremental tool_calls from delta (OpenAI format)
                    // Each delta event has a tool_calls array with index and partial data
                    if let Some(tool_calls_array) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                        for tool_call in tool_calls_array {
                            if let Some(index) = tool_call.get("index").and_then(|v| v.as_u64()) {
                                let idx = index as usize;
                                let entry = tool_calls_by_index.entry(idx).or_insert_with(|| {
                                    if idx >= next_call_index {
                                        next_call_index = idx + 1;
                                    }
                                    ToolCall {
                                        id: format!("lmstudio_call_{}", idx),
                                        name: String::new(),
                                        arguments_json: json!({}),
                                    }
                                });

                                // Update function name if present
                                if let Some(fn_obj) = tool_call.get("function") {
                                    if let Some(name) = fn_obj.get("name").and_then(|v| v.as_str()) {
                                        entry.name = name.to_string();
                                    }
                                    // Accumulate arguments string
                                    if let Some(args_delta) = fn_obj.get("arguments").and_then(|v| v.as_str()) {
                                        entry.arguments_json = match &entry.arguments_json {
                                            Value::Object(obj) if obj.is_empty() => {
                                                // First chunk - try to parse as JSON
                                                serde_json::from_str::<Value>(args_delta)
                                                    .unwrap_or_else(|_| Value::String(args_delta.to_string()))
                                            }
                                            existing => {
                                                // Append to existing - if it's not a complete JSON yet,
                                                // just keep it as a string representation
                                                existing.clone()
                                            }
                                        };
                                    }
                                }
                            }
                        }
                    }
                }
            }

            raw_events.push(event);
            if finish_reason.is_some() {
                break;
            }
        }

        let tool_calls = tool_calls_by_index.into_values().collect();

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

impl Default for LmStudioProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for LmStudioProvider {
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "lmstudio",
            display_name: "LM Studio",
            endpoint: LMSTUDIO_CHAT_URL,
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
            .post(LMSTUDIO_CHAT_URL)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .context("failed to call LM Studio chat API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!("LM Studio API returned {}: {}", status, body));
        }

        let raw_response: Value = response.json().context("invalid JSON from LM Studio API")?;
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
            .post(LMSTUDIO_CHAT_URL)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&payload)
            .send()
            .context("failed to call LM Studio streaming chat API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!(
                "LM Studio streaming API returned {}: {}",
                status,
                body
            ));
        }

        Self::read_stream_response(response, on_chunk)
    }
}
