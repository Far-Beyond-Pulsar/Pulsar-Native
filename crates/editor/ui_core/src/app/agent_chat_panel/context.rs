use agent_chat_core::ChatMessage;
use gpui::*;
use ui::scroll::ScrollHandleOffsetable;

use super::panel::AgentChatPanel;
use super::types::*;

impl AgentChatPanel {
    pub(super) const CONTEXT_CHAR_BUDGET: usize = 24_000;
    pub(super) const COMPACTION_SUMMARY_CHAR_BUDGET: usize = 2_400;

    /// Per-model profile overrides live next to the project's other
    /// per-project data. Edit the file to pin a model's context size, tool
    /// calling mode, tier or sampling (see `agent_runtime::ProfileOverride`).
    pub(super) fn profile_overrides_path() -> Option<std::path::PathBuf> {
        let root = engine_state::get_project_path().map(std::path::PathBuf::from)?;
        Some(root.join(".pulsar").join("agent_profiles.json"))
    }

    /// The active model's profile: name-pattern table, then the provider's
    /// descriptor, then the user's override.
    pub(super) fn active_profile(&self) -> agent_runtime::ModelProfile {
        let provider = self.active_provider();
        let provider_id = provider.map(|p| p.id).unwrap_or("");
        let local = provider.is_some_and(|p| p.kind == ProviderKind::Local);
        let Some(model) = self.active_model() else {
            return agent_runtime::resolve_profile(agent_runtime::ResolveInput {
                provider_id,
                local,
                ..Default::default()
            });
        };
        let descriptor = agent_chat_core::ModelDescriptor {
            id: model.id.to_string(),
            label: model.label.to_string(),
            supports_tools: model.supports_tools,
            context_tokens: model.context_tokens,
            compact_model: None,
        };
        agent_runtime::resolve_with_store(
            Self::profile_overrides_path().as_deref(),
            provider_id,
            agent_runtime::ResolveInput {
                provider_id,
                model_id: model.id,
                descriptor: Some(&descriptor),
                local,
                overrides: None,
            },
        )
    }

    pub(super) fn active_context_chars(&self) -> usize {
        let profile = self.active_profile();
        (profile.effective_context() as f32 * profile.tokenizer_hint.chars_per_token()) as usize
    }

    pub(super) fn expand_file_references(text: &str) -> String {
        use std::fs;
        let workspace_root = engine_state::get_project_path()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        let mut injections = Vec::new();
        for word in text.split_whitespace() {
            if !word.starts_with('@') {
                continue;
            }
            let path_str = &word[1..];
            if path_str.is_empty() {
                continue;
            }
            let candidates = [
                std::path::PathBuf::from(path_str),
                workspace_root.join(path_str),
            ];
            for candidate in &candidates {
                if candidate.is_file() {
                    if let Ok(content) = fs::read_to_string(candidate) {
                        let ext = candidate.extension().and_then(|e| e.to_str()).unwrap_or("");
                        let display = candidate.display().to_string();
                        injections.push(format!("```{ext}\n// {display}\n{content}\n```"));
                    }
                    break;
                }
            }
        }

        if injections.is_empty() {
            return text.to_string();
        }
        format!("{}\n\n{}", injections.join("\n\n"), text)
    }

    pub(super) fn scroll_messages_to_bottom(&self) {
        if self.auto_scroll {
            self.messages_scroll_handle.scroll_to_bottom();
        }
    }

    pub(super) fn jump_to_bottom(&mut self) {
        self.auto_scroll = true;
        self.messages_scroll_handle.scroll_to_bottom();
    }

    pub(super) fn distance_from_bottom(&self) -> Pixels {
        let content_h = self.messages_scroll_handle.content_size().height;
        let offset_y = self.messages_scroll_handle.offset().y;
        let viewport_h = self.chat_viewport_height;
        (content_h - offset_y - viewport_h).max(px(0.0))
    }

    pub(super) fn message_row_height(message: &ChatMessage) -> Pixels {
        let explicit_lines = message.content.lines().collect::<Vec<_>>();
        let visual_lines: usize = explicit_lines
            .iter()
            .map(|line| {
                let chars = line.chars().count().max(1);
                chars.div_ceil(64)
            })
            .sum::<usize>()
            .max(1);
        let estimated = 10.0 + 14.0 + 14.0 + (visual_lines as f32 * 18.0) + 6.0;
        px(estimated)
    }

    pub(super) fn format_elapsed(started_ms: u64, finished_ms: Option<u64>, now_ms: u64) -> String {
        if started_ms == 0 {
            return String::new();
        }
        let end = finished_ms.unwrap_or(now_ms);
        let elapsed_ms = end.saturating_sub(started_ms);
        if elapsed_ms < 1_000 {
            format!("{}ms", elapsed_ms)
        } else {
            format!("{:.1}s", elapsed_ms as f32 / 1_000.0)
        }
    }

    pub(super) fn display_item_height(item: &DisplayItem) -> Pixels {
        fn line_height(content: &str, per_line_px: f32, overhead_px: f32) -> Pixels {
            let visual_lines: usize = content
                .lines()
                .map(|line| line.chars().count().max(1).div_ceil(64))
                .sum::<usize>()
                .max(1);
            px(overhead_px + visual_lines as f32 * per_line_px)
        }

        match item {
            DisplayItem::UserMessage { content, .. }
            | DisplayItem::AssistantMessage { content, .. } => line_height(content, 18.0, 44.0),
            DisplayItem::ToolCallGroup {
                calls, is_expanded, ..
            } => {
                if *is_expanded {
                    px(56.0 + calls.len() as f32 * 80.0)
                } else {
                    px(40.0)
                }
            }
            DisplayItem::CompactionSummary {
                summary,
                is_expanded,
                ..
            } => {
                if *is_expanded {
                    line_height(summary, 15.0, 48.0)
                } else {
                    px(36.0)
                }
            }
            DisplayItem::SystemPrompt {
                content,
                is_expanded,
                ..
            } => {
                if *is_expanded {
                    line_height(content, 16.0, 56.0)
                } else {
                    px(40.0)
                }
            }
            DisplayItem::ThinkingBlock {
                content,
                is_expanded,
                ..
            } => {
                if *is_expanded {
                    line_height(content, 16.0, 56.0)
                } else {
                    px(40.0)
                }
            }
            DisplayItem::SubagentInvocation {
                steps, is_expanded, ..
            } => {
                if *is_expanded {
                    px(52.0 + steps.len() as f32 * 75.0)
                } else {
                    px(40.0)
                }
            }
        }
    }
}
