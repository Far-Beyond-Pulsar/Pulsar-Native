//! Builds the `ChatRequest` for a provider call before the worker thread starts.
//!
//! Owns history trimming (budget + AI summary of dropped messages), the live
//! orchestration-state system message, and tool-schema validation, all
//! returned as a plain [`PreparedRequest`]. Entry point:
//! `AgentChatPanel::prepare_provider_request`. The pure helpers
//! ([`build_live_state_message`], [`insert_compaction_summary`],
//! [`insert_system_message`]) are unit tested without any UI.

use super::*;
use agent_chat_core::{ChatRequest, ToolDefinition};
use serde_json::Value;

/// Everything the worker needs to start talking to the provider.
pub(super) struct PreparedRequest {
    /// The first request of the agent loop (history, tools, sampling params).
    pub(super) request: ChatRequest,
    /// Character budget for history; reused for in-loop compaction.
    pub(super) history_budget: usize,
    /// Model used to summarise dropped messages.
    pub(super) compact_model: String,
    /// Auth token for the provider (empty when none is configured).
    pub(super) token: String,
    /// Summary produced when the history had to be compacted up front.
    pub(super) compaction_summary: Option<String>,
}

/// Insert a system message after the leading system messages. When there is
/// no dialog message yet it goes at the end (`fallback_to_end`) or the start.
fn insert_system_message(messages: &mut Vec<ChatMessage>, content: String, fallback_to_end: bool) {
    let fallback = if fallback_to_end { messages.len() } else { 0 };
    let insert_at = messages
        .iter()
        .position(|m| m.role != ChatRole::System)
        .unwrap_or(fallback);
    messages.insert(
        insert_at,
        ChatMessage {
            role: ChatRole::System,
            content,
            tool_call_id: None,
            tool_calls: vec![],
        },
    );
}

/// Insert the AI-written summary of dropped messages before the kept dialog.
pub(super) fn insert_compaction_summary(messages: &mut Vec<ChatMessage>, summary: &str) {
    insert_system_message(
        messages,
        format!("Conversation summary (auto-compacted):\n{summary}"),
        false,
    );
}

/// Format the "Live Agent State" system message from the task manifest and the
/// active sub-agents, or `None` when both are empty.
pub(super) fn build_live_state_message(tasks: &[Value], active_subs: &[Value]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    if !tasks.is_empty() {
        let lines: Vec<String> = tasks
            .iter()
            .map(|t| {
                let status = t.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                let title = t.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                let notes = t.get("notes").and_then(|v| v.as_str()).unwrap_or("");
                if notes.is_empty() {
                    format!("  [{status}] {title}")
                } else {
                    format!("  [{status}] {title} — {notes}")
                }
            })
            .collect();
        parts.push(format!("## Task List\n{}", lines.join("\n")));
    }

    if !active_subs.is_empty() {
        let lines: Vec<String> = active_subs
            .iter()
            .map(|a| {
                let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let status = a.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                let id = a.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let task = a.get("task").and_then(|v| v.as_str()).unwrap_or("");
                format!("  - {name} ({status}, id={id}): {task}")
            })
            .collect();
        parts.push(format!("## Active Sub-Agents\n{}", lines.join("\n")));
    }

    if parts.is_empty() {
        return None;
    }
    Some(format!(
        "## Live Agent State\n_{note}_\n\n{body}",
        note = "Rebuilt each turn — always current",
        body = parts.join("\n\n"),
    ))
}

/// Convert one registry tool to a provider tool definition, or `None` (after
/// logging) when its parameter schema is not a JSON object schema.
fn to_provider_tool(name: String, description: String, params: Value) -> Option<ToolDefinition> {
    if params.get("type").and_then(|v| v.as_str()) != Some("object") {
        tracing::error!(
            "[agent_chat] WARNING: Tool {} has invalid parameters schema, skipping",
            name
        );
        return None;
    }

    Some(ToolDefinition {
        name,
        description: Some(description),
        parameters_json_schema: params,
    })
}

impl AgentChatPanel {
    /// Build the first provider request: trim history to the context budget
    /// (summarising dropped messages), add live orchestration state and attach
    /// the validated tool schemas.
    pub(super) fn prepare_provider_request(
        &self,
        provider: &Arc<dyn ChatProvider>,
        token: Option<String>,
    ) -> PreparedRequest {
        let provider_id = provider.id();
        let model = self
            .active_model()
            .map(|m| m.id.to_string())
            .unwrap_or_else(|| "default".to_string());

        // Leave a sliver for the compaction-instructions block itself.
        let context_chars = self.active_context_chars();
        let history_budget = context_chars.saturating_sub(Self::COMPACTION_SUMMARY_CHAR_BUDGET);

        // Unwrap token early so it can be used for the compaction summary call.
        let token = token.unwrap_or_default();

        // Resolve the compact model: use the specified one or fall back to the current model.
        let compact_model: String = self
            .active_model()
            .and_then(|m| m.compact_model)
            .unwrap_or_else(|| model.as_str())
            .to_string();

        let provider_messages_raw = self.build_provider_history_messages();
        let (mut provider_messages, dropped_opt) = super::prompt_ranking::compact_messages(
            provider_messages_raw,
            history_budget,
            Self::COMPACTION_SUMMARY_CHAR_BUDGET,
        );

        // If messages were dropped, call the compact model to produce a real summary.
        let compaction_summary: Option<String> = if let Some(dropped) = dropped_opt {
            let summary = Self::ai_summarize_dropped(&dropped, provider, &compact_model, &token);
            insert_compaction_summary(&mut provider_messages, &summary);
            Some(summary)
        } else {
            None
        };

        // Inject live orchestration context — task manifest and active sub-agents.
        // This survives compaction because it is rebuilt fresh on every request,
        // so the agent is always oriented even after context shrinks.
        let tasks = agent_chat_tools::get_task_manifest_snapshot();
        let active_subs = agent_chat_tools::get_active_subagents_snapshot();
        if let Some(content) = build_live_state_message(&tasks, &active_subs) {
            insert_system_message(&mut provider_messages, content, true);
        }

        // Validate and convert tool schemas
        let tools: Vec<ToolDefinition> = self
            .tool_registry
            .definitions()
            .into_iter()
            .filter_map(|def| to_provider_tool(def.name, def.description, def.parameters_schema))
            .collect();

        tracing::error!(
            "[agent_chat] Validated {} tools for sending to provider",
            tools.len()
        );

        let request = ChatRequest {
            model,
            messages: provider_messages,
            // Enable tool calls for agentic loop
            enable_tool_calls: !tools.is_empty(),
            tools,
            temperature: Some(0.2),
            top_p: Some(1.0),
            max_tokens: Some(8192),
        };
        tracing::error!(
            "[agent_chat] start provider={} model={} messages={} compacted={}",
            provider_id,
            request.model,
            request.messages.len(),
            compaction_summary.is_some()
        );

        PreparedRequest {
            request,
            history_budget,
            compact_model,
            token,
            compaction_summary,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_live_state_message, insert_compaction_summary, insert_system_message,
        to_provider_tool,
    };
    use agent_chat_core::{ChatMessage, ChatRole};
    use serde_json::json;

    fn msg(role: ChatRole, content: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: content.to_string(),
            tool_call_id: None,
            tool_calls: vec![],
        }
    }

    #[test]
    fn tool_schema_must_be_object() {
        let ok = to_provider_tool("t".into(), "d".into(), json!({"type": "object"})).unwrap();
        assert_eq!(ok.name, "t");
        assert_eq!(ok.description.as_deref(), Some("d"));
        assert!(to_provider_tool("t".into(), "d".into(), json!({"type": "string"})).is_none());
        assert!(to_provider_tool("t".into(), "d".into(), json!({})).is_none());
    }

    #[test]
    fn live_state_is_none_when_nothing_is_active() {
        assert_eq!(build_live_state_message(&[], &[]), None);
    }

    #[test]
    fn live_state_lists_tasks_and_subagents() {
        let tasks = vec![
            json!({"status": "done", "title": "A"}),
            json!({"status": "todo", "title": "B", "notes": "later"}),
        ];
        let subs = vec![json!({"name": "n", "status": "running", "id": "7", "task": "t"})];
        let text = build_live_state_message(&tasks, &subs).unwrap();
        assert!(text.starts_with("## Live Agent State\n_Rebuilt each turn"));
        assert!(text.contains("## Task List\n  [done] A\n  [todo] B — later"));
        assert!(text.contains("## Active Sub-Agents\n  - n (running, id=7): t"));
    }

    #[test]
    fn live_state_uses_placeholders_for_missing_fields() {
        let text = build_live_state_message(&[json!({})], &[]).unwrap();
        assert!(text.contains("  [?] ?"));
    }

    #[test]
    fn system_message_goes_after_leading_system_messages() {
        let mut msgs = vec![msg(ChatRole::System, "sys"), msg(ChatRole::User, "hi")];
        insert_system_message(&mut msgs, "live".into(), true);
        let contents: Vec<_> = msgs.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents, ["sys", "live", "hi"]);
    }

    #[test]
    fn system_message_fallback_differs_when_no_dialog() {
        let mut only_system = vec![msg(ChatRole::System, "sys")];
        insert_system_message(&mut only_system, "end".into(), true);
        assert_eq!(only_system[1].content, "end");

        let mut only_system = vec![msg(ChatRole::System, "sys")];
        insert_system_message(&mut only_system, "front".into(), false);
        assert_eq!(only_system[0].content, "front");
    }

    #[test]
    fn compaction_summary_is_prefixed_and_placed_before_dialog() {
        let mut msgs = vec![msg(ChatRole::System, "sys"), msg(ChatRole::User, "hi")];
        insert_compaction_summary(&mut msgs, "earlier stuff");
        assert_eq!(msgs[1].role, ChatRole::System);
        assert_eq!(
            msgs[1].content,
            "Conversation summary (auto-compacted):\nearlier stuff"
        );
        assert_eq!(msgs[2].content, "hi");
    }
}
