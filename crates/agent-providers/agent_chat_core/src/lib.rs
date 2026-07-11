use std::collections::HashMap;
use std::sync::Arc;

// ── Provider registration layer ─────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    Cloud,
    Local,
}

#[derive(Clone, Debug)]
pub struct ConfigField {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub sensitive: bool,
    pub required: bool,
    pub placeholder: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct ProviderConfig {
    pub values: HashMap<String, String>,
}

impl ProviderConfig {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|s| s.as_str())
    }
    pub fn require(&self, key: &str) -> anyhow::Result<&str> {
        self.values.get(key).map(|s| s.as_str()).ok_or_else(|| {
            anyhow::anyhow!("missing required config field: {key}")
        })
    }
}

#[derive(Clone, Debug)]
pub struct ProviderEntry {
    pub id: &'static str,
    pub display_name: &'static str,
    pub kind: ProviderKind,
    pub default_endpoint: Option<&'static str>,
    pub config_fields: Vec<ConfigField>,
}

pub trait ProviderCrate: Send + Sync {
    fn entries(&self) -> Vec<ProviderEntry>;
    fn create(&self, id: &str, config: ProviderConfig) -> anyhow::Result<Box<dyn ChatProvider>>;
}

// ── Runtime chat interface ──────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ModelDescriptor {
    pub id: String,
    pub label: String,
    pub supports_tools: bool,
    pub context_tokens: u32,
    pub compact_model: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
    AgentEvent,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    pub tool_call_id: Option<String>,
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
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn config_fields(&self) -> &[ConfigField] { &[] }
    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>>;
    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse>;

    fn chat_streaming(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let response = self.chat(request)?;
        if response.streamed_text_chunks.is_empty() {
            if let Some(text) = &response.assistant_message {
                on_chunk(text.clone());
            }
        } else {
            for chunk in &response.streamed_text_chunks {
                on_chunk(chunk.clone());
            }
        }
        Ok(response)
    }
}

// ── Provider registry ───────────────────────────────────────────────────────

#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn ChatProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Arc<dyn ChatProvider>) {
        self.providers
            .insert(provider.id().to_string(), provider);
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn ChatProvider>> {
        self.providers.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.providers.contains_key(id)
    }

    pub fn remove(&mut self, id: &str) {
        self.providers.remove(id);
    }

    pub fn all(&self) -> impl Iterator<Item = (&String, &Arc<dyn ChatProvider>)> {
        self.providers.iter()
    }
}
