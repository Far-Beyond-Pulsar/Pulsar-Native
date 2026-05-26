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
const ANTHROPIC_MODELS_URL: &str = "https://api.anthropic.com/v1/models";
const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// Override the model used for all Anthropic requests regardless of what the
/// UI selects. Useful for CI / scripted environments.
const ANTHROPIC_MODEL_ENV: &str = "PULSAR_ANTHROPIC_MODEL";

pub struct AnthropicProvider {
    client: Client,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    // ─── Static fallback model list ───────────────────────────────────────────
    // Used before auth is available (models() is called without a token).
    // list_models_api() fetches the live list once a key is known.

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "claude-opus-4-5",
                label: "Claude Opus 4.5",
                supports_tools: true,
                context_tokens: 200_000,
                compact_model: Some("claude-haiku-4-5"),
            },
            ModelDescriptor {
                id: "claude-sonnet-4-5",
                label: "Claude Sonnet 4.5",
                supports_tools: true,
                context_tokens: 200_000,
                compact_model: Some("claude-haiku-4-5"),
            },
            ModelDescriptor {
                id: "claude-haiku-4-5",
                label: "Claude Haiku 4.5",
                supports_tools: true,
                context_tokens: 200_000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "claude-3-7-sonnet-latest",
                label: "Claude 3.7 Sonnet",
                supports_tools: true,
                context_tokens: 200_000,
                compact_model: Some("claude-3-5-haiku-latest"),
            },
            ModelDescriptor {
                id: "claude-3-5-haiku-latest",
                label: "Claude 3.5 Haiku",
                supports_tools: true,
                context_tokens: 200_000,
                compact_model: None,
            },
        ]
    }

    // ─── Helpers ──────────────────────────────────────────────────────────────

    fn auth_token_from_env(env: &dyn ProviderEnvironment) -> Option<String> {
        env.get_env("ANTHROPIC_API_KEY")
    }

    /// Apply `PULSAR_ANTHROPIC_MODEL` env-var override when present and non-empty.
    fn resolve_model(request_model: &str, env: &dyn ProviderEnvironment) -> String {
        if let Some(env_model) = env.get_env(ANTHROPIC_MODEL_ENV) {
            let trimmed = env_model.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        request_model.to_string()
    }

    /// Partition messages into the Anthropic `messages` array and a combined
    /// `system` string.
    ///
    /// Anthropic-specific quirks handled here:
    /// - `System` messages are collected into the top-level `system` field.
    /// - `Tool` (tool-result) messages become `{"role":"user","content":[{"type":"tool_result",...}]}`.
    /// - `Assistant` messages that carry tool calls are sent as a content
    ///   array with `tool_use` blocks, matching what Anthropic returned and
    ///   expects back during multi-turn tool use.
    fn build_messages_and_system(messages: &[ChatMessage]) -> (Vec<Value>, Option<String>) {
        let mut out_messages: Vec<Value> = Vec::new();
        let mut system_parts: Vec<String> = Vec::new();

        for message in messages {
            match message.role {
                ChatRole::System => {
                    system_parts.push(message.content.clone());
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
                        // Build a content-array message: optional text block +
                        // one tool_use block per call.
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
                    // Anthropic expects tool results as a user message containing
                    // a tool_result content block, not a plain string.
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

    /// Build the full JSON request payload.  `stream: true` enables SSE.
    fn build_request_payload(model: &str, request: &ChatRequest, stream: bool) -> (Vec<Value>, Option<String>, Value) {
        let (messages, system) = Self::build_messages_and_system(&request.messages);

        let mut payload = json!({
            "model": model,
            "messages": messages,
            // max_tokens is required by the Anthropic API.
            "max_tokens": request.max_tokens.unwrap_or(8096),
        });

        if stream {
            payload["stream"] = json!(true);
        }
        if let Some(sys) = &system {
            payload["system"] = json!(sys);
        }
        // Anthropic rejects requests that specify both temperature and top_p.
        // Prefer temperature; fall back to top_p only when temperature is absent.
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
                            // Anthropic calls this `input_schema`, not `parameters`.
                            "input_schema": tool.parameters_json_schema,
                        })
                    })
                    .collect(),
            );
        }

        (messages, system, payload)
    }

    // ─── Response parsing (non-streaming) ─────────────────────────────────────

    /// Extract text + thinking from a non-streaming response content array.
    ///
    /// Thinking blocks (`{"type":"thinking","thinking":"..."}`) are wrapped in
    /// `<think>…</think>` so the UI renders them as collapsible reasoning —
    /// identical to how Ollama and GitHub Models surface thinking tokens.
    fn parse_assistant_text(raw: &Value) -> Option<String> {
        let content = raw
            .get("content")
            .and_then(|v| v.as_array())?;

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

        if result.is_empty() { None } else { Some(result) }
    }

    /// Extract tool calls from a non-streaming response content array.
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
                        let arguments_json = block
                            .get("input")
                            .cloned()
                            .unwrap_or_else(|| json!({}));
                        Some(ToolCall { id, name, arguments_json })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    // ─── Error helpers ────────────────────────────────────────────────────────

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

    // ─── Dynamic model discovery ──────────────────────────────────────────────

    /// Fetch the live model list from `GET /v1/models`.
    ///
    /// The Anthropic models endpoint requires a valid API key — the result is
    /// only used by `list_models_api()` (post-auth).  `models()` returns the
    /// static fallback instead.
    fn fetch_models_from_api(&self, token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        let response = self
            .client
            .get(ANTHROPIC_MODELS_URL)
            .header("x-api-key", token)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .send()
            .context("failed to call Anthropic models API")?;

        if !response.status().is_success() {
            return Err(Self::format_http_error(response, "models API"));
        }

        let raw: Value = response
            .json()
            .context("invalid JSON from Anthropic models API")?;

        // Response shape: {"data": [{"id": "claude-...", "display_name": "..."}], ...}
        let items = raw
            .get("data")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("Anthropic models API returned unexpected shape: {raw}"))?;

        if items.is_empty() {
            return Err(anyhow!("Anthropic models API returned empty model list"));
        }

        // Collect IDs first so compact_model pairing can reference the full set.
        let mut descriptors: Vec<ModelDescriptor> = items
            .iter()
            .filter_map(|item| {
                let raw_id = item.get("id")?.as_str()?.to_string();
                let display_name = item
                    .get("display_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&raw_id)
                    .to_string();

                // Anthropic's models API does not return context window sizes;
                // all current Claude models are 200 k.
                let id: &'static str = Box::leak(raw_id.into_boxed_str());
                let label: &'static str = Box::leak(display_name.into_boxed_str());

                Some(ModelDescriptor {
                    id,
                    label,
                    supports_tools: true, // all current Claude models support tools
                    context_tokens: 200_000,
                    compact_model: None,
                })
            })
            .collect();

        // Best-effort compact model pairing by name heuristics.
        let ids: Vec<&str> = descriptors.iter().map(|d| d.id).collect();
        let pick_compact = |preferred: &[&'static str]| -> Option<&'static str> {
            preferred.iter().copied().find(|&id| ids.contains(&id))
        };

        for desc in &mut descriptors {
            if desc.compact_model.is_some() {
                continue;
            }
            desc.compact_model = if desc.id.contains("opus") {
                pick_compact(&["claude-haiku-4-5", "claude-3-5-haiku-latest"])
            } else if desc.id.contains("sonnet") {
                pick_compact(&["claude-haiku-4-5", "claude-3-5-haiku-latest"])
            } else {
                None
            };
        }

        Ok(descriptors)
    }

    // ─── Streaming response reader ────────────────────────────────────────────

    /// Read an Anthropic SSE stream, forwarding content via `on_chunk`.
    ///
    /// Anthropic's streaming format uses typed events:
    /// - `content_block_start` — signals the start of a new content block
    ///   (type `text`, `thinking`, or `tool_use`)
    /// - `content_block_delta` — incremental content (`text_delta`,
    ///   `thinking_delta`, or `input_json_delta`)
    /// - `content_block_stop` — end of a block (finalises tool-call JSON)
    /// - `message_delta` — carries `stop_reason`
    /// - `message_stop` — stream complete
    ///
    /// Thinking blocks are emitted as `<think>…</think>` so the UI renders
    /// them identically to Ollama's `message.thinking` and GitHub Models'
    /// `reasoning_content`.
    fn read_stream_response(
        response: reqwest::blocking::Response,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        // Per-block accumulator for reconstructing streaming tool calls.
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
        let mut tool_blocks: Vec<Option<ToolBlock>> = Vec::new(); // indexed by block index
        let mut thinking_open = false;

        let reader = BufReader::new(response);
        for line in reader.lines() {
            let line = line.context("failed reading Anthropic streaming response line")?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Anthropic SSE: `event:` lines carry the event name (redundant —
            // the type is also in `data`).  We only need the `data:` lines.
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();

            let event: Value =
                serde_json::from_str(data).context("invalid JSON event in Anthropic stream")?;

            match event.get("type").and_then(|v| v.as_str()) {
                // ── New content block ────────────────────────────────────────
                Some("content_block_start") => {
                    let index = event
                        .get("index")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let block_type = event
                        .get("content_block")
                        .and_then(|b| b.get("type"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    match block_type {
                        "text" => {
                            // If we were in a thinking block, close it now.
                            if thinking_open {
                                on_chunk("</think>".to_string());
                                thinking_open = false;
                            }
                            // Ensure index slot exists (non-tool, use None).
                            while tool_blocks.len() <= index {
                                tool_blocks.push(None);
                            }
                        }
                        "thinking" => {
                            // Don't open the tag yet — wait for the first token.
                            while tool_blocks.len() <= index {
                                tool_blocks.push(None);
                            }
                        }
                        "tool_use" => {
                            // Close any open thinking block.
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

                // ── Incremental content ──────────────────────────────────────
                Some("content_block_delta") => {
                    let index = event
                        .get("index")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;

                    if let Some(delta) = event.get("delta") {
                        match delta.get("type").and_then(|v| v.as_str()) {
                            // ── Thinking token ───────────────────────────────
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

                            // ── Text token ───────────────────────────────────
                            Some("text_delta") => {
                                if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                    if !text.is_empty() {
                                        // Close thinking block before first prose.
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

                            // ── Tool call JSON fragment ───────────────────────
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

                // ── Block finished — finalise tool calls ─────────────────────
                Some("content_block_stop") => {
                    // Nothing to do for text/thinking; tool_use is finalised
                    // when we assemble the response at the end.
                }

                // ── Stop reason ──────────────────────────────────────────────
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

        // Ensure any open thinking block is closed.
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

        // Finalise tool calls from accumulated blocks.
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

// ─── Trait impl ───────────────────────────────────────────────────────────────

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

    /// Return static model list — Anthropic's `/v1/models` requires auth so
    /// we can't call it here.  `list_models_api()` fetches the live list once
    /// the user has provided a key.
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

    /// Fetch the live model list from `GET /v1/models` using the stored token.
    fn list_models_api(&self, token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        self.fetch_models_from_api(token)
    }

    fn chat_completion(&self, token: &str, request: &ChatRequest) -> anyhow::Result<ChatResponse> {
        let env = agent_chat_core::ProcessEnvironment;
        let model = Self::resolve_model(&request.model, &env);
        let (_, _, payload) = Self::build_request_payload(&model, request, false);

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
            return Err(Self::format_http_error(response, "messages API"));
        }

        let raw_response: Value = response
            .json()
            .context("invalid JSON from Anthropic API")?;

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
        let env = agent_chat_core::ProcessEnvironment;
        let model = Self::resolve_model(&request.model, &env);

        tracing::debug!(
            "[agent_provider_anthropic] starting streaming request model={}",
            model
        );

        let (_, _, payload) = Self::build_request_payload(&model, request, true);

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
            tracing::error!(
                "[agent_provider_anthropic] streaming request failed status={}",
                response.status()
            );
            return Err(Self::format_http_error(response, "streaming API"));
        }

        Self::read_stream_response(response, on_chunk)
    }
}
