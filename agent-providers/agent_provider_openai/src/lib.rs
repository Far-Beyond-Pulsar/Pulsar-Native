use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatMessage, ChatProvider, ChatRequest, ChatResponse,
    ChatRole, ModelDescriptor, PromptTokenRequest, ProviderAvailability, ProviderEnvironment,
    ProviderKind, ProviderMetadata, ToolCall,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

const OPENAI_CHAT_URL: &str = "https://api.openai.com/v1/chat/completions";

pub struct OpenAiProvider {
    client: Client,
}

pub struct OpenAiCompatibleProvider {
    client: Client,
    metadata: ProviderMetadata,
    models: Vec<ModelDescriptor>,
    protocol: CompatibleProtocol,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompatibleProtocol {
    OpenAiCompatible,
    Ollama,
}

impl OpenAiProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "gpt-4.1",
                label: "GPT-4.1",
                supports_tools: true,
            },
            ModelDescriptor {
                id: "gpt-4.1-mini",
                label: "GPT-4.1 Mini",
                supports_tools: true,
            },
            ModelDescriptor {
                id: "gpt-4o",
                label: "GPT-4o",
                supports_tools: true,
            },
            ModelDescriptor {
                id: "o4-mini",
                label: "o4 Mini",
                supports_tools: true,
            },
            ModelDescriptor {
                id: "o3",
                label: "o3",
                supports_tools: true,
            },
        ]
    }

    fn auth_token_from_env(env: &dyn ProviderEnvironment) -> Option<String> {
        env.get_env("OPENAI_API_KEY")
    }

    fn map_role(role: ChatRole) -> &'static str {
        match role {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::Tool => "tool",
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

    fn parse_tool_arguments_value(value: Option<&Value>) -> Value {
        match value {
            Some(Value::String(raw)) => serde_json::from_str::<Value>(raw)
                .unwrap_or_else(|_| Value::String(raw.clone())),
            Some(value) => value.clone(),
            None => json!({}),
        }
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
                        let arguments_json = Self::parse_tool_arguments_value(function.get("arguments"));

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
                                .unwrap_or(partials.len() as u64) as usize;

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

                                if let Some(arguments_fragment) = function
                                    .get("arguments")
                                    .and_then(|value| value.as_str())
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

    fn build_chat_url(endpoint: &str) -> String {
        let trimmed = endpoint.trim_end_matches('/');
        if trimmed.ends_with("/chat/completions") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/chat/completions")
        }
    }

    fn build_ollama_chat_url(endpoint: &str) -> String {
        let trimmed = endpoint.trim_end_matches('/');
        if trimmed.ends_with("/api/chat") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/api/chat")
        }
    }

    fn build_request_payload(request: &ChatRequest) -> Value {
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
                    msg["tool_calls"] = json!(
                        message.tool_calls.iter().map(|call| {
                            json!({
                                "id": call.id,
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": call.arguments_json.to_string(),
                                }
                            })
                        }).collect::<Vec<_>>()
                    );
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
            let line = line.context("failed reading OpenAI-compatible streaming response line")?;
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
                serde_json::from_str(data).context("invalid JSON event in OpenAI-compatible stream")?;

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
                    } else if let Some(parts) = delta.get("content").and_then(|value| value.as_array()) {
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

    fn build_ollama_request_payload(request: &ChatRequest, stream: bool) -> Value {
        let messages = request
            .messages
            .iter()
            .map(|message| {
                let mut msg = serde_json::Map::new();
                msg.insert("role".to_string(), json!(Self::map_role(message.role)));
                msg.insert("content".to_string(), json!(message.content));

                if !message.tool_calls.is_empty() {
                    let ollama_calls = message
                        .tool_calls
                        .iter()
                        .map(|call| {
                            json!({
                                "function": {
                                    "name": call.name,
                                    "arguments": call.arguments_json,
                                }
                            })
                        })
                        .collect::<Vec<_>>();
                    msg.insert("tool_calls".to_string(), Value::Array(ollama_calls));
                }

                Value::Object(msg)
            })
            .collect::<Vec<_>>();

        let mut payload = json!({
            "model": request.model,
            "messages": messages,
            "stream": stream,
        });

        let mut options = serde_json::Map::new();
        if let Some(temperature) = request.temperature {
            options.insert("temperature".to_string(), json!(temperature));
        }
        if let Some(top_p) = request.top_p {
            options.insert("top_p".to_string(), json!(top_p));
        }
        if let Some(max_tokens) = request.max_tokens {
            options.insert("num_predict".to_string(), json!(max_tokens));
        }
        if !options.is_empty() {
            payload["options"] = Value::Object(options);
        }

        if request.enable_tool_calls && !request.tools.is_empty() {
            let tools = request
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
                .collect::<Vec<_>>();
            payload["tools"] = Value::Array(tools);
        }

        payload
    }

    fn parse_ollama_tool_calls(
        message: Option<&Value>,
        next_call_index: &mut usize,
    ) -> Vec<ToolCall> {
        let Some(message) = message else {
            return Vec::new();
        };
        let Some(calls) = message.get("tool_calls").and_then(|value| value.as_array()) else {
            return Vec::new();
        };

        calls
            .iter()
            .filter_map(|call| {
                let function = call.get("function")?;
                let name = function.get("name")?.as_str()?.to_string();

                let arguments_json = match function.get("arguments") {
                    Some(Value::String(raw)) => {
                        serde_json::from_str::<Value>(raw).unwrap_or_else(|_| json!({}))
                    }
                    Some(value) => value.clone(),
                    None => json!({}),
                };

                let id = call
                    .get("id")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| {
                        let generated = format!("ollama_call_{}", *next_call_index);
                        *next_call_index += 1;
                        generated
                    });

                Some(ToolCall {
                    id,
                    name,
                    arguments_json,
                })
            })
            .collect()
    }

    fn parse_ollama_response(raw_response: Value) -> ChatResponse {
        let assistant_message = raw_response
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .map(|content| content.to_string())
            .filter(|content| !content.is_empty());

        let mut next_call_index = 1usize;
        let tool_calls = Self::parse_ollama_tool_calls(
            raw_response.get("message"),
            &mut next_call_index,
        );

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
            .get("done_reason")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .or_else(|| {
                raw_response
                    .get("done")
                    .and_then(|value| value.as_bool())
                    .and_then(|done| if done { Some("stop".to_string()) } else { None })
            });

        ChatResponse {
            assistant_message,
            streamed_text_chunks,
            tool_calls,
            finish_reason,
            raw_response,
        }
    }

    fn read_ollama_stream_response(
        response: reqwest::blocking::Response,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let mut raw_events = Vec::new();
        let mut streamed_text_chunks = Vec::new();
        let mut assistant_message = String::new();
        let mut finish_reason = None;
        let mut tool_calls = Vec::new();
        let mut next_call_index = 1usize;

        let reader = BufReader::new(response);
        for line in reader.lines() {
            let line = line.context("failed reading Ollama streaming response line")?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let event: Value =
                serde_json::from_str(line).context("invalid JSON event in Ollama stream")?;

            if let Some(err) = event.get("error").and_then(|value| value.as_str()) {
                return Err(anyhow!("Ollama streaming API returned error: {}", err));
            }

            if let Some(message) = event.get("message") {
                if let Some(content) = message.get("content").and_then(|value| value.as_str()) {
                    if !content.is_empty() {
                        let chunk = content.to_string();
                        assistant_message.push_str(&chunk);
                        streamed_text_chunks.push(chunk.clone());
                        on_chunk(chunk);
                    }
                }

                let mut calls =
                    Self::parse_ollama_tool_calls(Some(message), &mut next_call_index);
                tool_calls.append(&mut calls);
            }

            if finish_reason.is_none() {
                finish_reason = event
                    .get("done_reason")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string());
            }
            if finish_reason.is_none()
                && event
                    .get("done")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
            {
                finish_reason = Some("stop".to_string());
            }

            let done = event
                .get("done")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            raw_events.push(event);
            if done {
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
            tool_calls,
            finish_reason,
            raw_response: Value::Array(raw_events),
        })
    }
}

impl OpenAiCompatibleProvider {
    fn static_str(value: String) -> &'static str {
        Box::leak(value.into_boxed_str())
    }

    pub fn from_dynamic(
        id: String,
        display_name: String,
        endpoint: String,
        kind: ProviderKind,
        models: Vec<(String, String, bool)>,
    ) -> Self {
        let model_descriptors = models
            .into_iter()
            .map(|(model_id, label, supports_tools)| ModelDescriptor {
                id: Self::static_str(model_id),
                label: Self::static_str(label),
                supports_tools,
            })
            .collect::<Vec<_>>();

        Self {
            client: Client::new(),
            metadata: ProviderMetadata {
                id: Self::static_str(id),
                display_name: Self::static_str(display_name),
                endpoint: Self::static_str(OpenAiProvider::build_chat_url(&endpoint)),
                kind,
            },
            models: model_descriptors,
            protocol: CompatibleProtocol::OpenAiCompatible,
        }
    }

    pub fn from_dynamic_ollama(
        id: String,
        display_name: String,
        endpoint: String,
        kind: ProviderKind,
        models: Vec<(String, String, bool)>,
    ) -> Self {
        let model_descriptors = models
            .into_iter()
            .map(|(model_id, label, supports_tools)| ModelDescriptor {
                id: Self::static_str(model_id),
                label: Self::static_str(label),
                supports_tools,
            })
            .collect::<Vec<_>>();

        Self {
            client: Client::new(),
            metadata: ProviderMetadata {
                id: Self::static_str(id),
                display_name: Self::static_str(display_name),
                endpoint: Self::static_str(OpenAiProvider::build_ollama_chat_url(&endpoint)),
                kind,
            },
            models: model_descriptors,
            protocol: CompatibleProtocol::Ollama,
        }
    }
}

impl Default for OpenAiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for OpenAiProvider {
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "openai",
            display_name: "OpenAI",
            endpoint: OPENAI_CHAT_URL,
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
                "Authentication required. Set OPENAI_API_KEY or paste a token.",
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
                    title: "OpenAI Authentication".to_string(),
                    prompt: "Paste your OpenAI API key.".to_string(),
                    placeholder: Some("sk-...".to_string()),
                    env_var_hint: Some("OPENAI_API_KEY".to_string()),
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
        let payload = Self::build_request_payload(request);

        let response = self
            .client
            .post(OPENAI_CHAT_URL)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .context("failed to call OpenAI chat API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!("OpenAI API returned {}: {}", status, body));
        }

        let raw_response: Value = response.json().context("invalid JSON from OpenAI API")?;
        Ok(Self::parse_non_stream_response(raw_response))
    }

    fn chat_completion_streaming(
        &self,
        token: &str,
        request: &ChatRequest,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let mut payload = Self::build_request_payload(request);
        payload["stream"] = json!(true);

        let response = self
            .client
            .post(OPENAI_CHAT_URL)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&payload)
            .send()
            .context("failed to call OpenAI streaming chat API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!(
                "OpenAI streaming API returned {}: {}",
                status,
                body
            ));
        }

        Self::read_stream_response(response, on_chunk)
    }
}

impl ChatProvider for OpenAiCompatibleProvider {
    fn metadata(&self) -> ProviderMetadata {
        self.metadata.clone()
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        self.models.clone()
    }

    fn availability(&self, _env: &dyn ProviderEnvironment) -> ProviderAvailability {
        ProviderAvailability::ready()
    }

    fn auth_methods(&self) -> Vec<AuthMethod> {
        vec![]
    }

    fn authenticate(
        &self,
        _method: AuthMethod,
        _host: &mut dyn AuthHost,
    ) -> anyhow::Result<AuthResult> {
        Ok(AuthResult::Cancelled)
    }

    fn list_models_api(&self, _token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(self.models.clone())
    }

    fn chat_completion(&self, token: &str, request: &ChatRequest) -> anyhow::Result<ChatResponse> {
        let payload = match self.protocol {
            CompatibleProtocol::OpenAiCompatible => OpenAiProvider::build_request_payload(request),
            CompatibleProtocol::Ollama => {
                OpenAiProvider::build_ollama_request_payload(request, false)
            }
        };

        let mut req = self
            .client
            .post(self.metadata.endpoint)
            .header("Content-Type", "application/json")
            .json(&payload);
        if !token.trim().is_empty() {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        let response = req.send().with_context(|| match self.protocol {
            CompatibleProtocol::OpenAiCompatible => {
                "failed to call OpenAI-compatible chat API".to_string()
            }
            CompatibleProtocol::Ollama => "failed to call Ollama chat API".to_string(),
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            let label = match self.protocol {
                CompatibleProtocol::OpenAiCompatible => "OpenAI-compatible API",
                CompatibleProtocol::Ollama => "Ollama API",
            };
            return Err(anyhow!("{} returned {}: {}", label, status, body));
        }

        let raw_response: Value = response.json().with_context(|| match self.protocol {
            CompatibleProtocol::OpenAiCompatible => {
                "invalid JSON from OpenAI-compatible API".to_string()
            }
            CompatibleProtocol::Ollama => "invalid JSON from Ollama API".to_string(),
        })?;

        Ok(match self.protocol {
            CompatibleProtocol::OpenAiCompatible => {
                OpenAiProvider::parse_non_stream_response(raw_response)
            }
            CompatibleProtocol::Ollama => OpenAiProvider::parse_ollama_response(raw_response),
        })
    }

    fn chat_completion_streaming(
        &self,
        token: &str,
        request: &ChatRequest,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let payload = match self.protocol {
            CompatibleProtocol::OpenAiCompatible => {
                let mut payload = OpenAiProvider::build_request_payload(request);
                payload["stream"] = json!(true);
                payload
            }
            CompatibleProtocol::Ollama => OpenAiProvider::build_ollama_request_payload(request, true),
        };

        let mut req = self
            .client
            .post(self.metadata.endpoint)
            .header("Content-Type", "application/json")
            .json(&payload);
        match self.protocol {
            CompatibleProtocol::OpenAiCompatible => {
                req = req.header("Accept", "text/event-stream");
            }
            CompatibleProtocol::Ollama => {
                req = req.header("Accept", "application/x-ndjson");
            }
        }
        if !token.trim().is_empty() {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        let response = req.send().with_context(|| match self.protocol {
            CompatibleProtocol::OpenAiCompatible => {
                "failed to call OpenAI-compatible streaming chat API".to_string()
            }
            CompatibleProtocol::Ollama => {
                "failed to call Ollama streaming chat API".to_string()
            }
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            let label = match self.protocol {
                CompatibleProtocol::OpenAiCompatible => "OpenAI-compatible streaming API",
                CompatibleProtocol::Ollama => "Ollama streaming API",
            };
            return Err(anyhow!("{} returned {}: {}", label, status, body));
        }

        match self.protocol {
            CompatibleProtocol::OpenAiCompatible => {
                OpenAiProvider::read_stream_response(response, on_chunk)
            }
            CompatibleProtocol::Ollama => {
                OpenAiProvider::read_ollama_stream_response(response, on_chunk)
            }
        }
    }
}
