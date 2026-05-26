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

/// Override the model used for all GitHub Models requests regardless of what the
/// UI selects. Useful for CI / scripted environments.
const GITHUB_MODEL_ENV: &str = "PULSAR_GITHUB_MODEL";

pub struct GithubCopilotProvider {
    client: Client,
}

impl GithubCopilotProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    // ─── Helpers ──────────────────────────────────────────────────────────────

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

    /// Apply `PULSAR_GITHUB_MODEL` env-var override when present and non-empty.
    fn resolve_model(request_model: &str, env: &dyn ProviderEnvironment) -> String {
        if let Some(env_model) = env.get_env(GITHUB_MODEL_ENV) {
            let trimmed = env_model.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        request_model.to_string()
    }

    /// Build the `messages` array common to both streaming and non-streaming
    /// requests.
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
                                    // OpenAI expects a JSON-encoded string here
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

    /// Build the full JSON request payload shared by streaming and non-streaming
    /// paths.  Pass `stream: true` for SSE.
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
            // `max_tokens` works for all models; o-series also accepts
            // `max_completion_tokens` but the alias is universally supported.
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

    // ─── Response parsing ─────────────────────────────────────────────────────

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

    /// Extract assistant message text from a non-streaming response.
    ///
    /// If the response includes a `reasoning_content` field (returned by OpenAI
    /// o-series, DeepSeek-R1, and compatible reasoning models), it is prepended
    /// as `<think>…</think>` so the UI can render it as a collapsible thinking
    /// block — identical to how Ollama surfaces `message.thinking`.
    fn parse_assistant_message(raw: &Value) -> Option<String> {
        let message = raw
            .get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))?;

        // reasoning_content is the OpenAI-compat field for chain-of-thought text.
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
            (Some(thinking), Some(body)) => {
                Some(format!("<think>{}</think>{}", thinking, body))
            }
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

    /// Reconstruct tool calls from accumulated streaming delta events.
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

    // ─── Error helpers ────────────────────────────────────────────────────────

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

    // ─── Dynamic model discovery ──────────────────────────────────────────────

    /// Fetch and parse the GitHub Models catalog, building a `ModelDescriptor`
    /// for every chat-capable entry.  Context window sizes come from the
    /// `model_limits.max_context_window_tokens` field in the catalog response.
    ///
    /// The catalog endpoint is public — pass `token = ""` to fetch without auth
    /// (used by `models()`).  A non-empty token is forwarded as `Bearer` so that
    /// enterprise / private preview models also appear when the user is authed.
    fn fetch_models_from_catalog(&self, token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        let mut builder = self
            .client
            .get(GITHUB_MODELS_CATALOG_URL)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION);

        if !token.is_empty() {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }

        let response = builder
            .send()
            .context("failed to call GitHub Models catalog API")?;
        

        if !response.status().is_success() {
            return Err(Self::format_http_error(response, "catalog API"));
        }

        let raw: Value = response
            .json()
            .context("invalid JSON from GitHub Models catalog")?;

        let items = match raw.as_array().or_else(|| {
            raw.get("models").and_then(|v| v.as_array())
        }) {
            Some(items) => items.clone(),
            None => return Err(anyhow!("GitHub Models catalog returned unexpected shape: {raw}")),
        };

        let mut descriptors: Vec<ModelDescriptor> = items
            .iter()
            .filter_map(|item| {
                // Catalog field layout (confirmed from live API response):
                //   `id`   — inference identifier sent in API requests, e.g. "openai/gpt-4.1"
                //   `name` — human-readable display name,               e.g. "OpenAI GPT-4.1"
                //   `capabilities` — string array; "tool-calling" = function-call support
                //   `limits.max_input_tokens` — context window size in tokens
                let raw_id = item.get("id")?.as_str()?.to_string();

                // Skip entries that don't accept text input (e.g. image/audio-only models).
                let supported_input = item
                    .get("supported_input_modalities")
                    .and_then(|v| v.as_array());
                if let Some(modalities) = supported_input {
                    if !modalities.iter().any(|m| m.as_str() == Some("text")) {
                        return None;
                    }
                }

                // `name` is the display label; fall back to the raw id if absent.
                let display_name = item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&raw_id)
                    .to_string();
                let label = format!("{} (GitHub Models)", display_name);

                // `limits.max_input_tokens` is the usable context window.
                let context_tokens = item
                    .get("limits")
                    .and_then(|limits| limits.get("max_input_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;

                // `capabilities` is a string array; presence of "tool-calling" signals support.
                let supports_tools = item
                    .get("capabilities")
                    .and_then(|v| v.as_array())
                    .map(|caps| caps.iter().any(|c| c.as_str() == Some("tool-calling")))
                    .unwrap_or(false);

                // Leak both strings so we get &'static str as required by the
                // descriptor.  These are bounded by the number of models in the
                // catalog (~dozens), not unbounded allocations.
                let id: &'static str = Box::leak(raw_id.into_boxed_str());
                let label: &'static str = Box::leak(label.into_boxed_str());

                Some(ModelDescriptor {
                    id,
                    label,
                    supports_tools,
                    context_tokens,
                    compact_model: None, // assigned in a second pass below
                })
            })
            .collect();

        if descriptors.is_empty() {
            return Err(anyhow!("GitHub Models catalog returned no text-capable models"));
        }

        // Best-effort compact_model assignment: pair large/reasoning models with
        // small fast sibling when one exists in the same catalog result.
        let ids: Vec<&str> = descriptors.iter().map(|d| d.id).collect();
        // All string literals in preferred slices are 'static, so the cast is
        // sound.  We need the explicit type to satisfy the borrow checker.
        let pick_compact = |preferred: &[&'static str]| -> Option<&'static str> {
            preferred
                .iter()
                .copied()
                .find(|&id| ids.contains(&id))
        };

        for desc in &mut descriptors {
            if desc.compact_model.is_some() {
                continue;
            }
            // Assign a compact sibling based on naming heuristics.
            desc.compact_model = if desc.id.contains("o1")
                || desc.id.contains("o3")
                || desc.id.contains("o4")
                || desc.id.to_lowercase().contains("reasoning")
                || desc.id.contains("r1")
            {
                pick_compact(&[
                    "openai/gpt-4.1-mini",
                    "openai/gpt-4o-mini",
                    "meta/llama-4-scout",
                ])
            } else if desc.id.contains("pro") || desc.id.contains("4.1\"") || desc.id.contains("opus") {
                pick_compact(&[
                    "openai/gpt-4.1-mini",
                    "google/gemini-2.5-flash",
                    "meta/llama-4-scout",
                ])
            } else {
                None
            };
        }

        Ok(descriptors)
    }

    // ─── Streaming response reader ────────────────────────────────────────────

    /// Read a GitHub Models SSE stream, forwarding content chunks via `on_chunk`.
    ///
    /// Thinking / reasoning tokens — emitted by OpenAI o-series, DeepSeek-R1,
    /// and any OpenAI-compatible reasoning model — appear in the `delta` as
    /// `reasoning_content`.  We wrap them in `<think>…</think>` tags so the UI
    /// renders them identically to Ollama's `message.thinking` field.
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

        // Track whether we have an open <think> block so we can close it
        // cleanly when content begins or the stream ends — mirroring the same
        // state machine used in the Ollama provider for `message.thinking`.
        let mut thinking_open = false;

        let reader = BufReader::new(response);
        for line in reader.lines() {
            let line = line.context("failed reading streaming response line")?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // GitHub Models SSE — lines are prefixed "data: …"
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
                // Capture finish_reason when it first appears; we still
                // process the delta of the same event before breaking.
                if finish_reason.is_none() {
                    finish_reason = choice
                        .get("finish_reason")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                }

                if let Some(delta) = choice.get("delta") {
                    // ── Reasoning / thinking tokens ──────────────────────
                    // `reasoning_content` is the OpenAI-compat field used by
                    // o-series, DeepSeek-R1, etc.  Ollama surfaces the same
                    // concept via `message.thinking`; we use the same
                    // `<think>` tag convention for UI consistency.
                    if let Some(thinking) = delta
                        .get("reasoning_content")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        if !thinking_open {
                            // Open the tag with the first token attached.
                            on_chunk(format!("<think>{}", thinking));
                            thinking_open = true;
                        } else {
                            on_chunk(thinking.to_string());
                        }
                    }

                    // ── Content tokens ────────────────────────────────────
                    // Handle both the common string form and the rarer
                    // array-of-parts form (some Claude-via-OpenAI proxies).
                    let content_chunks: Vec<String> =
                        if let Some(text) = delta.get("content").and_then(|v| v.as_str()) {
                            if text.is_empty() {
                                vec![]
                            } else {
                                vec![text.to_string()]
                            }
                        } else if let Some(parts) =
                            delta.get("content").and_then(|v| v.as_array())
                        {
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
                        // Close any open thinking block before emitting prose.
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

        // Ensure the thinking block is always closed, even if content never
        // followed (model produced only reasoning with no final answer).
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

// ─── Trait impl ───────────────────────────────────────────────────────────────

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

    /// Fetch the live catalog without auth — the endpoint is public.
    /// Falls back to an empty list only if the network is unreachable, so the
    /// model picker is populated immediately without requiring the user to
    /// authenticate first (mirroring how Ollama calls `/api/tags`).
    fn models(&self) -> Vec<ModelDescriptor> {
        self.fetch_models_from_catalog("").unwrap_or_default()
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

    /// Fetch the live GitHub Models catalog and return descriptors with
    /// real context-window sizes and tool-support flags from the API.
    fn list_models_api(&self, token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        self.fetch_models_from_catalog(token)
    }

    fn chat_completion(&self, token: &str, request: &ChatRequest) -> anyhow::Result<ChatResponse> {
        let env = agent_chat_core::ProcessEnvironment;
        let model = Self::resolve_model(&request.model, &env);
        let payload = Self::build_request_payload(&model, request, false);

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

        // Simulate streaming chunks from non-streaming response so callers that
        // drive the UI via streamed_text_chunks work without a special case.
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
        let env = agent_chat_core::ProcessEnvironment;
        let model = Self::resolve_model(&request.model, &env);

        tracing::debug!(
            "[agent_provider_github_copilot] starting streaming request model={}",
            model
        );

        let payload = Self::build_request_payload(&model, request, true);

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
            tracing::error!(
                "[agent_provider_github_copilot] streaming request failed status={}",
                response.status()
            );
            return Err(Self::format_http_error(response, "streaming API"));
        }

        Self::read_stream_response(response, on_chunk)
    }
}
