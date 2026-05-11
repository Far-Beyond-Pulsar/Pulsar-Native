use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A single tool call within a `DisplayItem::ToolCallGroup`.
#[derive(Clone, Debug, Serialize, Deserialize)]
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
///
/// `is_streaming` is always written as `false` to disk — it is purely runtime
/// state and should never be restored from a saved file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DisplayItem {
    UserMessage {
        content: String,
        /// Index into `AgentChatPanel::messages` — used by rollback/fork.
        message_index: usize,
    },
    AssistantMessage {
        content: String,
        message_index: usize,
        /// Always `false` when loaded from disk.
        #[serde(default)]
        is_streaming: bool,
    },
    /// Collapsed tool-use block rendered between assistant messages.
    ToolCallGroup {
        calls: Vec<ToolCallDisplay>,
        is_expanded: bool,
    },
    /// Shown when old messages were dropped to fit the context window.
    CompactionSummary { summary: String, is_expanded: bool },
    /// Collapsed thinking/reasoning block rendered before the assistant's reply.
    ThinkingBlock {
        content: String,
        is_expanded: bool,
        /// `false` only during live generation; always `true` on disk.
        #[serde(default = "bool_true")]
        is_done: bool,
    },
    /// The system prompt card — always first in the list, never sent to the AI,
    /// reconstructed from `messages[0]` on load so it is not persisted in
    /// `display_items`.
    #[serde(skip)]
    SystemPrompt {
        content: String,
        is_expanded: bool,
        is_outdated: bool,
    },
}

fn bool_true() -> bool {
    true
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
    /// Maximum context window in tokens. 0 means unknown.
    pub context_tokens: u32,
    /// Cheaper model in the same provider used for context compaction summaries.
    /// `None` → use the current model.
    pub compact_model: Option<&'static str>,
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
    /// Provider history — sent to the AI on every turn.
    pub messages: Vec<PersistedChatMessage>,
    /// UI display items: tool call cards, thinking blocks, and message bubbles.
    /// Not sent to the AI. Absent in files written before this field was added
    /// (falls back to reconstructing from `messages`).
    #[serde(default)]
    pub display_items: Vec<DisplayItem>,
}

#[derive(Clone, Debug)]
pub struct ChatHistoryEntry {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
}
