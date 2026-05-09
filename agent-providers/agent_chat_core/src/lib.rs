use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    Cloud,
    Local,
}

#[derive(Clone, Debug)]
pub struct ProviderMetadata {
    pub id: &'static str,
    pub display_name: &'static str,
    pub endpoint: &'static str,
    pub kind: ProviderKind,
}

#[derive(Clone, Debug)]
pub struct ModelDescriptor {
    pub id: &'static str,
    pub label: &'static str,
    pub supports_tools: bool,
}

#[derive(Clone, Debug)]
pub enum AvailabilityState {
    Ready,
    RequiresAuth,
    Wip,
}

#[derive(Clone, Debug)]
pub struct ProviderAvailability {
    pub state: AvailabilityState,
    pub reason: Option<String>,
}

impl ProviderAvailability {
    pub fn ready() -> Self {
        Self {
            state: AvailabilityState::Ready,
            reason: None,
        }
    }

    pub fn requires_auth(reason: impl Into<String>) -> Self {
        Self {
            state: AvailabilityState::RequiresAuth,
            reason: Some(reason.into()),
        }
    }

    pub fn wip(reason: impl Into<String>) -> Self {
        Self {
            state: AvailabilityState::Wip,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMethod {
    PromptToken,
    BrowserDeviceCode,
}

#[derive(Clone, Debug)]
pub struct PromptTokenRequest {
    pub title: String,
    pub prompt: String,
    pub placeholder: Option<String>,
    pub env_var_hint: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OpenBrowserRequest {
    pub url: String,
    pub instructions: String,
    pub code_hint: Option<String>,
}

/// Information returned by the GitHub device code flow's first step.
#[derive(Clone, Debug)]
pub struct DeviceCodeInfo {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Clone, Debug)]
pub enum AuthResult {
    Authenticated { token: String },
    Cancelled,
}

pub trait AuthHost {
    fn prompt_for_token(&mut self, request: PromptTokenRequest) -> anyhow::Result<Option<String>>;

    fn open_browser_for_token(
        &mut self,
        request: OpenBrowserRequest,
    ) -> anyhow::Result<Option<String>>;
}

pub trait ProviderEnvironment {
    fn get_env(&self, key: &str) -> Option<String>;
}

pub struct ProcessEnvironment;

impl ProviderEnvironment for ProcessEnvironment {
    fn get_env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.trim().is_empty())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    /// Tool call ID for tool role messages (used for provider threading)
    pub tool_call_id: Option<String>,
    /// Tool calls for assistant messages (when model decides to call tools)
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters_json_schema: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments_json: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub enable_tool_calls: bool,
    pub tools: Vec<ToolDefinition>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct ChatResponse {
    pub assistant_message: Option<String>,
    pub streamed_text_chunks: Vec<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub raw_response: serde_json::Value,
}

pub trait ChatProvider: Send + Sync {
    fn metadata(&self) -> ProviderMetadata;

    fn models(&self) -> Vec<ModelDescriptor>;

    fn availability(&self, env: &dyn ProviderEnvironment) -> ProviderAvailability;

    fn auth_methods(&self) -> Vec<AuthMethod>;

    fn authenticate(
        &self,
        method: AuthMethod,
        host: &mut dyn AuthHost,
    ) -> anyhow::Result<AuthResult>;

    fn list_models_api(&self, token: &str) -> anyhow::Result<Vec<ModelDescriptor>>;

    fn chat_completion(&self, token: &str, request: &ChatRequest) -> anyhow::Result<ChatResponse>;

    /// Streaming variant for providers that can deliver incremental chunks.
    /// Default behavior falls back to `chat_completion` and emits whatever
    /// chunks are available in the final response.
    fn chat_completion_streaming(
        &self,
        token: &str,
        request: &ChatRequest,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let response = self.chat_completion(token, request)?;

        if response.streamed_text_chunks.is_empty() {
            if let Some(text) = response.assistant_message.clone() {
                on_chunk(text);
            }
        } else {
            for chunk in response.streamed_text_chunks.iter() {
                on_chunk(chunk.clone());
            }
        }

        Ok(response)
    }

    /// Start a GitHub-style OAuth device code flow.  Returns `None` if the
    /// provider does not support this flow.
    fn start_device_flow(&self) -> Option<anyhow::Result<DeviceCodeInfo>> {
        None
    }

    /// Poll the token endpoint once for the given `device_code`.
    /// Returns `Ok(Some(token))` when the user has approved, `Ok(None)` when
    /// still pending, or `Err` on expiry / denial.
    fn poll_device_code(&self, _device_code: &str) -> anyhow::Result<Option<String>> {
        Err(anyhow::anyhow!(
            "device code polling not supported by this provider"
        ))
    }
}

#[derive(Clone, Debug)]
pub struct ProviderCatalogEntry {
    pub metadata: ProviderMetadata,
    pub models: Vec<ModelDescriptor>,
    pub availability: ProviderAvailability,
    pub auth_methods: Vec<AuthMethod>,
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<&'static str, Arc<dyn ChatProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Arc<dyn ChatProvider>) {
        let metadata = provider.metadata();
        self.providers.insert(metadata.id, provider);
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn ChatProvider>> {
        self.providers.get(id)
    }

    pub fn catalog(&self, env: &dyn ProviderEnvironment) -> Vec<ProviderCatalogEntry> {
        self.providers
            .values()
            .map(|provider| ProviderCatalogEntry {
                metadata: provider.metadata(),
                models: provider.models(),
                availability: provider.availability(env),
                auth_methods: provider.auth_methods(),
            })
            .collect()
    }
}
