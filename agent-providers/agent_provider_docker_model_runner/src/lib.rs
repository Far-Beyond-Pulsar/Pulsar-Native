use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatMessage, ChatProvider, ChatRequest, ChatResponse,
    ChatRole, ModelDescriptor, ProviderAvailability, ProviderEnvironment, ProviderKind,
    ProviderMetadata, ToolCall,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

const DMR_CHAT_URL: &str = "http://localhost:12434/engines/v1/chat/completions";
const DMR_MODELS_URL: &str = "http://localhost:12434/engines/v1/models";
const DMR_MODEL_ENV: &str = "PULSAR_DOCKER_MODEL";

pub struct DockerModelRunnerProvider {
    client: Client,
}

impl DockerModelRunnerProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn default_models() -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: "ai/gemma4:4B",
            label: "Gemma 4 4B (Docker)",
            supports_tools: true,
            context_tokens: 0,
            compact_model: None,
        }]
    }

    fn looks_like_model_id(value: &str) -> bool {
        let value = value.trim();
        !value.is_empty()
            && !value.contains("http://")
            && !value.contains("https://")
            && !value.contains("/engines/v1")
            && !value.contains("/models")
            && !value.contains(' ')
    }

    fn resolve_model(request_model: &str, env: &dyn ProviderEnvironment) -> String {
        if let Some(env_model) = env.get_env(DMR_MODEL_ENV) {
            let env_model = env_model.trim().to_string();
            if Self::looks_like_model_id(&env_model) {
                return env_model;
            }
        }

        request_model.to_string()
    }

    fn parse_tool_arguments_value(value: Option<&Value>) -> Value {
        match value {
            Some(Value::String(raw)) => {
                serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.clone()))
            }
            Some(value) => value.clone(),
            None => json!({}),
        }
    }

    fn parse_assistant_message(raw: &Value) -> Option<String> {
        raw.get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .map(|s| s.to_string())
    }

    fn parse_tool_calls(raw: &Value) -> Vec<ToolCall> {
        raw.get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.as_array())
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|call| {
                        let id = call.get("id")?.as_str()?.to_string();
                        let function = call.get("function")?;
                        let name = function.get("name")?.as_str()?.to_string();
                        let arguments_json =
                            Self::parse_tool_arguments_value(function.get("arguments"));

                        Some(ToolCall {
                            id,
                            name,
                            arguments_json,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn parse_stream_tool_calls(raw_events: &[Value]) -> Vec<ToolCall> {
        #[derive(Default)]
        struct PartialToolCall {
            id: Option<String>,
            name: Option<String>,
            arguments: String,
        }

        let mut partials: Vec<PartialToolCall> = Vec::new();

        for event in raw_events {
            if let Some(choice) = event
                .get("choices")
                .and_then(|choices| choices.as_array())
                .and_then(|choices| choices.first())
            {
                if let Some(delta) = choice.get("delta") {
                    if let Some(tool_calls_array) =
                        delta.get("tool_calls").and_then(|value| value.as_array())
                    {
                        for tool_call in tool_calls_array {
                            let index = tool_call
                                .get("index")
                                .and_then(|value| value.as_u64())
                                .unwrap_or(partials.len() as u64)
                                as usize;

                            while partials.len() <= index {
                                partials.push(PartialToolCall::default());
                            }

                            let partial = &mut partials[index];

                            if let Some(id) = tool_call.get("id").and_then(|value| value.as_str()) {
                                partial.id = Some(id.to_string());
                            }

                            if let Some(function) = tool_call.get("function") {
                                if let Some(name) =
                                    function.get("name").and_then(|value| value.as_str())
                                {
                                    partial.name = Some(name.to_string());
                                }

                                if let Some(arguments_fragment) =
                                    function.get("arguments").and_then(|value| value.as_str())
                                {
                                    partial.arguments.push_str(arguments_fragment);
                                }
                            }
                        }
                    }
                }
            }
        }

        partials
            .into_iter()
            .filter_map(|partial| {
                let id = partial.id?;
                let name = partial.name?;
                let arguments_json = if partial.arguments.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str::<Value>(&partial.arguments)
                        .unwrap_or_else(|_| Value::String(partial.arguments.clone()))
                };

                Some(ToolCall {
                    id,
                    name,
                    arguments_json,
                })
            })
            .collect()
    }

    fn build_request_payload(model: &str, request: &ChatRequest) -> Value {
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|message: &ChatMessage| {
                let mut msg = json!({
                    "role": Self::map_role(message.role),
                    "content": message.content,
                });
                if let Some(tool_call_id) = &message.tool_call_id {
                    msg["tool_call_id"] = json!(tool_call_id);
                }
                if !message.tool_calls.is_empty() {
                    msg["tool_calls"] = json!(message
                        .tool_calls
                        .iter()
                        .map(|call| {
                            json!({
                                "id": call.id,
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": call.arguments_json.to_string(),
                                }
                            })
                        })
                        .collect::<Vec<_>>());
                }
                msg
            })
            .collect();

        let tools = if request.enable_tool_calls {
            request
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
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let mut payload = json!({
            "model": model,
            "messages": messages,
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
        if request.enable_tool_calls && !tools.is_empty() {
            payload["tools"] = Value::Array(tools);
        }

        payload
    }

    fn parse_non_stream_response(raw_response: Value) -> ChatResponse {
        let assistant_message = Self::parse_assistant_message(&raw_response);
        let tool_calls = Self::parse_tool_calls(&raw_response);
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
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());

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

        let reader = BufReader::new(response);
        for line in reader.lines() {
            let line = line.context("failed reading Docker Model Runner streaming line")?;
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
                serde_json::from_str(data).context("invalid JSON event in Docker stream")?;

            if let Some(choice) = event
                .get("choices")
                .and_then(|choices| choices.as_array())
                .and_then(|choices| choices.first())
            {
                if finish_reason.is_none() {
                    finish_reason = choice
                        .get("finish_reason")
                        .and_then(|value| value.as_str())
                        .map(|value| value.to_string());
                }

                if let Some(delta) = choice.get("delta") {
                    if let Some(content) = delta.get("content").and_then(|value| value.as_str()) {
                        if !content.is_empty() {
                            let chunk = content.to_string();
                            assistant_message.push_str(&chunk);
                            streamed_text_chunks.push(chunk.clone());
                            on_chunk(chunk);
                        }
                    } else if let Some(parts) =
                        delta.get("content").and_then(|value| value.as_array())
                    {
                        for part in parts {
                            if let Some(text) = part.get("text").and_then(|value| value.as_str()) {
                                if !text.is_empty() {
                                    let chunk = text.to_string();
                                    assistant_message.push_str(&chunk);
                                    streamed_text_chunks.push(chunk.clone());
                                    on_chunk(chunk);
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

        let tool_calls = Self::parse_stream_tool_calls(&raw_events);

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

    fn map_role(role: ChatRole) -> &'static str {
        match role {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::Tool => "tool",
            ChatRole::AgentEvent => "system",
        }
    }
}

impl Default for DockerModelRunnerProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for DockerModelRunnerProvider {
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "docker_model_runner",
            display_name: "Docker AI",
            endpoint: DMR_CHAT_URL,
            kind: ProviderKind::Local,
        }
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        Self::default_models()
    }

    fn availability(&self, env: &dyn ProviderEnvironment) -> ProviderAvailability {
        let _ = env;
        ProviderAvailability::ready()
    }

    fn auth_methods(&self) -> Vec<AuthMethod> {
        Vec::new()
    }

    fn authenticate(
        &self,
        method: AuthMethod,
        host: &mut dyn AuthHost,
    ) -> anyhow::Result<AuthResult> {
        let _ = method;
        let _ = host;
        Ok(AuthResult::Cancelled)
    }

    fn list_models_api(&self, _token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        let response = self
            .client
            .get(DMR_MODELS_URL)
            .header("Content-Type", "application/json")
            .send()
            .context("failed to call Docker Model Runner models API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!(
                "Docker Model Runner models API returned {}: {}",
                status,
                body
            ));
        }

        let raw: Value = response
            .json()
            .context("invalid JSON from Docker Model Runner models API")?;

        let models = raw
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
                            context_tokens: 0,
                            compact_model: None,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

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
        let payload = Self::build_request_payload(&model, request);

        let response = self
            .client
            .post(DMR_CHAT_URL)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .context("failed to call Docker Model Runner chat API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!(
                "Docker Model Runner chat API returned {}: {}",
                status,
                body
            ));
        }

        let raw_response: Value = response
            .json()
            .context("invalid JSON from Docker Model Runner chat API")?;
        Ok(Self::parse_non_stream_response(raw_response))
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
        let mut payload = Self::build_request_payload(&model, request);
        payload["stream"] = json!(true);

        let response = self
            .client
            .post(DMR_CHAT_URL)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&payload)
            .send()
            .context("failed to call Docker Model Runner streaming chat API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!(
                "Docker Model Runner streaming API returned {}: {}",
                status,
                body
            ));
        }

        Self::read_stream_response(response, on_chunk)
    }
}
