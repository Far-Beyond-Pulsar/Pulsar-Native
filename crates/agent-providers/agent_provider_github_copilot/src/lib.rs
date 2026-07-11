use agent_chat_core::{
    ChatMessage, ChatProvider, ChatRequest, ChatResponse, ChatRole, ModelDescriptor, ToolCall,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

const GITHUB_MODELS_CHAT_URL: &str = "https://models.github.ai/inference/chat/completions";
const GITHUB_API_VERSION: &str = "2026-03-10";

const GITHUB_MODEL_ENV: &str = "PULSAR_GITHUB_MODEL";

pub struct GithubCopilotProvider {
    client: Client,
    api_key: String,
    models: Vec<ModelDescriptor>,
}

impl GithubCopilotProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            models: Self::static_models(),
        }
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "openai/gpt-4.1".to_string(),
                label: "OpenAI GPT-4.1 (GitHub Models)".to_string(),
                supports_tools: true,
                context_tokens: 131072,
                compact_model: None,
            },
            ModelDescriptor {
                id: "openai/gpt-4.1-mini".to_string(),
                label: "OpenAI GPT-4.1 Mini (GitHub Models)".to_string(),
                supports_tools: true,
                context_tokens: 131072,
                compact_model: None,
            },
            ModelDescriptor {
                id: "openai/gpt-4.1-nano".to_string(),
                label: "OpenAI GPT-4.1 Nano (GitHub Models)".to_string(),
                supports_tools: true,
                context_tokens: 131072,
                compact_model: None,
            },
            ModelDescriptor {
                id: "openai/o4-mini".to_string(),
                label: "OpenAI o4 Mini (GitHub Models)".to_string(),
                supports_tools: true,
                context_tokens: 200000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "openai/o3".to_string(),
                label: "OpenAI o3 (GitHub Models)".to_string(),
                supports_tools: false,
                context_tokens: 200000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "google/gemini-2.5-pro".to_string(),
                label: "Gemini 2.5 Pro (GitHub Models)".to_string(),
                supports_tools: true,
                context_tokens: 1048576,
                compact_model: None,
            },
            ModelDescriptor {
                id: "google/gemini-2.5-flash".to_string(),
                label: "Gemini 2.5 Flash (GitHub Models)".to_string(),
                supports_tools: true,
                context_tokens: 1048576,
                compact_model: None,
            },
            ModelDescriptor {
                id: "meta/llama-4-scout".to_string(),
                label: "Llama 4 Scout (GitHub Models)".to_string(),
                supports_tools: true,
                context_tokens: 131072,
                compact_model: None,
            },
        ]
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

    fn resolve_model(request_model: &str) -> String {
        if let Ok(env_model) = std::env::var(GITHUB_MODEL_ENV) {
            let trimmed = env_model.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        request_model.to_string()
    }

    fn build_messages_payload(messages: &[ChatMessage]) -> Vec<Value> {
        messages
            .iter()
            .map(|message| {
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
            .collect()
    }

    fn build_request_payload(model: &str, request: &ChatRequest, stream: bool) -> Value {
        let messages = Self::build_messages_payload(&request.messages);

        let mut payload = json!({
            "model": model,
            "messages": messages,
        });

        if stream {
            payload["stream"] = json!(true);
        }

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
            let tools: Vec<Value> = request
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
                .collect();
            payload["tools"] = Value::Array(tools);
            payload["tool_choice"] = json!("auto");
        }

        payload
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

    fn parse_assistant_message(raw: &Value) -> Option<String> {
        let message = raw
            .get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))?;

        let reasoning = message
            .get("reasoning_content")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        let content = message
            .get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());

        match (reasoning, content) {
            (Some(thinking), Some(body)) => Some(format!("<think>{}</think>{}", thinking, body)),
            (Some(thinking), None) => Some(format!("<think>{}</think>", thinking)),
            (None, Some(body)) => Some(body),
            (None, None) => None,
        }
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
                        delta.get("tool_calls").and_then(|v| v.as_array())
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
                            if let Some(id) = tool_call.get("id").and_then(|v| v.as_str()) {
                                partial.id = Some(id.to_string());
                            }

                            if let Some(function) = tool_call.get("function") {
                                if let Some(name) = function.get("name").and_then(|v| v.as_str()) {
                                    partial.name = Some(name.to_string());
                                }
                                if let Some(args) =
                                    function.get("arguments").and_then(|v| v.as_str())
                                {
                                    partial.arguments.push_str(args);
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

    fn extract_error_message(body: &str) -> Option<String> {
        let parsed: Value = serde_json::from_str(body).ok()?;
        parsed
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(|message| message.as_str())
            .map(|message| message.trim().to_string())
            .filter(|message| !message.is_empty())
    }

    fn format_http_error(response: reqwest::blocking::Response, api_name: &str) -> anyhow::Error {
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let body = response.text().unwrap_or_default();

        if status.as_u16() == 429 {
            let server_message = Self::extract_error_message(&body)
                .unwrap_or_else(|| "Too many requests to GitHub Models.".to_string());
            let retry_hint = retry_after
                .as_deref()
                .map(|seconds| format!(" Retry-After: {seconds} seconds."))
                .unwrap_or_else(|| " Wait a short time and try again.".to_string());

            return anyhow!(
                "GitHub Models rate limit hit (429) during {api_name}. {server_message}{retry_hint}"
            );
        }

        let body_for_message = if body.trim().is_empty() {
            "<empty body>".to_string()
        } else {
            body
        };

        anyhow!("GitHub Models {api_name} returned {status}: {body_for_message}")
    }

    fn read_stream_response(
        response: reqwest::blocking::Response,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let mut raw_events: Vec<Value> = Vec::new();
        let mut streamed_text_chunks: Vec<String> = Vec::new();
        let mut assistant_message = String::new();
        let mut finish_reason: Option<String> = None;
        let mut saw_first_chunk = false;
        let mut event_count = 0usize;
        let mut end_reason = "eof";

        let mut thinking_open = false;

        let reader = BufReader::new(response);
        for line in reader.lines() {
            let line = line.context("failed reading streaming response line")?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                end_reason = "done-token";
                break;
            }

            let event: Value =
                serde_json::from_str(data).context("invalid JSON event in stream")?;
            event_count += 1;

            if let Some(choice) = event
                .get("choices")
                .and_then(|choices| choices.as_array())
                .and_then(|choices| choices.first())
            {
                if finish_reason.is_none() {
                    finish_reason = choice
                        .get("finish_reason")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                }

                if let Some(delta) = choice.get("delta") {
                    if let Some(thinking) = delta
                        .get("reasoning_content")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        if !thinking_open {
                            on_chunk(format!("<think>{}", thinking));
                            thinking_open = true;
                        } else {
                            on_chunk(thinking.to_string());
                        }
                    }

                    let content_chunks: Vec<String> = if let Some(text) =
                        delta.get("content").and_then(|v| v.as_str())
                    {
                        if text.is_empty() {
                            vec![]
                        } else {
                            vec![text.to_string()]
                        }
                    } else if let Some(parts) = delta.get("content").and_then(|v| v.as_array()) {
                        parts
                            .iter()
                            .filter_map(|part| {
                                part.get("text")
                                    .and_then(|v| v.as_str())
                                    .filter(|s| !s.is_empty())
                                    .map(|s| s.to_string())
                            })
                            .collect()
                    } else {
                        vec![]
                    };

                    for chunk in content_chunks {
                        if thinking_open {
                            on_chunk("</think>".to_string());
                            thinking_open = false;
                        }

                        if !saw_first_chunk {
                            saw_first_chunk = true;
                            tracing::debug!(
                                "[agent_provider_github_copilot] first content chunk len={}",
                                chunk.len()
                            );
                        }
                        assistant_message.push_str(&chunk);
                        streamed_text_chunks.push(chunk.clone());
                        on_chunk(chunk);
                    }
                }
            }

            raw_events.push(event);

            if finish_reason.is_some() {
                end_reason = "finish-reason";
                break;
            }
        }

        if thinking_open {
            on_chunk("</think>".to_string());
        }

        tracing::debug!(
            "[agent_provider_github_copilot] streaming completed events={} chunks={} \
             assistant_len={} finish_reason={} end_reason={}",
            event_count,
            streamed_text_chunks.len(),
            assistant_message.len(),
            finish_reason.as_deref().unwrap_or("none"),
            end_reason
        );

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
}

impl Default for GithubCopilotProvider {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl ChatProvider for GithubCopilotProvider {
    fn id(&self) -> &str {
        "github_copilot"
    }

    fn display_name(&self) -> &str {
        "GitHub Models"
    }

    fn validate_config(&self) -> anyhow::Result<()> {
        if self.api_key.is_empty() {
            return Err(anyhow::anyhow!("GitHub token is required"));
        }
        // Make a lightweight API call to validate the token
        let response = self
            .client
            .get("https://api.github.com/user")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "Pulsar")
            .send()
            .map_err(|e| anyhow::anyhow!("Failed to connect: {e}"))?;

        if response.status().as_u16() == 200 || response.status().as_u16() == 201 {
            Ok(())
        } else if response.status().as_u16() == 401 {
            Err(anyhow::anyhow!("GitHub token is invalid or expired"))
        } else if response.status().as_u16() == 403 {
            Err(anyhow::anyhow!("GitHub token lacks permission or is rate-limited"))
        } else {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            Err(anyhow::anyhow!("GitHub API returned {status}: {body}"))
        }
    }

    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        let url = "https://models.github.ai/inference/models";
        let response = self
            .client
            .get(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Accept", "application/json")
            .send()
            .map_err(|e| anyhow::anyhow!("Failed to fetch GitHub Models: {e}"))?;
        if !response.status().is_success() {
            return Err(anyhow::anyhow!("GitHub Models API {}: {}", response.status(), response.text().unwrap_or_default()));
        }
        let body: serde_json::Value = response.json()?;
        let models = body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m.get("id")?.as_str()?.to_string();
                        let label = m.get("display_name").or_else(|| m.get("name")).and_then(|v| v.as_str()).unwrap_or(&id).to_string();
                        Some(ModelDescriptor { id, label, supports_tools: true, context_tokens: 0, compact_model: None })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(models)
    }

    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let model = Self::resolve_model(&request.model);
        let payload = Self::build_request_payload(&model, &request, false);

        let response = self
            .client
            .post(GITHUB_MODELS_CHAT_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .json(&payload)
            .send()
            .context("failed to call GitHub Models chat API")?;

        if !response.status().is_success() {
            return Err(Self::format_http_error(response, "chat API"));
        }

        let raw_response: Value = response
            .json()
            .context("invalid JSON from GitHub Models API")?;

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
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(ChatResponse {
            assistant_message,
            streamed_text_chunks,
            tool_calls,
            finish_reason,
            raw_response,
        })
    }

    fn chat_streaming(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let model = Self::resolve_model(&request.model);

        tracing::debug!(
            "[agent_provider_github_copilot] starting streaming request model={}",
            model
        );

        let payload = Self::build_request_payload(&model, &request, true);

        let response = self
            .client
            .post(GITHUB_MODELS_CHAT_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .json(&payload)
            .send()
            .context("failed to call GitHub Models streaming chat API")?;

        if !response.status().is_success() {
            tracing::error!(
                "[agent_provider_github_copilot] streaming request failed status={}",
                response.status()
            );
            return Err(Self::format_http_error(response, "streaming API"));
        }

        Self::read_stream_response(response, on_chunk)
    }
}

use agent_chat_core::{ConfigField, ProviderConfig, ProviderCrate, ProviderEntry, ProviderKind};

pub struct GithubCopilotProviderCrate;

impl ProviderCrate for GithubCopilotProviderCrate {
    fn entries(&self) -> Vec<ProviderEntry> {
        vec![ProviderEntry {
            id: "github_copilot",
            display_name: "GitHub Copilot",
            kind: ProviderKind::Cloud,
            default_endpoint: Some("https://models.github.ai/inference/chat/completions"),
            config_fields: vec![ConfigField {
                key: "token",
                label: "GitHub Token",
                description: "GitHub personal access token with Copilot access",
                sensitive: true,
                required: true,
                placeholder: Some("ghp_..."),
            }],
        }]
    }

    fn create(&self, id: &str, config: ProviderConfig) -> anyhow::Result<Box<dyn ChatProvider>> {
        anyhow::ensure!(id == "github_copilot", "unknown provider: {id}");
        let token = config.get("token").unwrap_or_default().to_string();
        Ok(Box::new(GithubCopilotProvider::new(token)))
    }
}
