use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatMessage, ChatProvider, ChatRequest, ChatResponse,
    ChatRole, ModelDescriptor, PromptTokenRequest, ProviderAvailability, ProviderEnvironment,
    ProviderKind, ProviderMetadata, ToolCall,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

/// GitHub Models inference endpoint. Requires a fine-grained PAT with `models:read` scope.
const GITHUB_MODELS_CHAT_URL: &str = "https://models.github.ai/inference/chat/completions";
const GITHUB_MODELS_CATALOG_URL: &str = "https://models.github.ai/catalog/models";
const GITHUB_API_VERSION: &str = "2026-03-10";

pub struct GithubCopilotProvider {
    client: Client,
}

impl GithubCopilotProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "openai/gpt-4.1",
                label: "GPT-4.1 (GitHub Models)",
                supports_tools: true,
                context_tokens: 0,
                compact_model: None,
            },
            ModelDescriptor {
                id: "openai/gpt-5-mini",
                label: "GPT-5 mini (GitHub Models)",
                supports_tools: true,
                context_tokens: 0,
                compact_model: None,
            },
            ModelDescriptor {
                id: "anthropic/claude-sonnet-4-6",
                label: "Claude Sonnet 4.6 (GitHub Models)",
                supports_tools: true,
                context_tokens: 0,
                compact_model: None,
            },
            ModelDescriptor {
                id: "google/gemini-2.5-pro",
                label: "Gemini 2.5 Pro (GitHub Models)",
                supports_tools: true,
                context_tokens: 0,
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
        }
    }

    fn auth_token_from_env(env: &dyn ProviderEnvironment) -> Option<String> {
        env.get_env("GITHUB_TOKEN")
            .or_else(|| env.get_env("GH_TOKEN"))
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
        raw.get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .map(|s| s.to_string())
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

                                if let Some(arguments_fragment) =
                                    function.get("arguments").and_then(|v| v.as_str())
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
}

impl Default for GithubCopilotProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for GithubCopilotProvider {
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "github_copilot",
            display_name: "GitHub Models",
            endpoint: GITHUB_MODELS_CHAT_URL,
            kind: ProviderKind::Cloud,
        }
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        Self::static_models()
    }

    fn availability(&self, env: &dyn ProviderEnvironment) -> ProviderAvailability {
        if Self::auth_token_from_env(env).is_some() {
            ProviderAvailability::ready()
        } else {
            ProviderAvailability::requires_auth(
                "Authentication required. Select provider and complete token auth.",
            )
        }
    }

    fn auth_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::PromptToken]
    }

    fn authenticate(
        &self,
        method: AuthMethod,
        host: &mut dyn AuthHost,
    ) -> anyhow::Result<AuthResult> {
        match method {
            AuthMethod::PromptToken => {
                let token = host.prompt_for_token(PromptTokenRequest {
                    title: "GitHub Models Authentication".to_string(),
                    prompt: "Paste a fine-grained PAT with the \"models:read\" scope.".to_string(),
                    placeholder: Some("github_pat_...".to_string()),
                    env_var_hint: Some("GITHUB_TOKEN".to_string()),
                })?;

                Ok(match token {
                    Some(token) => AuthResult::Authenticated { token },
                    None => AuthResult::Cancelled,
                })
            }
            AuthMethod::BrowserDeviceCode => Ok(AuthResult::Cancelled),
        }
    }

    fn list_models_api(&self, token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        let response = self
            .client
            .get(GITHUB_MODELS_CATALOG_URL)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .send()
            .context("failed to call GitHub Models catalog API")?;

        if !response.status().is_success() {
            return Err(Self::format_http_error(response, "catalog API"));
        }

        // The catalog returns an array of model objects; fall back to static list
        // since ModelDescriptor uses &'static str IDs.
        Ok(Self::static_models())
    }

    fn chat_completion(&self, token: &str, request: &ChatRequest) -> anyhow::Result<ChatResponse> {
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
                // Add tool_calls if present (for assistant messages that called tools)
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
            "model": request.model,
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
            payload["tool_choice"] = json!("auto");
        }

        let response = self
            .client
            .post(GITHUB_MODELS_CHAT_URL)
            .header("Authorization", format!("Bearer {token}"))
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

    fn chat_completion_streaming(
        &self,
        token: &str,
        request: &ChatRequest,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        println!(
            "[agent_provider_github_copilot] starting streaming request model={}",
            request.model
        );
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
                // Add tool_calls if present (for assistant messages that called tools)
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
            "model": request.model,
            "messages": messages,
            "stream": true,
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
            payload["tool_choice"] = json!("auto");
        }

        let response = self
            .client
            .post(GITHUB_MODELS_CHAT_URL)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .json(&payload)
            .send()
            .context("failed to call GitHub Models streaming chat API")?;

        if !response.status().is_success() {
            let status = response.status();
            eprintln!(
                "[agent_provider_github_copilot] streaming request failed status={} body_len={}",
                status,
                response.content_length().unwrap_or(0)
            );
            return Err(Self::format_http_error(response, "streaming API"));
        }

        let mut raw_events = Vec::new();
        let mut streamed_text_chunks = Vec::new();
        let mut assistant_message = String::new();
        let mut finish_reason = None;
        let mut saw_first_chunk = false;
        let mut event_count = 0usize;
        let mut end_reason = "eof";

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
                let mut should_finish = false;
                if finish_reason.is_none() {
                    finish_reason = choice
                        .get("finish_reason")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
                if finish_reason.is_some() {
                    should_finish = true;
                }

                if let Some(delta) = choice.get("delta") {
                    if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
                        if !content.is_empty() {
                            let chunk = content.to_string();
                            if !saw_first_chunk {
                                saw_first_chunk = true;
                                println!(
                                    "[agent_provider_github_copilot] first chunk len={}",
                                    chunk.len()
                                );
                            }
                            assistant_message.push_str(&chunk);
                            streamed_text_chunks.push(chunk.clone());
                            on_chunk(chunk);
                        }
                    } else if let Some(parts) = delta.get("content").and_then(|v| v.as_array()) {
                        for part in parts {
                            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                if !text.is_empty() {
                                    let chunk = text.to_string();
                                    if !saw_first_chunk {
                                        saw_first_chunk = true;
                                        println!(
                                            "[agent_provider_github_copilot] first chunk len={}",
                                            chunk.len()
                                        );
                                    }
                                    assistant_message.push_str(&chunk);
                                    streamed_text_chunks.push(chunk.clone());
                                    on_chunk(chunk);
                                }
                            }
                        }
                    }
                }

                raw_events.push(event);
                if should_finish {
                    end_reason = "finish-reason";
                    break;
                }
                continue;
            }

            raw_events.push(event);
        }

        println!(
            "[agent_provider_github_copilot] streaming completed events={} chunks={} assistant_len={} finish_reason={} end_reason={}",
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
