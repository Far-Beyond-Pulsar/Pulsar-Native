use agent_chat_core::{
    ChatMessage, ChatProvider, ChatRequest, ChatResponse, ChatRole, ModelDescriptor, ToolCall,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_API_VERSION: &str = "2023-06-01";

const ANTHROPIC_MODEL_ENV: &str = "PULSAR_ANTHROPIC_MODEL";

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    models: Vec<ModelDescriptor>,
}

impl AnthropicProvider {
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
                id: "claude-opus-4-5".to_string(),
                label: "Claude Opus 4.5".to_string(),
                supports_tools: true,
                context_tokens: 200_000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "claude-sonnet-4-5".to_string(),
                label: "Claude Sonnet 4.5".to_string(),
                supports_tools: true,
                context_tokens: 200_000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "claude-haiku-4-5".to_string(),
                label: "Claude Haiku 4.5".to_string(),
                supports_tools: true,
                context_tokens: 200_000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "claude-3-7-sonnet-latest".to_string(),
                label: "Claude 3.7 Sonnet".to_string(),
                supports_tools: true,
                context_tokens: 200_000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "claude-3-5-haiku-latest".to_string(),
                label: "Claude 3.5 Haiku".to_string(),
                supports_tools: true,
                context_tokens: 200_000,
                compact_model: None,
            },
        ]
    }

    fn resolve_model(request_model: &str) -> String {
        if let Ok(env_model) = std::env::var(ANTHROPIC_MODEL_ENV) {
            let trimmed = env_model.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        request_model.to_string()
    }

    fn build_messages_and_system(messages: &[ChatMessage]) -> (Vec<Value>, Option<String>) {
        let mut out_messages: Vec<Value> = Vec::new();
        let mut system_parts: Vec<String> = Vec::new();

        for message in messages {
            match message.role {
                ChatRole::System => {
                    system_parts.push(message.content.clone());
                }

                ChatRole::AgentEvent => {
                    system_parts.push(format!("[Engine event]\n{}", message.content));
                }

                ChatRole::User => {
                    out_messages.push(json!({
                        "role": "user",
                        "content": message.content,
                    }));
                }

                ChatRole::Assistant => {
                    if message.tool_calls.is_empty() {
                        out_messages.push(json!({
                            "role": "assistant",
                            "content": message.content,
                        }));
                    } else {
                        let mut content: Vec<Value> = Vec::new();
                        if !message.content.trim().is_empty() {
                            content.push(json!({"type": "text", "text": message.content}));
                        }
                        for call in &message.tool_calls {
                            content.push(json!({
                                "type": "tool_use",
                                "id": call.id,
                                "name": call.name,
                                "input": call.arguments_json,
                            }));
                        }
                        out_messages.push(json!({
                            "role": "assistant",
                            "content": content,
                        }));
                    }
                }

                ChatRole::Tool => {
                    out_messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": message.tool_call_id.as_deref().unwrap_or(""),
                            "content": message.content,
                        }],
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

    fn build_request_payload(model: &str, request: &ChatRequest, stream: bool) -> Value {
        let (messages, system) = Self::build_messages_and_system(&request.messages);

        let mut payload = json!({
            "model": model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(8096),
        });

        if stream {
            payload["stream"] = json!(true);
        }
        if let Some(sys) = &system {
            payload["system"] = json!(sys);
        }
        if let Some(temperature) = request.temperature {
            payload["temperature"] = json!(temperature);
        } else if let Some(top_p) = request.top_p {
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

        payload
    }

    fn parse_assistant_text(raw: &Value) -> Option<String> {
        let content = raw.get("content").and_then(|v| v.as_array())?;

        let mut thinking_parts: Vec<&str> = Vec::new();
        let mut text_parts: Vec<&str> = Vec::new();

        for block in content {
            match block.get("type").and_then(|v| v.as_str()) {
                Some("thinking") => {
                    if let Some(t) = block.get("thinking").and_then(|v| v.as_str()) {
                        if !t.is_empty() {
                            thinking_parts.push(t);
                        }
                    }
                }
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                        if !t.is_empty() {
                            text_parts.push(t);
                        }
                    }
                }
                _ => {}
            }
        }

        let mut result = String::new();
        if !thinking_parts.is_empty() {
            result.push_str("<think>");
            result.push_str(&thinking_parts.join(""));
            result.push_str("</think>");
        }
        result.push_str(&text_parts.join(""));

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    fn parse_tool_calls(raw: &Value) -> Vec<ToolCall> {
        raw.get("content")
            .and_then(|v| v.as_array())
            .map(|content| {
                content
                    .iter()
                    .filter_map(|block| {
                        if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                            return None;
                        }
                        let id = block.get("id")?.as_str()?.to_string();
                        let name = block.get("name")?.as_str()?.to_string();
                        let arguments_json =
                            block.get("input").cloned().unwrap_or_else(|| json!({}));
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

    fn extract_error_message(body: &str) -> Option<String> {
        let parsed: Value = serde_json::from_str(body).ok()?;
        parsed
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn format_http_error(response: reqwest::blocking::Response, api_name: &str) -> anyhow::Error {
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let body = response.text().unwrap_or_default();

        if status.as_u16() == 429 {
            let msg = Self::extract_error_message(&body)
                .unwrap_or_else(|| "Too many requests to Anthropic.".to_string());
            let hint = retry_after
                .as_deref()
                .map(|s| format!(" Retry-After: {s} seconds."))
                .unwrap_or_else(|| " Wait a short time and try again.".to_string());
            return anyhow!("Anthropic rate limit hit (429) during {api_name}. {msg}{hint}");
        }

        let body_display = if body.trim().is_empty() {
            Self::extract_error_message(&body).unwrap_or_else(|| "<empty body>".to_string())
        } else {
            Self::extract_error_message(&body).unwrap_or(body)
        };

        anyhow!("Anthropic {api_name} returned {status}: {body_display}")
    }

    fn read_stream_response(
        response: reqwest::blocking::Response,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        #[derive(Default)]
        struct ToolBlock {
            id: String,
            name: String,
            input_json: String,
        }

        let mut raw_events: Vec<Value> = Vec::new();
        let mut streamed_text_chunks: Vec<String> = Vec::new();
        let mut assistant_message = String::new();
        let mut finish_reason: Option<String> = None;
        let mut tool_blocks: Vec<Option<ToolBlock>> = Vec::new();
        let mut thinking_open = false;

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

            let event: Value =
                serde_json::from_str(data).context("invalid JSON event in Anthropic stream")?;

            match event.get("type").and_then(|v| v.as_str()) {
                Some("content_block_start") => {
                    let index = event.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let block_type = event
                        .get("content_block")
                        .and_then(|b| b.get("type"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    match block_type {
                        "text" => {
                            if thinking_open {
                                on_chunk("</think>".to_string());
                                thinking_open = false;
                            }
                            while tool_blocks.len() <= index {
                                tool_blocks.push(None);
                            }
                        }
                        "thinking" => {
                            while tool_blocks.len() <= index {
                                tool_blocks.push(None);
                            }
                        }
                        "tool_use" => {
                            if thinking_open {
                                on_chunk("</think>".to_string());
                                thinking_open = false;
                            }
                            let id = event
                                .get("content_block")
                                .and_then(|b| b.get("id"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = event
                                .get("content_block")
                                .and_then(|b| b.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            while tool_blocks.len() <= index {
                                tool_blocks.push(None);
                            }
                            tool_blocks[index] = Some(ToolBlock {
                                id,
                                name,
                                input_json: String::new(),
                            });
                        }
                        _ => {}
                    }
                }

                Some("content_block_delta") => {
                    let index = event.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

                    if let Some(delta) = event.get("delta") {
                        match delta.get("type").and_then(|v| v.as_str()) {
                            Some("thinking_delta") => {
                                if let Some(thinking) =
                                    delta.get("thinking").and_then(|v| v.as_str())
                                {
                                    if !thinking.is_empty() {
                                        if !thinking_open {
                                            on_chunk(format!("<think>{}", thinking));
                                            thinking_open = true;
                                        } else {
                                            on_chunk(thinking.to_string());
                                        }
                                    }
                                }
                            }

                            Some("text_delta") => {
                                if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                    if !text.is_empty() {
                                        if thinking_open {
                                            on_chunk("</think>".to_string());
                                            thinking_open = false;
                                        }
                                        let chunk = text.to_string();
                                        assistant_message.push_str(&chunk);
                                        streamed_text_chunks.push(chunk.clone());
                                        on_chunk(chunk);
                                    }
                                }
                            }

                            Some("input_json_delta") => {
                                if let Some(partial) =
                                    delta.get("partial_json").and_then(|v| v.as_str())
                                {
                                    if let Some(Some(block)) = tool_blocks.get_mut(index) {
                                        block.input_json.push_str(partial);
                                    }
                                }
                            }

                            _ => {}
                        }
                    }
                }

                Some("content_block_stop") => {}

                Some("message_delta") => {
                    if finish_reason.is_none() {
                        finish_reason = event
                            .get("delta")
                            .and_then(|d| d.get("stop_reason"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                    }
                }

                Some("message_stop") => {
                    if finish_reason.is_none() {
                        finish_reason = Some("stop".to_string());
                    }
                }

                Some("error") => {
                    let msg = event
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown error");
                    return Err(anyhow!("Anthropic streaming API error: {msg}"));
                }

                _ => {}
            }

            raw_events.push(event);
        }

        if thinking_open {
            on_chunk("</think>".to_string());
        }

        tracing::debug!(
            "[agent_provider_anthropic] streaming completed chunks={} assistant_len={} \
             finish_reason={}",
            streamed_text_chunks.len(),
            assistant_message.len(),
            finish_reason.as_deref().unwrap_or("none"),
        );

        let tool_calls: Vec<ToolCall> = tool_blocks
            .into_iter()
            .flatten()
            .filter(|b| !b.name.is_empty())
            .map(|b| {
                let arguments_json = if b.input_json.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str::<Value>(&b.input_json)
                        .unwrap_or_else(|_| Value::String(b.input_json.clone()))
                };
                ToolCall {
                    id: b.id,
                    name: b.name,
                    arguments_json,
                }
            })
            .collect();

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

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl ChatProvider for AnthropicProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn display_name(&self) -> &str {
        "Anthropic"
    }

    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(self.models.clone())
    }

    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let model = Self::resolve_model(&request.model);
        let payload = Self::build_request_payload(&model, &request, false);

        let response = self
            .client
            .post(ANTHROPIC_MESSAGES_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .context("failed to call Anthropic messages API")?;

        if !response.status().is_success() {
            return Err(Self::format_http_error(response, "messages API"));
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

    fn chat_streaming(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let model = Self::resolve_model(&request.model);

        tracing::debug!(
            "[agent_provider_anthropic] starting streaming request model={}",
            model
        );

        let payload = Self::build_request_payload(&model, &request, true);

        let response = self
            .client
            .post(ANTHROPIC_MESSAGES_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&payload)
            .send()
            .context("failed to call Anthropic streaming API")?;

        if !response.status().is_success() {
            tracing::error!(
                "[agent_provider_anthropic] streaming request failed status={}",
                response.status()
            );
            return Err(Self::format_http_error(response, "streaming API"));
        }

        Self::read_stream_response(response, on_chunk)
    }
}

use agent_chat_core::{ConfigField, ProviderConfig, ProviderCrate, ProviderEntry, ProviderKind};

pub struct AnthropicProviderCrate;

impl ProviderCrate for AnthropicProviderCrate {
    fn entries(&self) -> Vec<ProviderEntry> {
        vec![ProviderEntry {
            id: "anthropic",
            display_name: "Anthropic",
            kind: ProviderKind::Cloud,
            default_endpoint: Some("https://api.anthropic.com/v1/messages"),
            config_fields: vec![ConfigField {
                key: "api_key",
                label: "API Key",
                description: "Your Anthropic API key (sk-ant-...)",
                sensitive: true,
                required: true,
                placeholder: Some("sk-ant-..."),
            }],
        }]
    }

    fn create(&self, id: &str, config: ProviderConfig) -> anyhow::Result<Box<dyn ChatProvider>> {
        anyhow::ensure!(id == "anthropic", "unknown provider: {id}");
        let api_key = config.require("api_key")?.to_string();
        Ok(Box::new(AnthropicProvider::new(api_key)))
    }
}
