use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A single tool call within a `DisplayItem::ToolCallGroup`.
#[derive(Clone, Debug)]
pub struct ToolCallDisplay {
    pub id: String,
    pub name: String,
    /// Pre-formatted JSON for display (truncated to ~300 chars).
    pub args_preview: String,
    /// `None` while the tool is still running.
    pub result_preview: Option<String>,
    pub is_error: bool,
}

/// Flat items rendered in the chat virtual list.
/// System and raw Tool-role messages are never added here.
#[derive(Clone, Debug)]
pub enum DisplayItem {
    UserMessage {
        content: String,
        /// Index into `AgentChatPanel::messages` — used by rollback/fork.
        message_index: usize,
    },
    AssistantMessage {
        content: String,
        message_index: usize,
        is_streaming: bool,
    },
    /// Collapsed tool-use block rendered between assistant messages.
    ToolCallGroup {
        calls: Vec<ToolCallDisplay>,
        is_expanded: bool,
    },
    /// Collapsed thinking/reasoning block rendered before the assistant's reply.
    ThinkingBlock {
        content: String,
        is_expanded: bool,
        /// False while the model is still generating the thinking content.
        is_done: bool,
    },
}

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
