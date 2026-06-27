use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatMessage, ChatProvider, ChatRequest, ChatResponse,
    ChatRole, ModelDescriptor, PromptTokenRequest, ProviderAvailability, ProviderEnvironment,
    ProviderKind, ProviderMetadata,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

const GEMINI_CHAT_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai/";

pub struct GeminiProvider {
    client: Client,
}

impl GeminiProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "gemini-2.5-pro",
                label: "Gemini 2.5 Pro",
                supports_tools: true,
                context_tokens: 0,
                compact_model: None,
            },
            ModelDescriptor {
                id: "gemini-2.5-flash",
                label: "Gemini 2.5 Flash",
                supports_tools: true,
                context_tokens: 0,
                compact_model: None,
            },
            ModelDescriptor {
                id: "gemini-2.0-flash",
                label: "Gemini 2.0 Flash",
                supports_tools: true,
                context_tokens: 0,
                compact_model: None,
            },
            ModelDescriptor {
                id: "gemini-2.0-flash-lite",
                label: "Gemini 2.0 Flash Lite",
                supports_tools: false,
                context_tokens: 0,
                compact_model: None,
            },
        ]
    }

    fn auth_token_from_env(env: &dyn ProviderEnvironment) -> Option<String> {
        env.get_env("GOOGLE_API_KEY")
            .or_else(|| env.get_env("GEMINI_API_KEY"))
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

    fn parse_assistant_message(raw: &Value) -> Option<String> {
        raw.get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .map(|s| s.to_string())
    }
}

impl Default for GeminiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for GeminiProvider {
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "gemini",
            display_name: "Google Gemini",
            endpoint: GEMINI_CHAT_URL,
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
                "Authentication required. Set GOOGLE_API_KEY or GEMINI_API_KEY.",
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
                    title: "Google Gemini Authentication".to_string(),
                    prompt: "Paste your Google AI Studio API key.".to_string(),
                    placeholder: Some("AIza...".to_string()),
                    env_var_hint: Some("GOOGLE_API_KEY".to_string()),
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
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|message: &ChatMessage| {
                json!({
                    "role": Self::map_role(message.role),
                    "content": message.content,
                })
            })
            .collect();

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

        let url = format!("{}chat/completions?key={}", GEMINI_CHAT_URL, token);
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .context("failed to call Gemini chat API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!("Gemini API returned {}: {}", status, body));
        }

        let raw_response: Value = response.json().context("invalid JSON from Gemini API")?;
        let assistant_message = Self::parse_assistant_message(&raw_response);
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
            tool_calls: Vec::new(),
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
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|message: &ChatMessage| {
                json!({
                    "role": Self::map_role(message.role),
                    "content": message.content,
                })
            })
            .collect();

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

        let url = format!("{}chat/completions?key={}", GEMINI_CHAT_URL, token);
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&payload)
            .send()
            .context("failed to call Gemini streaming chat API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!(
                "Gemini streaming API returned {}: {}",
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
            let line = line.context("failed reading Gemini streaming response line")?;
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
                serde_json::from_str(data).context("invalid JSON event in Gemini stream")?;

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
