use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    Cloud,
    Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddProviderPromptStep {
    ProviderId,
    ProviderLabel,
    Endpoint,
    ModelId,
    ModelLabel,
    ModelSupportsTools,
}

#[derive(Clone, Debug, Default)]
pub struct PendingCustomProvider {
    pub id: String,
    pub label: String,
    pub endpoint: String,
    pub model_id: String,
    pub model_label: String,
    pub model_supports_tools: bool,
}

#[derive(Clone, Debug)]
pub struct ModelDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub supports_tools: bool,
}

#[derive(Clone, Debug)]
pub struct ProviderDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: ProviderKind,
    pub endpoint: &'static str,
    pub models: Arc<Vec<ModelDefinition>>,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: &'static str,
    pub content: String,
    pub tool_call_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatSessionFile {
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub messages: Vec<PersistedChatMessage>,
}

#[derive(Clone, Debug)]
pub struct ChatHistoryEntry {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
}
