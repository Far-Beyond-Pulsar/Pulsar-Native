use gpui::*;
use agent_chat_core::{ChatMessage, ChatRole};
use ui::scroll::ScrollHandleOffsetable;

use super::panel::AgentChatPanel;
use super::types::*;

impl AgentChatPanel {
    pub(super) const CONTEXT_CHAR_BUDGET: usize = 24_000;
    pub(super) const COMPACTION_SUMMARY_CHAR_BUDGET: usize = 2_400;

    pub(super) fn active_context_chars(&self) -> usize {
        let tokens = self
            .active_model()
            .and_then(|m| {
                if m.context_tokens > 0 {
                    Some(m.context_tokens as usize)
                } else {
                    Self::infer_context_tokens(m.id)
                }
            })
            .unwrap_or(6_000);
        tokens * 7 / 2
    }

    pub(super) fn infer_context_tokens(id: &str) -> Option<usize> {
        let id = id.to_ascii_lowercase();
        if id.contains("gpt-4.1") {
            return Some(1_047_576);
        }
        if id.contains("gpt-4o") {
            return Some(128_000);
        }
        if id.contains("o4-mini") || id == "o4-mini" {
            return Some(200_000);
        }
        if id == "o3" {
            return Some(200_000);
        }
        if id.contains("gpt-5") {
            return Some(200_000);
        }
        if id.contains("claude") {
            return Some(200_000);
        }
        if id.contains("gemini-2") {
            return Some(1_048_576);
        }
        if id.contains("gemini") {
            return Some(1_048_576);
        }
        if id.contains("codestral") {
            return Some(256_000);
        }
        if id.contains("mistral") || id.contains("ministral") {
            return Some(128_000);
        }
        if id.contains("mixtral") {
            return Some(32_768);
        }
        if id.contains("llama") {
            return Some(131_072);
        }
        if id.contains("qwen") {
            return Some(131_072);
        }
        if id.contains("deepseek-reasoner") {
            return Some(131_072);
        }
        if id.contains("deepseek") {
            return Some(65_536);
        }
        if id.contains("grok") {
            return Some(131_072);
        }
        if id.contains("command-a") {
            return Some(256_000);
        }
        if id.contains("command-r") {
            return Some(128_000);
        }
        if id.contains("sonar") {
            return Some(200_000);
        }
        if id.contains("phi-4") {
            return Some(16_384);
        }
        if id.contains("gemma") {
            return Some(32_768);
        }
        None
    }

    pub(super) fn format_tool_result_preview(tool_name: &str, raw: &str) -> String {
        if tool_name == "web_search" {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) {
                if let Some(results) = json.get("results").and_then(|r| r.as_array()) {
                    let lines: Vec<String> = results
                        .iter()
                        .take(5)
                        .enumerate()
                        .map(|(i, r)| {
                            let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("—");
                            let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
                            let summary = r
                                .get("summary")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .chars()
                                .take(120)
                                .collect::<String>();
                            format!("[{}] {}\n    {}\n    {}", i + 1, title, summary, url)
                        })
                        .collect();
                    return lines.join("\n\n");
                }
            }
        }

        if tool_name == "fetch_url" {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) {
                if let Some(content) = json.get("content").and_then(|v| v.as_str()) {
                    let preview: String = content.chars().take(400).collect();
                    return format!("{}…", preview);
                }
            }
        }

        if raw.len() > 300 {
            format!(
                "{}…",
                &raw[..raw
                    .char_indices()
                    .nth(300)
                    .map(|(i, _)| i)
                    .unwrap_or(raw.len())]
            )
        } else {
            raw.to_string()
        }
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
