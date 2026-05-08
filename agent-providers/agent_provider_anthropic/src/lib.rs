use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatMessage, ChatProvider, ChatRequest, ChatResponse,
    ChatRole, ModelDescriptor, PromptTokenRequest, ProviderAvailability, ProviderEnvironment,
    ProviderKind, ProviderMetadata, ToolCall,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    client: Client,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "claude-3-7-sonnet-latest",
                label: "Claude 3.7 Sonnet",
                supports_tools: true,
            },
            ModelDescriptor {
                id: "claude-3-5-sonnet-latest",
                label: "Claude 3.5 Sonnet",
                supports_tools: true,
            },
            ModelDescriptor {
                id: "claude-3-5-haiku-latest",
                label: "Claude 3.5 Haiku",
                supports_tools: true,
            },
        ]
    }

    fn auth_token_from_env(env: &dyn ProviderEnvironment) -> Option<String> {
        env.get_env("ANTHROPIC_API_KEY")
    }

    fn build_messages_and_system(messages: &[ChatMessage]) -> (Vec<Value>, Option<String>) {
        let mut out_messages = Vec::new();
        let mut system_parts = Vec::new();

        for message in messages {
            match message.role {
                ChatRole::System => system_parts.push(message.content.clone()),
                ChatRole::User => out_messages.push(json!({
                    "role": "user",
                    "content": message.content,
                })),
                ChatRole::Assistant => out_messages.push(json!({
                    "role": "assistant",
                    "content": message.content,
                })),
                ChatRole::Tool => {
                    out_messages.push(json!({
                        "role": "user",
                        "content": message.content,
                    }));
                }
            }
        }

        let system = if system_parts.is_empty() {
            None
        } else {
            Some(system_parts.join("\n\n"))
        };

        (out_messages, system)
    }

    fn parse_assistant_text(raw: &Value) -> Option<String> {
        let parts = raw
            .get("content")
            .and_then(|content| content.as_array())
            .map(|content| {
                content
                    .iter()
                    .filter_map(|part| {
                        if part.get("type").and_then(|v| v.as_str()) == Some("text") {
                            part.get("text")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(""))
        }
    }

    fn parse_tool_calls(raw: &Value) -> Vec<ToolCall> {
        raw.get("content")
            .and_then(|content| content.as_array())
            .map(|content| {
                content
                    .iter()
                    .filter_map(|part| {
                        if part.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                            return None;
                        }

                        let id = part.get("id")?.as_str()?.to_string();
                        let name = part.get("name")?.as_str()?.to_string();
                        let arguments_json =
                            part.get("input").cloned().unwrap_or_else(|| json!({}));

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
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for AnthropicProvider {
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "anthropic",
            display_name: "Anthropic",
            endpoint: ANTHROPIC_MESSAGES_URL,
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
                "Authentication required. Set ANTHROPIC_API_KEY or paste a token.",
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
                    title: "Anthropic Authentication".to_string(),
                    prompt: "Paste your Anthropic API key.".to_string(),
                    placeholder: Some("sk-ant-...".to_string()),
                    env_var_hint: Some("ANTHROPIC_API_KEY".to_string()),
                })?;

                Ok(match token {
                    Some(token) => AuthResult::Authenticated { token },
                    None => AuthResult::Cancelled,
                })
            }
            AuthMethod::BrowserDeviceCode => Ok(AuthResult::Cancelled),
        }
    }

    fn list_models_api(&self, _token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(Self::static_models())
    }

    fn chat_completion(&self, token: &str, request: &ChatRequest) -> anyhow::Result<ChatResponse> {
        let (messages, system) = Self::build_messages_and_system(&request.messages);

        let mut payload = json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(1024),
        });

        if let Some(system) = system {
            payload["system"] = json!(system);
        }
        if let Some(temperature) = request.temperature {
            payload["temperature"] = json!(temperature);
        }
        if let Some(top_p) = request.top_p {
            payload["top_p"] = json!(top_p);
        }
        if request.enable_tool_calls && !request.tools.is_empty() {
            payload["tools"] = Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "name": tool.name,
                            "description": tool.description,
                            "input_schema": tool.parameters_json_schema,
                        })
                    })
                    .collect(),
            );
        }

        let response = self
            .client
            .post(ANTHROPIC_MESSAGES_URL)
            .header("x-api-key", token)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .context("failed to call Anthropic messages API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!("Anthropic API returned {}: {}", status, body));
        }

        let raw_response: Value = response.json().context("invalid JSON from Anthropic API")?;
        let assistant_message = Self::parse_assistant_text(&raw_response);
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
            .get("stop_reason")
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
        let (messages, system) = Self::build_messages_and_system(&request.messages);

        let mut payload = json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(1024),
            "stream": true,
        });

        if let Some(system) = system {
            payload["system"] = json!(system);
        }
        if let Some(temperature) = request.temperature {
            payload["temperature"] = json!(temperature);
        }
        if let Some(top_p) = request.top_p {
            payload["top_p"] = json!(top_p);
        }
        if request.enable_tool_calls && !request.tools.is_empty() {
            payload["tools"] = Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "name": tool.name,
                            "description": tool.description,
                            "input_schema": tool.parameters_json_schema,
                        })
                    })
                    .collect(),
            );
        }

        let response = self
            .client
            .post(ANTHROPIC_MESSAGES_URL)
            .header("x-api-key", token)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&payload)
            .send()
            .context("failed to call Anthropic streaming API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!(
                "Anthropic streaming API returned {}: {}",
                status,
                body
            ));
        }

        let mut raw_events = Vec::new();
        let mut streamed_text_chunks = Vec::new();
        let mut assistant_message = String::new();
        let mut finish_reason = None;

        let reader = BufReader::new(response);
        for line in reader.lines() {
            let line = line.context("failed reading Anthropic streaming response line")?;
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
                serde_json::from_str(data).context("invalid JSON event in Anthropic stream")?;

            if let Some(event_type) = event.get("type").and_then(|v| v.as_str()) {
                match event_type {
                    "content_block_delta" => {
                        if let Some(text) = event
                            .get("delta")
                            .and_then(|delta| delta.get("text"))
                            .and_then(|v| v.as_str())
                        {
                            if !text.is_empty() {
                                let chunk = text.to_string();
                                assistant_message.push_str(&chunk);
                                streamed_text_chunks.push(chunk.clone());
                                on_chunk(chunk);
                            }
                        }
                    }
                    "message_delta" => {
                        if finish_reason.is_none() {
                            finish_reason = event
                                .get("delta")
                                .and_then(|delta| delta.get("stop_reason"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                        }
                    }
                    "message_stop" => {
                        if finish_reason.is_none() {
                            finish_reason = Some("stop".to_string());
                        }
                    }
                    _ => {}
                }
            }

            raw_events.push(event);
            if finish_reason.is_some() {
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
            tool_calls: Vec::new(),
            finish_reason,
            raw_response: Value::Array(raw_events),
        })
    }
}
