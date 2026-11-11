use agent_chat_core::{
    ChatMessage, ChatProvider, ChatRequest, ChatResponse, ChatRole, ModelDescriptor,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::time::Duration;

const GEMINI_CHAT_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai/";

/// The blocking client. `Client::new()` gives every wait 30 s, which the
/// first byte of a long generation exceeds. The blocking client applies
/// `timeout` to each wait for the server (the response, then each body
/// read), not to the whole request, so this keeps a slow stream open as long
/// as the server keeps sending, and still fails a stalled one.
fn http_client() -> anyhow::Result<Client> {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()
        .context("building HTTP client")
}

/// The message of a top-level `error` a server sends as a stream event
/// instead of an HTTP error status.
fn stream_error(event: &Value) -> Option<String> {
    let error = event.get("error")?;
    Some(match error {
        Value::Null => return None,
        Value::String(message) => message.clone(),
        Value::Object(fields) => fields
            .get("message")
            .and_then(|m| m.as_str())
            .map_or_else(|| error.to_string(), str::to_string),
        _ => error.to_string(),
    })
}

pub struct GeminiProvider {
    client: Client,
    api_key: String,
    models: Vec<ModelDescriptor>,
}

impl GeminiProvider {
    pub fn new(api_key: String) -> anyhow::Result<Self> {
        Ok(Self {
            client: http_client()?,
            api_key,
            models: Self::static_models(),
        })
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "gemini-2.5-pro".to_string(),
                label: "Gemini 2.5 Pro".to_string(),
                supports_tools: true,
                context_tokens: 0,
                compact_model: None,
            },
            ModelDescriptor {
                id: "gemini-2.5-flash".to_string(),
                label: "Gemini 2.5 Flash".to_string(),
                supports_tools: true,
                context_tokens: 0,
                compact_model: None,
            },
            ModelDescriptor {
                id: "gemini-2.0-flash".to_string(),
                label: "Gemini 2.0 Flash".to_string(),
                supports_tools: true,
                context_tokens: 0,
                compact_model: None,
            },
            ModelDescriptor {
                id: "gemini-2.0-flash-lite".to_string(),
                label: "Gemini 2.0 Flash Lite".to_string(),
                supports_tools: false,
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

impl ChatProvider for GeminiProvider {
    fn id(&self) -> &str {
        "gemini"
    }

    fn display_name(&self) -> &str {
        "Google Gemini"
    }

    fn validate_config(&self) -> anyhow::Result<()> {
        if self.api_key.is_empty() {
            return Err(anyhow::anyhow!("Gemini API key is required"));
        }
        let url = format!("{}models", GEMINI_CHAT_URL);
        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .map_err(|e| anyhow::anyhow!("Failed to connect to Gemini: {e}"))?;
        if response.status().is_success() {
            Ok(())
        } else if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
            Err(anyhow::anyhow!("Gemini API key is invalid or expired"))
        } else {
            Err(anyhow::anyhow!(
                "Gemini API returned {}: {}",
                response.status(),
                response.text().unwrap_or_default()
            ))
        }
    }

    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        let url = format!("{}models", GEMINI_CHAT_URL);
        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .map_err(|e| anyhow::anyhow!("Failed to fetch Gemini models: {e}"))?;
        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Gemini models API {}: {}",
                response.status(),
                response.text().unwrap_or_default()
            ));
        }
        let body: serde_json::Value = response.json()?;
        let models = body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m.get("id")?.as_str()?.to_string();
                        let label = m
                            .get("display_name")
                            .or_else(|| m.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or(&id)
                            .to_string();
                        Some(ModelDescriptor {
                            id,
                            label,
                            supports_tools: true,
                            context_tokens: 0,
                            compact_model: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(models)
    }

    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
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

        // The key goes in a header, as for `models`: never in the URL, which
        // proxies and logs record.
        let url = format!("{}chat/completions", GEMINI_CHAT_URL);
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
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

    fn chat_streaming(
        &self,
        request: ChatRequest,
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

        let url = format!("{}chat/completions", GEMINI_CHAT_URL);
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
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
            if let Some(message) = stream_error(&event) {
                return Err(anyhow!("Gemini streaming API error: {message}"));
            }

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

use agent_chat_core::{ConfigField, ProviderConfig, ProviderCrate, ProviderEntry, ProviderKind};

pub struct GeminiProviderCrate;

impl ProviderCrate for GeminiProviderCrate {
    fn entries(&self) -> Vec<ProviderEntry> {
        vec![ProviderEntry {
            id: "gemini",
            display_name: "Gemini",
            kind: ProviderKind::Cloud,
            template: false,
            default_endpoint: Some("https://generativelanguage.googleapis.com/v1beta/openai/"),
            config_fields: vec![ConfigField {
                key: "api_key",
                label: "API Key",
                description: "Your Google AI Studio API key",
                sensitive: true,
                required: true,
                placeholder: Some("AIza..."),
            }],
        }]
    }

    fn create(&self, id: &str, config: ProviderConfig) -> anyhow::Result<Box<dyn ChatProvider>> {
        anyhow::ensure!(id == "gemini", "unknown provider: {id}");
        let api_key = config.get("api_key").unwrap_or_default().to_string();
        Ok(Box::new(GeminiProvider::new(api_key)?))
    }
}

// The editor's chat offers this provider (`agent_chat_core::provider_crates`).
pulsar_plugins::inventory::submit! {
    agent_chat_core::AgentProviderRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: "agent.gemini",
        order: 30,
        create: || Box::new(GeminiProviderCrate),
    }
}
