//! `OpenAiChatProvider`: sends a chat request and reads the reply in either wire format.

use agent_chat_core::{ChatProvider, ChatRequest, ChatResponse, ModelDescriptor};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::Value;

use crate::ollama_wire::{build_ollama_payload, parse_ollama_response, read_ollama_stream};
use crate::openai_wire::{build_openai_payload, parse_non_stream_response, read_stream_response};

pub(crate) struct OpenAiChatProvider {
    pub(crate) client: Client,
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) chat_url: String,
    pub(crate) models_url: String,
    pub(crate) api_key: String,
    pub(crate) is_ollama: bool,
    /// A hosted API: unreachable means misconfigured, not merely down.
    pub(crate) cloud: bool,
}

impl ChatProvider for OpenAiChatProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn validate_config(&self) -> anyhow::Result<()> {
        if self.api_key.is_empty() {
            return Ok(());
        }
        // Try connecting to the models endpoint — 401/403 means invalid key
        let mut req = self
            .client
            .get(&self.models_url)
            .header("Content-Type", "application/json");
        req = req.header("Authorization", format!("Bearer {}", self.api_key));
        match req.send() {
            Ok(resp) => match resp.status().as_u16() {
                401 | 403 => Err(anyhow::anyhow!("API key is invalid or expired")),
                _ => Ok(()),
            },
            Err(e) if self.cloud => Err(anyhow!("could not reach {}: {e}", self.models_url)),
            Err(e) => {
                // A self-hosted server may be down while its settings are
                // right; the chat reports the failure when it is used.
                tracing::warn!(url = %self.models_url, error = %e, "validate_config: connection failed, assuming valid");
                Ok(())
            }
        }
    }

    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        if self.is_ollama {
            return self.ollama_models();
        }

        let mut req = self
            .client
            .get(&self.models_url)
            .header("Content-Type", "application/json");
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let resp = match req.send() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(url = %self.models_url, error = %e, "models fetch failed, returning empty");
                return Ok(vec![]);
            }
        };
        if !resp.status().is_success() {
            tracing::warn!(url = %self.models_url, status = %resp.status(), "models API returned non-success, returning empty");
            return Ok(vec![]);
        }
        let body_text = resp.text().unwrap_or_default();
        let body: Value = match serde_json::from_str(&body_text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(url = %self.models_url, error = %e, response = %body_text, "models JSON parse failed, returning empty");
                return Ok(vec![]);
            }
        };
        let models = match body {
            Value::Array(ref arr) => Self::parse_model_array(arr),
            Value::Object(_) => body
                .get("data")
                .and_then(|d| d.as_array())
                .map(|arr| Self::parse_model_array(arr))
                .unwrap_or_default(),
            _ => vec![],
        };
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

    fn chat_streaming(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let payload = if self.is_ollama {
            build_ollama_payload(&request, true)
        } else {
            build_openai_payload(&request, true)?
        };

        let mut req = self
            .client
            .post(&self.chat_url)
            .header("Content-Type", "application/json")
            .json(&payload);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        if self.is_ollama {
            req = req.header("Accept", "application/x-ndjson");
        } else {
            req = req.header("Accept", "text/event-stream");
        }

        let response = req
            .send()
            .with_context(|| format!("chat {}", self.chat_url))?;
        if !response.status().is_success() {
            return Err(anyhow!(
                "chat API {}: {}",
                response.status(),
                response.text().unwrap_or_default()
            ));
        }

        if self.is_ollama {
            read_ollama_stream(response, on_chunk)
        } else {
            read_stream_response(response, on_chunk)
        }
    }
}

impl OpenAiChatProvider {
    fn parse_model_array(arr: &[Value]) -> Vec<ModelDescriptor> {
        arr.iter()
            .filter_map(|m| {
                let id = m.get("id")?.as_str()?.to_string();
                let label = m
                    .get("display_name")
                    .or_else(|| m.get("name"))
                    .or_else(|| m.get("id"))
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
    }

    fn send(&self, payload: &Value) -> anyhow::Result<Value> {
        let mut req = self
            .client
            .post(&self.chat_url)
            .header("Content-Type", "application/json")
            .json(payload);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let resp = req
            .send()
            .with_context(|| format!("chat {}", self.chat_url))?;
        if !resp.status().is_success() {
            return Err(anyhow!(
                "chat API {}: {}",
                resp.status(),
                resp.text().unwrap_or_default()
            ));
        }
        Ok(resp.json()?)
    }

    fn ollama_models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        let resp = self
            .client
            .get(&self.models_url)
            .send()
            .with_context(|| format!("ollama tags from {}", self.models_url))?;
        if !resp.status().is_success() {
            return Err(anyhow!(
                "ollama tags API {}: {}",
                resp.status(),
                resp.text().unwrap_or_default()
            ));
        }
        let body: Value = resp.json()?;
        let models = body
            .get("models")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let name = m.get("name").and_then(|v| v.as_str())?.to_string();
                        Some(ModelDescriptor {
                            id: name.clone(),
                            label: name,
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
}
