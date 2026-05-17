use super::*;
use agent_chat_core::{
    AvailabilityState, ChatMessage, ChatRequest, ChatRole, ProcessEnvironment, ToolCall,
};
use engine_state;
use smol::Timer;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use ui::{input::Enter, scroll::ScrollHandleOffsetable};

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl AgentChatPanel {
    pub(super) const CONTEXT_CHAR_BUDGET: usize = 24_000;
    pub(super) const COMPACTION_SUMMARY_CHAR_BUDGET: usize = 2_400;

    fn build_provider_history_messages(&self) -> Vec<ChatMessage> {
        self.messages
            .iter()
            .filter(|m| !m.content.trim().is_empty())
            .map(|m| ChatMessage {
                role: m.role,
                content: m.content.clone(),
                tool_call_id: m.tool_call_id.clone(),
                tool_calls: m.tool_calls.clone(),
            })
            .collect()
    }

    /// Call the provider with `compact_model` (or the current model) to produce
    /// a concise AI-generated summary of `dropped_messages`.
    /// Falls back to a heuristic snippet summary on any error.
    fn ai_summarize_dropped(
        dropped: &[ChatMessage],
        provider: &Arc<dyn agent_chat_core::ChatProvider>,
        compact_model: &str,
        token: &str,
    ) -> String {
        let formatted: String = dropped
            .iter()
            .take(40)
            .map(|m| {
                let role = match m.role {
                    ChatRole::User => "User",
                    ChatRole::Assistant => "AI",
                    ChatRole::Tool => "Tool",
                    ChatRole::System => "System",
                };
                let snippet: String = m.content.chars().take(600).collect();
                format!("[{role}]: {snippet}")
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let summary_request = agent_chat_core::ChatRequest {
            model: compact_model.to_string(),
            messages: vec![
                ChatMessage {
                    role: ChatRole::System,
                    content: "You are a conversation summarizer. Given a list of messages, \
                              produce a concise summary (max 250 words) that captures: \
                              key decisions, important findings, files or tools used, \
                              and any critical context needed to continue the conversation. \
                              Write in past tense. Be specific, not vague."
                        .to_string(),
                    tool_call_id: None,
                    tool_calls: vec![],
                },
                ChatMessage {
                    role: ChatRole::User,
                    content: format!(
                        "Summarize these earlier conversation messages:\n\n{formatted}"
                    ),
                    tool_call_id: None,
                    tool_calls: vec![],
                },
            ],
            enable_tool_calls: false,
            tools: vec![],
            temperature: Some(0.3),
            top_p: Some(1.0),
            max_tokens: Some(350),
        };

        provider
            .chat_completion(token, &summary_request)
            .ok()
            .and_then(|r| r.assistant_message)
            .unwrap_or_else(|| {
                // Heuristic fallback
                dropped
                    .iter()
                    .take(12)
                    .map(|m| {
                        let role = match m.role {
                            ChatRole::User => "user",
                            ChatRole::Assistant => "assistant",
                            _ => "system",
                        };
                        let snippet: String =
                            m.content.replace('\n', " ").chars().take(180).collect();
                        format!("- {role}: {snippet}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
    }

    /// Scan `text` for `@path` tokens and inject matching file contents as a
    /// fenced context block prepended to the message. Unresolvable tokens are
    /// left as-is. Paths are tried absolute, relative to CWD, and relative to
    /// the workspace root.
    /// Resolve the context window for the active model as a character budget.
    ///
    /// Priority: explicit `context_tokens` on the model → ID-pattern lookup →
    /// conservative fallback of 6 000 tokens (≈ 21 000 chars).
    ///
    /// Uses 3.5 chars/token as the conversion ratio (mix of code and English).
    /// Reserves `COMPACTION_SUMMARY_CHAR_BUDGET` chars as a sliver for the
    /// compaction-instructions message so the bar fills right to the brim.
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

        // 3.5 chars/token — keep as integer math to avoid float noise.
        tokens * 7 / 2
    }

    /// Infer the context window (in tokens) from a model ID string.
    /// Returns `None` if the model is unknown.
    pub(super) fn infer_context_tokens(id: &str) -> Option<usize> {
        let id = id.to_ascii_lowercase();
        // OpenAI
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
        // Anthropic Claude (all recent models are 200k)
        if id.contains("claude") {
            return Some(200_000);
        }
        // Google Gemini
        if id.contains("gemini-2") {
            return Some(1_048_576);
        }
        if id.contains("gemini") {
            return Some(1_048_576);
        }
        // Mistral family
        if id.contains("codestral") {
            return Some(256_000);
        }
        if id.contains("mistral") || id.contains("ministral") {
            return Some(128_000);
        }
        if id.contains("mixtral") {
            return Some(32_768);
        }
        // Meta Llama
        if id.contains("llama") {
            return Some(131_072);
        }
        // Qwen
        if id.contains("qwen") {
            return Some(131_072);
        }
        // DeepSeek
        if id.contains("deepseek-reasoner") {
            return Some(131_072);
        }
        if id.contains("deepseek") {
            return Some(65_536);
        }
        // xAI Grok
        if id.contains("grok") {
            return Some(131_072);
        }
        // Cohere
        if id.contains("command-a") {
            return Some(256_000);
        }
        if id.contains("command-r") {
            return Some(128_000);
        }
        // Perplexity Sonar
        if id.contains("sonar") {
            return Some(200_000);
        }
        // Phi
        if id.contains("phi-4") {
            return Some(16_384);
        }
        // Gemma
        if id.contains("gemma") {
            return Some(32_768);
        }
        None
    }

    /// Produce a human-readable preview for the expanded tool card.
    /// Web-search and fetch_url results get structured formatting; everything
    /// else is truncated plain text.
    fn format_tool_result_preview(tool_name: &str, raw: &str) -> String {
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

    fn expand_file_references(text: &str) -> String {
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

    /// Scroll to the bottom only when `auto_scroll` is active (i.e. the user
    /// has not manually scrolled up). Call this on structural events only —
    /// NOT on every text chunk, which causes jitter.
    pub(super) fn scroll_messages_to_bottom(&self) {
        if self.auto_scroll {
            self.messages_scroll_handle.scroll_to_bottom();
        }
    }

    /// Unconditional scroll to bottom — used by the "jump to bottom" action
    /// which also re-enables auto_scroll.
    pub(super) fn jump_to_bottom(&mut self) {
        self.auto_scroll = true;
        self.messages_scroll_handle.scroll_to_bottom();
    }

    /// Compute the distance from the current scroll position to the bottom of
    /// the content, using the measured viewport height.
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
        // started_at_ms == 0 is the serde default for items loaded from disk that
        // predate timing support — don't show garbage elapsed times for those.
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
        // These estimates are ONLY used for the very first render of each item —
        // before the canvas callback fires with the actual measured height.
        // Streaming items (ThinkingBlock, AssistantMessage) deliberately preserve
        // their cached heights so the virtual list always uses the last real
        // measurement, not a fresh (and potentially wrong) estimate.
        //
        // Caps are intentionally generous: the canvas overrides the value after the
        // first layout, so an oversized estimate is far less harmful than one that
        // is too small (which would cause scroll targets to undershoot the real bottom).
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
                steps,
                is_expanded,
                ..
            } => {
                if *is_expanded {
                    // Header + each step estimated at ~60px + detail content
                    px(52.0 + steps.len() as f32 * 75.0)
                } else {
                    px(40.0)
                }
            }
        }
    }

    pub(super) fn stream_assistant_chunks(
        &mut self,
        chunks: Vec<String>,
        fallback_message: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let chunks = if chunks.is_empty() {
            fallback_message
                .map(|text| vec![text])
                .unwrap_or_else(|| vec!["Provider returned an empty response.".to_string()])
        } else {
            chunks
        };

        let message_ix = self.messages.len();
        self.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![],
        });
        self.scroll_messages_to_bottom();
        cx.notify();

        cx.spawn(async move |this, cx| {
            for chunk in chunks {
                cx.update(|cx| {
                    this.update(cx, |panel, cx| {
                        if let Some(message) = panel.messages.get_mut(message_ix) {
                            message.content.push_str(&chunk);
                        }
                        panel.message_row_heights.remove(&message_ix);
                        panel.save_current_chat();
                        panel.scroll_messages_to_bottom();
                        cx.notify();
                    })
                    .ok();
                })
                .ok();

                Timer::after(Duration::from_millis(14)).await;
            }
        })
        .detach();
    }

    pub(super) fn on_prompt_enter(
        &mut self,
        enter: &Enter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if enter.secondary {
            return;
        }

        if self
            .prompt_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
        {
            self.send_prompt(window, cx);
        }
    }

    fn submit_user_prompt_text(
        &mut self,
        raw_prompt: String,
        prompt_for_model: String,
        display_user_bubble: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let user_message_index = self.messages.len();
        self.messages.push(ChatMessage {
            role: ChatRole::User,
            content: prompt_for_model,
            tool_call_id: None,
            tool_calls: vec![],
        });
        if display_user_bubble {
            self.display_items.push(DisplayItem::UserMessage {
                content: raw_prompt,
                message_index: user_message_index,
            });
        }
        self.scroll_messages_to_bottom();

        let provider_id = self
            .active_provider()
            .map(|p| p.id)
            .unwrap_or("unknown_provider");

        if self.wip_providers.contains_key(provider_id) {
            self.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: "Selected provider is still WIP and not yet executable.".to_string(),
                tool_call_id: None,
                tool_calls: vec![],
            });
            self.save_current_chat();
            self.refresh_chat_history_list(cx);
            self.scroll_messages_to_bottom();
            cx.notify();
            return false;
        }

        let Some(provider) = self.provider_registry.get(provider_id).cloned() else {
            let provider = self
                .active_provider()
                .map(|p| p.label)
                .unwrap_or("Unknown Provider");
            let model = self
                .active_model()
                .map(|m| m.label)
                .unwrap_or("Unknown Model");
            self.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: format!("Queued with {provider} / {model}."),
                tool_call_id: None,
                tool_calls: vec![],
            });
            self.save_current_chat();
            self.refresh_chat_history_list(cx);
            self.scroll_messages_to_bottom();
            cx.notify();
            return false;
        };

        let token = self.auth_token_for_provider(provider_id);
        let availability = provider.availability(&ProcessEnvironment);

        if matches!(availability.state, AvailabilityState::RequiresAuth) && token.is_none() {
            self.pending_auth_provider = Some(provider_id);
            self.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: "Authentication required. Paste token in the auth row above.".to_string(),
                tool_call_id: None,
                tool_calls: vec![],
            });
            self.save_current_chat();
            self.refresh_chat_history_list(cx);
            self.scroll_messages_to_bottom();
            cx.notify();
            return false;
        }

        self.launch_provider_request(provider, token, cx);
        true
    }

    fn update_subagent_invocation_started(
        &mut self,
        subagent_id: &str,
        name: &str,
        task: &str,
        created_at_ms: u64,
        cx: &mut Context<Self>,
    ) {
        if self.display_items.iter().any(|item| {
            matches!(
                item,
                DisplayItem::SubagentInvocation { subagent_id: existing_id, .. } if existing_id == subagent_id
            )
        }) {
            return;
        }

        let step = SubagentStepDisplay {
            id: format!("{subagent_id}:start"),
            description: "Spawned and running".to_string(),
            details: "Subagent execution started. Completion will be queued when ready.".to_string(),
            status: SubagentStepStatus::Running,
            started_at_ms: created_at_ms,
            finished_at_ms: None,
        };

        self.display_items.push(DisplayItem::SubagentInvocation {
            subagent_id: subagent_id.to_string(),
            name: name.to_string(),
            task: task.to_string(),
            steps: vec![step],
            is_expanded: false,
            status: SubagentStepStatus::Running,
            started_at_ms: created_at_ms,
            finished_at_ms: None,
        });
        self.scroll_messages_to_bottom();
        cx.notify();
    }

    fn apply_subagent_completion_event(
        &mut self,
        event: &serde_json::Value,
        queue_depth_after_enqueue: usize,
        cx: &mut Context<Self>,
    ) {
        let subagent_id = event
            .get("subagent_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if subagent_id.is_empty() {
            return;
        }

        let finished_at_ms = event
            .get("finished_at_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or_else(now_ms);
        let status_raw = event
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("success");
        let status = match status_raw {
            "error" => SubagentStepStatus::Error,
            "cancelled" => SubagentStepStatus::Error,
            _ => SubagentStepStatus::Success,
        };

        for item in self.display_items.iter_mut() {
            if let DisplayItem::SubagentInvocation {
                subagent_id: existing_id,
                steps,
                status: overall,
                finished_at_ms: card_finished_at_ms,
                ..
            } = item
            {
                if existing_id == subagent_id {
                    if let Some(last) = steps.last_mut() {
                        if last.finished_at_ms.is_none() {
                            last.status = status;
                            last.finished_at_ms = Some(finished_at_ms);
                        }
                    }

                    let details = format!(
                        "Queued for main-agent processing. {} completion(s) waiting.",
                        queue_depth_after_enqueue.max(1)
                    );
                    steps.push(SubagentStepDisplay {
                        id: format!("{subagent_id}:queued"),
                        description: "Waiting for main agent".to_string(),
                        details,
                        status: SubagentStepStatus::Pending,
                        started_at_ms: finished_at_ms,
                        finished_at_ms: None,
                    });
                    *overall = status;
                    *card_finished_at_ms = Some(finished_at_ms);
                    break;
                }
            }
        }

        let name = event
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("subagent");
        let queue_text = if queue_depth_after_enqueue > 1 {
            format!(
                "Subagent '{}' finished and joined the completion queue ({} waiting).",
                name, queue_depth_after_enqueue
            )
        } else {
            format!("Subagent '{}' finished and is waiting for main-agent processing.", name)
        };

        let assistant_ix = self.messages.len();
        self.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content: queue_text.clone(),
            tool_call_id: None,
            tool_calls: vec![],
        });
        self.display_items.push(DisplayItem::AssistantMessage {
            content: queue_text,
            message_index: assistant_ix,
            is_streaming: false,
        });
        self.scroll_messages_to_bottom();
        cx.notify();
    }

    pub(super) fn maybe_start_next_subagent_processing(&mut self, cx: &mut Context<Self>) {
        if self.is_request_in_flight || self.is_processing_subagent_event {
            return;
        }
        let prompt_text = self.prompt_input.read(cx).text().to_string();
        if !prompt_text.trim().is_empty() {
            return;
        }

        let Some(event) = self.pending_subagent_events.pop_front() else {
            return;
        };

        let subagent_id = event
            .get("subagent_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown-subagent");
        let name = event
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Subagent");
        let status = event
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("success");

        for item in self.display_items.iter_mut() {
            if let DisplayItem::SubagentInvocation {
                subagent_id: existing_id,
                steps,
                ..
            } = item
            {
                if existing_id == subagent_id {
                    if let Some(last) = steps.last_mut() {
                        if last.description == "Waiting for main agent" {
                            last.status = SubagentStepStatus::Running;
                            last.description = "Main agent processing completion".to_string();
                            last.details =
                                "Main agent lock acquired for this completion.".to_string();
                        }
                    }
                    break;
                }
            }
        }

        self.is_processing_subagent_event = true;
        self.processing_subagent_id = Some(subagent_id.to_string());

        let result_preview = agent_chat_tools::get_subagent_result_preview(subagent_id);
        let preview_block = match &result_preview {
            Some(preview) => format!("\n\nFindings preview:\n{preview}"),
            None => String::new(),
        };
        let detail_hint = if result_preview.is_some() {
            format!(
                "Call get_subagent_result(\"{subagent_id}\") for the full transcript and file references if needed. "
            )
        } else {
            String::new()
        };

        let event_content = format!(
            "Sub-agent '{name}' (id={subagent_id}) completed — status: {status}.{preview_block}\n\n\
             Integrate these findings into the current work. \
             {detail_hint}\
             Update your task list with task_list_update if any tasks changed state."
        );

        let launched = self.launch_internal_agent_event(event_content, cx);
        if !launched {
            self.is_processing_subagent_event = false;
            self.processing_subagent_id = None;
        }
    }

    /// Deliver an internal engine event to the model without using ChatRole::User.
    ///
    /// Injects a synthetic Assistant message (containing an implicit
    /// `check_subagent_completions` tool call) followed by the event payload as a
    /// ChatRole::Tool result. The model sees this as the outcome of a tool it called,
    /// not as a human speaking — eliminating the role-confusion bug where the model
    /// describes "what the user said" instead of acting on the event.
    ///
    /// This matches the pattern used by Claude Code: async sub-agent results arrive
    /// as Tool role messages, never as User turns.
    fn launch_internal_agent_event(
        &mut self,
        content: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let provider_id = self
            .active_provider()
            .map(|p| p.id)
            .unwrap_or("unknown_provider");

        if self.wip_providers.contains_key(provider_id) {
            return false;
        }

        let Some(provider) = self.provider_registry.get(provider_id).cloned() else {
            return false;
        };

        let token = self.auth_token_for_provider(provider_id);
        let availability = provider.availability(&ProcessEnvironment);
        if matches!(availability.state, AvailabilityState::RequiresAuth) && token.is_none() {
            return false;
        }

        // Unique call ID so compaction keeps this assistant→tool pair intact.
        let call_id = format!("_internal_event_{}", now_ms());

        // The assistant "called" check_subagent_completions — an empty-content
        // assistant message with tool_calls is valid for all major providers.
        self.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![ToolCall {
                id: call_id.clone(),
                name: "check_subagent_completions".to_string(),
                arguments_json: serde_json::json!({}),
            }],
        });

        // The tool result is the actual event payload the model should act on.
        self.messages.push(ChatMessage {
            role: ChatRole::Tool,
            content,
            tool_call_id: Some(call_id),
            tool_calls: vec![],
        });

        self.launch_provider_request(provider, token, cx);
        true
    }

    pub(super) fn process_next_subagent_completion_now(&mut self, cx: &mut Context<Self>) {
        if self.is_request_in_flight || self.is_processing_subagent_event {
            return;
        }
        self.maybe_start_next_subagent_processing(cx);
    }

    fn complete_active_subagent_processing(&mut self, success: bool) {
        let Some(subagent_id) = self.processing_subagent_id.clone() else {
            return;
        };

        for item in self.display_items.iter_mut() {
            if let DisplayItem::SubagentInvocation {
                subagent_id: existing_id,
                steps,
                ..
            } = item
            {
                if *existing_id == subagent_id {
                    if let Some(last) = steps.last_mut() {
                        if last.description == "Main agent processing completion" {
                            last.status = if success {
                                SubagentStepStatus::Success
                            } else {
                                SubagentStepStatus::Error
                            };
                            last.details = if success {
                                "Main agent processed this completion.".to_string()
                            } else {
                                "Main agent processing failed; completion remains visible.".to_string()
                            };
                            last.finished_at_ms = Some(now_ms());
                        }
                    }
                    break;
                }
            }
        }
    }

    pub(super) fn poll_subagent_completion_events(&mut self, cx: &mut Context<Self>) {
        let mut enqueued = 0usize;
        while let Some(event) = agent_chat_tools::dequeue_subagent_completion_event() {
            self.pending_subagent_events.push_back(event.clone());
            enqueued += 1;
            self.apply_subagent_completion_event(&event, self.pending_subagent_events.len(), cx);
        }

        if enqueued > 0 {
            self.save_current_chat();
            self.refresh_chat_history_list(cx);
        }

        if self.subagent_completion_mode == SubagentCompletionMode::Auto {
            self.maybe_start_next_subagent_processing(cx);
        }
    }

    pub(super) fn send_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_request_in_flight {
            return;
        }

        let raw_prompt = self.prompt_input.read(cx).text().to_string();
        let raw_prompt = raw_prompt.trim().to_string();
        if raw_prompt.is_empty() {
            return;
        }

        // @file injection: resolve `@/some/path` or `@filename` references and
        // prepend their contents as a context block before the user's message.
        let prompt = Self::expand_file_references(&raw_prompt);

        self.prompt_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        let _ = self.submit_user_prompt_text(raw_prompt, prompt, true, cx);
    }

    /// Push an empty streaming assistant bubble and fire off the provider request.
    /// Called by `send_prompt` (after the user message is pushed) and by
    /// `regenerate_response` (after the old assistant message is removed).
    pub(super) fn launch_provider_request(
        &mut self,
        provider: Arc<dyn agent_chat_core::ChatProvider>,
        token: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let message_ix = self.messages.len();
        self.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![],
        });
        let streaming_dix = self.display_items.len();
        self.display_items.push(DisplayItem::AssistantMessage {
            content: String::new(),
            message_index: message_ix,
            is_streaming: true,
        });
        self.is_request_in_flight = true;
        self.streaming_message_ix = Some(message_ix);
        self.streaming_display_item_ix = Some(streaming_dix);
        self.auto_scroll = true; // always follow the bottom for a new request

        let provider_id = provider.metadata().id;
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
        let (mut provider_messages, dropped_opt) =
            super::prompt_ranking::compact_messages(
                provider_messages_raw,
                history_budget,
                Self::COMPACTION_SUMMARY_CHAR_BUDGET,
            );

        // If messages were dropped, call the compact model to produce a real summary.
        let initial_compaction_summary: Option<String> = if let Some(dropped) = dropped_opt {
            let summary = Self::ai_summarize_dropped(&dropped, &provider, &compact_model, &token);
            // Insert the AI summary as a system message at the top of kept history.
            provider_messages.insert(
                // After system messages, before dialog
                provider_messages
                    .iter()
                    .position(|m| m.role != ChatRole::System)
                    .unwrap_or(0),
                ChatMessage {
                    role: ChatRole::System,
                    content: format!("Conversation summary (auto-compacted):\n{summary}"),
                    tool_call_id: None,
                    tool_calls: vec![],
                },
            );
            Some(summary)
        } else {
            None
        };

        // Inject live orchestration context — task manifest and active sub-agents.
        // This survives compaction because it is rebuilt fresh on every request,
        // so the agent is always oriented even after context shrinks.
        {
            let tasks = agent_chat_tools::get_task_manifest_snapshot();
            let active_subs = agent_chat_tools::get_active_subagents_snapshot();
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

            if !parts.is_empty() {
                let content = format!(
                    "## Live Agent State\n_{note}_\n\n{body}",
                    note = "Rebuilt each turn — always current",
                    body = parts.join("\n\n"),
                );
                let insert_at = provider_messages
                    .iter()
                    .position(|m| m.role != ChatRole::System)
                    .unwrap_or(provider_messages.len());
                provider_messages.insert(
                    insert_at,
                    ChatMessage {
                        role: ChatRole::System,
                        content,
                        tool_call_id: None,
                        tool_calls: vec![],
                    },
                );
            }
        }

        // Validate and convert tool schemas
        let tools: Vec<agent_chat_core::ToolDefinition> = self
            .tool_registry
            .definitions()
            .into_iter()
            .filter_map(|def| {
                let params = def.parameters_schema;

                if params.get("type").and_then(|v| v.as_str()) != Some("object") {
                    tracing::error!(
                        "[agent_chat] WARNING: Tool {} has invalid parameters schema, skipping",
                        def.name
                    );
                    return None;
                }

                Some(agent_chat_core::ToolDefinition {
                    name: def.name,
                    description: Some(def.description),
                    parameters_json_schema: params,
                })
            })
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
            initial_compaction_summary.is_some()
        );

        enum StreamEvent {
            /// AI prose text chunk — appended to the current streaming assistant bubble.
            Chunk(String),
            /// A `<think>` tag was opened; UI inserts a ThinkingBlock.
            ThinkingStarted,
            /// Incremental thinking content — appended to the open ThinkingBlock.
            ThinkingChunk(String),
            /// `</think>` was reached; UI marks the ThinkingBlock done.
            ThinkingDone,
            /// The AI returned tool calls; UI renders a collapsed tool call group
            /// and starts a new streaming assistant bubble for the next iteration.
            ToolCallGroup(Vec<ToolCall>),
            /// A tool finished executing; update the matching call's result in the UI.
            ToolCallResult {
                id: String,
                tool_name: String,
                result_preview: String,
                result_full: String,
                is_error: bool,
            },
            /// Old messages were dropped inside the agentic loop to stay within context.
            ContextCompacted(String),
            OpenFile(PathBuf, Arc<(Mutex<Option<Result<(), String>>>, Condvar)>),
            ActivateOpenEditor(usize, Arc<(Mutex<Option<Result<(), String>>>, Condvar)>),
            Finished(Result<agent_chat_core::ChatResponse, String>),
        }

        let (tx, rx) = smol::channel::unbounded::<StreamEvent>();
        let tx_for_chunks = tx.clone();
        let tx_for_finish = tx.clone();
        let provider_for_task = provider.clone();
        let tool_registry_for_task = self.tool_registry.clone();
        let tool_registry_for_subagent = self.tool_registry.clone();
        let plugin_bridge_for_task = self.plugin_bridge.clone();
        let completion_sent = Arc::new(AtomicBool::new(false));

        // Handle initial compaction synchronously — insert the summary card before
        // the streaming bubble so the user sees the context was trimmed.
        if let Some(summary) = initial_compaction_summary {
            let t = now_ms();
            if let Some(dix) = self.streaming_display_item_ix {
                if dix + 1 == self.display_items.len() {
                    let bubble = self.display_items.pop().unwrap();
                    self.display_items.push(DisplayItem::CompactionSummary {
                        summary,
                        is_expanded: false,
                        started_at_ms: t,
                        finished_at_ms: Some(t),
                    });
                    self.display_items.push(bubble);
                    self.streaming_display_item_ix = Some(dix + 1);
                }
            }
        }

        // Per-request cancel channel — UI sends () to abort.
        let (cancel_tx, cancel_rx) = smol::channel::bounded::<()>(1);
        self.cancel_tx = Some(cancel_tx);

        let context_budget = history_budget;
        let compact_model_for_worker = compact_model.clone(); // used for in-loop AI summarization
        let completion_for_worker = completion_sent.clone();
        std::thread::spawn(move || {
            let mut current_messages = request.messages.clone();
            let mut iteration = 0u32;
            const MAX_ITERATIONS: u32 = 50;

            loop {
                // Check for user cancellation between iterations.
                if cancel_rx.try_recv().is_ok() {
                    if !completion_for_worker.swap(true, Ordering::SeqCst) {
                        let _ = tx_for_finish
                            .try_send(StreamEvent::Finished(Err("Request cancelled.".to_string())));
                    }
                    break;
                }

                if iteration >= MAX_ITERATIONS {
                    tracing::error!("[agent_chat] max iterations ({MAX_ITERATIONS}) reached");
                    // Treat hitting the limit as a clean finish so the UI isn't left hanging.
                    if !completion_for_worker.swap(true, Ordering::SeqCst) {
                        let _ = tx_for_finish.try_send(StreamEvent::Finished(Err(format!(
                            "Reached the {MAX_ITERATIONS}-iteration limit."
                        ))));
                    }
                    break;
                }
                iteration += 1;

                let mut current_request = ChatRequest {
                    model: request.model.clone(),
                    messages: current_messages.clone(),
                    enable_tool_calls: request.enable_tool_calls,
                    tools: request.tools.clone(),
                    temperature: request.temperature,
                    top_p: request.top_p,
                    max_tokens: request.max_tokens,
                };

                let mut pending_chunk = String::new();
                let mut pending_thinking = String::new();
                let mut last_emit = Instant::now();
                let mut last_think_emit = Instant::now();
                let mut in_thinking = false;

                let mut on_chunk = |chunk: String| {
                    let mut rest: &str = &chunk;
                    loop {
                        if in_thinking {
                            if let Some(end) = rest.find("</think>") {
                                pending_thinking.push_str(&rest[..end]);
                                if !pending_thinking.is_empty() {
                                    let _ = tx_for_chunks.try_send(StreamEvent::ThinkingChunk(
                                        std::mem::take(&mut pending_thinking),
                                    ));
                                }
                                let _ = tx_for_chunks.try_send(StreamEvent::ThinkingDone);
                                in_thinking = false;
                                rest = &rest[end + "</think>".len()..];
                                if rest.is_empty() {
                                    break;
                                }
                            } else {
                                pending_thinking.push_str(rest);
                                // Emit thinking in small batches so the block fills progressively.
                                let should_emit = pending_thinking.len() >= 128
                                    || last_think_emit.elapsed() >= Duration::from_millis(30);
                                if should_emit {
                                    let _ = tx_for_chunks.try_send(StreamEvent::ThinkingChunk(
                                        std::mem::take(&mut pending_thinking),
                                    ));
                                    last_think_emit = Instant::now();
                                }
                                break;
                            }
                        } else if let Some(start) = rest.find("<think>") {
                            let before = &rest[..start];
                            if !before.is_empty() {
                                pending_chunk.push_str(before);
                                let out = std::mem::take(&mut pending_chunk);
                                let _ = tx_for_chunks.try_send(StreamEvent::Chunk(out));
                                last_emit = Instant::now();
                            }
                            let _ = tx_for_chunks.try_send(StreamEvent::ThinkingStarted);
                            in_thinking = true;
                            rest = &rest[start + "<think>".len()..];
                        } else {
                            pending_chunk.push_str(rest);
                            let should_emit = pending_chunk.len() >= 256
                                || pending_chunk.contains('\n')
                                || last_emit.elapsed() >= Duration::from_millis(24);
                            if should_emit {
                                let out = std::mem::take(&mut pending_chunk);
                                let _ = tx_for_chunks.try_send(StreamEvent::Chunk(out));
                                last_emit = Instant::now();
                            }
                            break;
                        }
                    }
                };

                let result = provider_for_task
                    .chat_completion_streaming(&token, &current_request, &mut on_chunk)
                    .map_err(|err| err.to_string());

                if !pending_chunk.is_empty() {
                    let _ = tx_for_chunks.try_send(StreamEvent::Chunk(pending_chunk));
                }
                // Stream ended mid-think: flush remaining content then close
                if in_thinking {
                    if !pending_thinking.is_empty() {
                        let _ =
                            tx_for_chunks.try_send(StreamEvent::ThinkingChunk(pending_thinking));
                    }
                    let _ = tx_for_chunks.try_send(StreamEvent::ThinkingDone);
                }

                match result {
                    Ok(response) => {
                        if !response.tool_calls.is_empty() {
                            // Always add assistant message (even if empty) so tool results can follow
                            let assistant_text =
                                response.assistant_message.clone().unwrap_or_default();
                            current_messages.push(ChatMessage {
                                role: ChatRole::Assistant,
                                content: assistant_text.clone(),
                                tool_call_id: None,
                                tool_calls: response.tool_calls.clone(),
                            });

                            // Show assistant text to user (if any)
                            if !assistant_text.is_empty() {
                                let _ = tx_for_chunks.try_send(StreamEvent::Chunk(assistant_text));
                            }

                            // Tell the UI to render a collapsed tool-call block.
                            let _ = tx_for_chunks
                                .try_send(StreamEvent::ToolCallGroup(response.tool_calls.clone()));

                            // Create tool context for execution
                            let workspace_root = match engine_state::get_project_path() {
                                Some(path) => PathBuf::from(path),
                                None => PathBuf::from("."),
                            };
                            let provider_for_subagent = provider_for_task.clone();
                            let token_for_subagent = token.clone();
                            let default_subagent_model = current_request.model.clone();
                            let tool_context = agent_chat_tools::make_tool_context(
                                workspace_root,
                                None,
                                agent_chat_tools::PulsarToolExtras {
                                    plugin_bridge: plugin_bridge_for_task.clone(),
                                    open_file_request: Some(Arc::new({
                                        let tx_for_open = tx_for_chunks.clone();
                                        move |path: PathBuf| {
                                            let waiter = Arc::new((Mutex::new(None), Condvar::new()));

                                            tx_for_open
                                                .try_send(StreamEvent::OpenFile(path, waiter.clone()))
                                                .map_err(|err| {
                                                    format!(
                                                        "Failed to dispatch open-file request to UI thread: {}",
                                                        err
                                                    )
                                                })?;

                                            let (lock, cvar) = &*waiter;
                                            let mut guard = lock.lock().map_err(|_| {
                                                "OpenFile waiter mutex poisoned".to_string()
                                            })?;
                                            while guard.is_none() {
                                                guard = cvar.wait(guard).map_err(|_| {
                                                    "OpenFile waiter mutex poisoned".to_string()
                                                })?;
                                            }

                                            guard
                                                .take()
                                                .unwrap_or_else(|| {
                                                    Err("OpenFile waiter signaled without a result".to_string())
                                                })
                                        }
                                    })),
                                    query_open_editors: Some(Arc::new(|| {
                                        Ok(crate::app::open_editors::snapshot_json())
                                    })),
                                    activate_open_editor_request: Some(Arc::new({
                                        let tx_for_activate = tx_for_chunks.clone();
                                        move |index: usize| {
                                            let before = crate::app::open_editors::snapshot_json();
                                            let open_count = before
                                                .get("open_count")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0) as usize;
                                            if index >= open_count {
                                                let active_before = before
                                                    .get("active_index")
                                                    .and_then(|v| v.as_u64())
                                                    .map(|v| v as usize);
                                                return Err(format!(
                                                    "ActivateOpenEditor index {} is out of range (open_count={}, active_before={:?}).",
                                                    index, open_count, active_before
                                                ));
                                            }

                                            let waiter = Arc::new((Mutex::new(None), Condvar::new()));

                                            tx_for_activate
                                                .try_send(StreamEvent::ActivateOpenEditor(
                                                    index,
                                                    waiter.clone(),
                                                ))
                                                .map_err(|err| {
                                                    format!(
                                                        "Failed to dispatch activate-open-editor request to UI thread: {}",
                                                        err
                                                    )
                                                })?;

                                            let (lock, cvar) = &*waiter;
                                            let mut guard = lock.lock().map_err(|_| {
                                                "ActivateOpenEditor waiter mutex poisoned".to_string()
                                            })?;
                                            while guard.is_none() {
                                                guard = cvar.wait(guard).map_err(|_| {
                                                    "ActivateOpenEditor waiter mutex poisoned".to_string()
                                                })?;
                                            }

                                            guard
                                                .take()
                                                .unwrap_or_else(|| {
                                                    Err("ActivateOpenEditor waiter signaled without a result".to_string())
                                                })
                                        }
                                    })),
                                    subagent_executor: Some(Arc::new({
                                        let registry = tool_registry_for_subagent.clone();
                                        move |req: agent_chat_tools::SubagentLlmRequest| {
                                        let model_used = req
                                            .model
                                            .clone()
                                            .filter(|m| !m.trim().is_empty())
                                            .unwrap_or_else(|| default_subagent_model.clone());

                                        let instructions = req.instructions.unwrap_or_default();

                                        // Build tool list for sub-agent, excluding UI-only and
                                        // recursive tools so sub-agents stay focused and safe.
                                        let sub_tools: Vec<agent_chat_core::ToolDefinition> = registry
                                            .definitions()
                                            .into_iter()
                                            .filter(|def| {
                                                !matches!(
                                                    def.name.as_str(),
                                                    "spawn_subagent"
                                                        | "query_running_subagents"
                                                        | "open_file_in_default_editor"
                                                        | "activate_open_editor"
                                                        | "query_open_editors"
                                                )
                                            })
                                            .filter_map(|def| {
                                                let params = def.parameters_schema;
                                                if params.get("type").and_then(|v| v.as_str())
                                                    != Some("object")
                                                {
                                                    return None;
                                                }
                                                Some(agent_chat_core::ToolDefinition {
                                                    name: def.name,
                                                    description: Some(def.description),
                                                    parameters_json_schema: params,
                                                })
                                            })
                                            .collect();

                                        let enable_tools = !sub_tools.is_empty();

                                        let sub_tool_context = agent_chat_tools::make_tool_context(
                                            req.workspace_root.clone(),
                                            None,
                                            agent_chat_tools::PulsarToolExtras {
                                                plugin_bridge: None,
                                                open_file_request: None,
                                                query_open_editors: None,
                                                activate_open_editor_request: None,
                                                subagent_executor: None,
                                            },
                                        );

                                        let mut current_messages = vec![
                                            ChatMessage {
                                                role: ChatRole::System,
                                                content: format!(
                                                    "You are a focused sub-agent. Complete the assigned task using available tools.\n\
                                                     Workspace: {workspace}\n\
                                                     Read files, search the codebase, gather concrete evidence.\n\
                                                     Be specific: include file paths, line numbers, and direct quotes.\n\
                                                     When you have sufficient findings, provide your final answer without calling more tools.",
                                                    workspace = req.workspace_root.display(),
                                                ),
                                                tool_call_id: None,
                                                tool_calls: vec![],
                                            },
                                            ChatMessage {
                                                role: ChatRole::User,
                                                content: format!(
                                                    "Sub-agent: {name}\nTask: {task}\nInstructions: {instructions}",
                                                    name = req.name,
                                                    task = req.task,
                                                    instructions = instructions,
                                                ),
                                                tool_call_id: None,
                                                tool_calls: vec![],
                                            },
                                        ];

                                        let mut all_chunks: Vec<String> = Vec::new();
                                        let mut last_raw_response = serde_json::Value::Null;
                                        const MAX_SUB_ITERATIONS: u32 = 8;

                                        for _sub_iter in 0..MAX_SUB_ITERATIONS {
                                            let sub_request = ChatRequest {
                                                model: model_used.clone(),
                                                messages: current_messages.clone(),
                                                enable_tool_calls: enable_tools,
                                                tools: sub_tools.clone(),
                                                temperature: Some(0.2),
                                                top_p: Some(1.0),
                                                max_tokens: Some(4096),
                                            };

                                            let mut iter_chunks: Vec<String> = Vec::new();
                                            let response = provider_for_subagent
                                                .chat_completion_streaming(
                                                    &token_for_subagent,
                                                    &sub_request,
                                                    &mut |chunk| iter_chunks.push(chunk),
                                                )
                                                .map_err(|e| format!("Sub-agent provider error: {e}"))?;

                                            all_chunks.extend(iter_chunks.clone());
                                            last_raw_response = response.raw_response.clone();

                                            let assistant_text = response
                                                .assistant_message
                                                .clone()
                                                .or_else(|| {
                                                    if iter_chunks.is_empty() {
                                                        None
                                                    } else {
                                                        Some(iter_chunks.join(""))
                                                    }
                                                })
                                                .unwrap_or_default();

                                            if response.tool_calls.is_empty() {
                                                current_messages.push(ChatMessage {
                                                    role: ChatRole::Assistant,
                                                    content: assistant_text,
                                                    tool_call_id: None,
                                                    tool_calls: vec![],
                                                });
                                                break;
                                            }

                                            // Assistant turn with tool calls
                                            current_messages.push(ChatMessage {
                                                role: ChatRole::Assistant,
                                                content: assistant_text,
                                                tool_call_id: None,
                                                tool_calls: response.tool_calls.clone(),
                                            });

                                            // Execute tools sequentially (sub-agent context)
                                            for call in &response.tool_calls {
                                                let result = registry
                                                    .execute(
                                                        &call.name,
                                                        call.arguments_json.clone(),
                                                        &sub_tool_context,
                                                    )
                                                    .map(|v| v.to_string())
                                                    .unwrap_or_else(|e| {
                                                        format!("Tool error: {e}")
                                                    });
                                                current_messages.push(ChatMessage {
                                                    role: ChatRole::Tool,
                                                    content: result,
                                                    tool_call_id: Some(call.id.clone()),
                                                    tool_calls: vec![],
                                                });
                                            }
                                        }

                                        // Extract the final assistant response from the transcript
                                        let assistant_message = current_messages
                                            .iter()
                                            .rev()
                                            .find(|m| {
                                                m.role == ChatRole::Assistant
                                                    && !m.content.is_empty()
                                            })
                                            .map(|m| m.content.clone())
                                            .unwrap_or_default();

                                        let child_transcript = current_messages
                                            .iter()
                                            .map(|m| {
                                                let role = match m.role {
                                                    ChatRole::System => "system",
                                                    ChatRole::User => "user",
                                                    ChatRole::Assistant => "assistant",
                                                    ChatRole::Tool => "tool",
                                                };
                                                serde_json::json!({
                                                    "role": role,
                                                    "content": m.content,
                                                })
                                            })
                                            .collect();

                                        Ok(agent_chat_tools::SubagentLlmResponse {
                                            provider_id: provider_for_subagent
                                                .metadata()
                                                .id
                                                .to_string(),
                                            model_used,
                                            assistant_message,
                                            streamed_chunks: all_chunks,
                                            raw_response: last_raw_response,
                                            child_transcript,
                                        })
                                    }})),
                                },
                            );

                            // Spawn one thread per tool call so they execute concurrently.
                            let mut all_results = Vec::new();
                            let handles: Vec<_> = response
                                .tool_calls
                                .iter()
                                .map(|tool_call| {
                                    let name = tool_call.name.clone();
                                    let args = tool_call.arguments_json.clone();
                                    let id = tool_call.id.clone();
                                    let registry = tool_registry_for_task.clone();
                                    let ctx = tool_context.clone();
                                    let tx = tx_for_chunks.clone();
                                    std::thread::spawn(move || {
                                        let result = registry.execute(&name, args, &ctx);
                                        let (tool_result, is_error) = match result {
                                            Ok(value) => (value.to_string(), false),
                                            Err(err) => (format!("Tool error: {}", err), true),
                                        };
                                        let result_preview =
                                            Self::format_tool_result_preview(&name, &tool_result);
                                        let _ = tx.try_send(StreamEvent::ToolCallResult {
                                            id: id.clone(),
                                            tool_name: name.clone(),
                                            result_preview,
                                            result_full: tool_result.clone(),
                                            is_error,
                                        });
                                        (id, name, tool_result)
                                    })
                                })
                                .collect();

                            // Collect parallel results in original order for message threading.
                            for handle in handles {
                                all_results.push(handle.join().unwrap_or_else(|_| {
                                    (
                                        "".to_string(),
                                        "unknown".to_string(),
                                        "Tool thread panicked".to_string(),
                                    )
                                }));
                            }

                            for (tool_call_id, _tool_name, tool_result) in all_results {
                                current_messages.push(ChatMessage {
                                    role: ChatRole::Tool,
                                    content: tool_result,
                                    tool_call_id: Some(tool_call_id),
                                    tool_calls: vec![],
                                });
                            }

                            // Compact current_messages if tool results pushed us over budget.
                            let total_chars: usize =
                                current_messages.iter().map(|m| m.content.len()).sum();
                            if total_chars > context_budget {
                                let (mut compacted, dropped_opt) =
                                    super::prompt_ranking::compact_messages(
                                        std::mem::take(&mut current_messages),
                                        context_budget,
                                        AgentChatPanel::COMPACTION_SUMMARY_CHAR_BUDGET,
                                    );
                                if let Some(dropped) = dropped_opt {
                                    let summary = AgentChatPanel::ai_summarize_dropped(
                                        &dropped,
                                        &provider_for_task,
                                        &compact_model_for_worker,
                                        &token,
                                    );
                                    // Insert AI summary before the kept dialog.
                                    let insert_at = compacted
                                        .iter()
                                        .position(|m| m.role != ChatRole::System)
                                        .unwrap_or(0);
                                    compacted.insert(
                                        insert_at,
                                        ChatMessage {
                                            role: ChatRole::System,
                                            content: format!(
                                                "Conversation summary (auto-compacted):\n{summary}"
                                            ),
                                            tool_call_id: None,
                                            tool_calls: vec![],
                                        },
                                    );
                                    let _ = tx_for_chunks
                                        .try_send(StreamEvent::ContextCompacted(summary));
                                }
                                current_messages = compacted;
                            }

                            continue;
                        } else {
                            if !completion_for_worker.swap(true, Ordering::SeqCst) {
                                let _ = tx_for_finish.try_send(StreamEvent::Finished(Ok(response)));
                            }
                            break;
                        }
                    }
                    Err(err) => {
                        tracing::error!("[agent_chat] provider error (iter {iteration}): {err}");
                        if !completion_for_worker.swap(true, Ordering::SeqCst) {
                            let _ = tx_for_finish.try_send(StreamEvent::Finished(Err(err)));
                        }
                        break;
                    }
                }
            }
        });

        let tx_timeout = tx.clone();
        let completion_for_timeout = completion_sent.clone();
        cx.background_spawn(async move {
            // 10-minute ceiling — long enough for extended agentic runs.
            Timer::after(Duration::from_secs(600)).await;
            if !completion_for_timeout.swap(true, Ordering::SeqCst) {
                tracing::error!("[agent_chat] watchdog: 10-minute limit reached");
                let _ = tx_timeout.try_send(StreamEvent::Finished(Err(
                    "Request timed out after 10 minutes.".to_string(),
                )));
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let should_break = matches!(event, StreamEvent::Finished(_));
                cx.update(|cx| {
                    this.update(cx, |panel, cx| {
                        match event {
                            StreamEvent::Chunk(chunk) => {
                                // Update provider history message
                                if let Some(message) = panel.messages.get_mut(message_ix) {
                                    message.content.push_str(&chunk);
                                    panel.message_row_heights.remove(&message_ix);
                                }
                                // Update the streaming display bubble.
                                // Do NOT remove the cached height — the canvas measurement from the
                                // previous frame is accurate and close to current. Removing it
                                // would cause the virtual list to use a capped estimate (e.g. 520px)
                                // instead of the actual measured value (e.g. 3000px), making scroll
                                // targets wrong until the canvas fires again.
                                if let Some(dix) = panel.streaming_display_item_ix {
                                    if let Some(DisplayItem::AssistantMessage { content, .. }) =
                                        panel.display_items.get_mut(dix)
                                    {
                                        content.push_str(&chunk);
                                        // height cache intentionally preserved
                                    }
                                }
                                cx.notify();
                            }

                            StreamEvent::ThinkingStarted => {
                                if let Some(dix) = panel.streaming_display_item_ix {
                                    let is_empty = panel.display_items.get(dix).map(|item|
                                        matches!(item, DisplayItem::AssistantMessage { content, .. } if content.is_empty())
                                    ).unwrap_or(false);
                                    if is_empty && dix + 1 == panel.display_items.len() {
                                        panel.display_items.pop();
                                        panel.display_item_heights.remove(&dix);
                                    } else if let Some(DisplayItem::AssistantMessage { is_streaming, .. }) =
                                        panel.display_items.get_mut(dix)
                                    {
                                        *is_streaming = false;
                                        panel.display_item_heights.remove(&dix);
                                    }
                                }
                                panel.display_items.push(DisplayItem::ThinkingBlock {
                                    content: String::new(),
                                    is_expanded: true, // open immediately so user sees tokens arriving
                                    is_done: false,
                                    started_at_ms: now_ms(),
                                    finished_at_ms: None,
                                });
                                let new_dix = panel.display_items.len();
                                panel.display_items.push(DisplayItem::AssistantMessage {
                                    content: String::new(),
                                    message_index: message_ix,
                                    is_streaming: true,
                                });
                                panel.streaming_display_item_ix = Some(new_dix);
                                panel.scroll_messages_to_bottom();
                                cx.notify();
                            }

                            StreamEvent::ThinkingChunk(chunk) => {
                                for (_ix, item) in panel.display_items.iter_mut().enumerate().rev() {
                                    if let DisplayItem::ThinkingBlock { content, .. } = item {
                                        content.push_str(&chunk);
                                        // height cache preserved — canvas corrects after layout
                                        break;
                                    }
                                }
                                cx.notify();
                            }

                            StreamEvent::ThinkingDone => {
                                let done_ms = now_ms();
                                for (ix, item) in panel.display_items.iter_mut().enumerate().rev() {
                                    if let DisplayItem::ThinkingBlock {
                                        is_done,
                                        is_expanded,
                                        finished_at_ms,
                                        ..
                                    } = item
                                    {
                                        *is_done = true;
                                        *is_expanded = false; // auto-collapse when thinking stops
                                        *finished_at_ms = Some(done_ms);
                                        panel.display_item_heights.remove(&ix);
                                        break;
                                    }
                                }
                                cx.notify();
                            }

                            StreamEvent::ContextCompacted(summary) => {
                                let t = now_ms();
                                let card = DisplayItem::CompactionSummary {
                                    summary,
                                    is_expanded: false,
                                    started_at_ms: t,
                                    finished_at_ms: Some(t),
                                };
                                if let Some(dix) = panel.streaming_display_item_ix {
                                    if dix + 1 == panel.display_items.len() {
                                        let bubble = panel.display_items.pop().unwrap();
                                        panel.display_items.push(card);
                                        panel.display_items.push(bubble);
                                        panel.streaming_display_item_ix = Some(dix + 1);
                                    }
                                } else {
                                    panel.display_items.push(card);
                                }
                                panel.scroll_messages_to_bottom();
                                cx.notify();
                            }

                            StreamEvent::ToolCallGroup(calls) => {
                                if let Some(dix) = panel.streaming_display_item_ix {
                                    let is_empty = panel.display_items.get(dix).map(|item|
                                        matches!(item, DisplayItem::AssistantMessage { content, .. } if content.is_empty())
                                    ).unwrap_or(false);
                                    if is_empty && dix + 1 == panel.display_items.len() {
                                        panel.display_items.pop();
                                        panel.display_item_heights.remove(&dix);
                                    } else if let Some(DisplayItem::AssistantMessage { is_streaming, .. }) =
                                        panel.display_items.get_mut(dix)
                                    {
                                        *is_streaming = false;
                                        panel.display_item_heights.remove(&dix);
                                    }
                                }
                                let group_start = now_ms();
                                panel.display_items.push(DisplayItem::ToolCallGroup {
                                    calls: calls.iter().map(|c| {
                                        let args_raw = serde_json::to_string(&c.arguments_json).unwrap_or_default();
                                        let args_preview = if args_raw.len() > 120 {
                                            format!("{}…", &args_raw[..120])
                                        } else { args_raw.clone() };
                                        ToolCallDisplay {
                                            id: c.id.clone(),
                                            name: c.name.clone(),
                                            args_preview,
                                            args_full: args_raw,
                                            result_preview: None,
                                            result_full: None,
                                            is_error: false,
                                            started_at_ms: group_start,
                                            finished_at_ms: None,
                                        }
                                    }).collect(),
                                    is_expanded: false,
                                    started_at_ms: group_start,
                                    finished_at_ms: None,
                                });
                                let new_dix = panel.display_items.len();
                                panel.display_items.push(DisplayItem::AssistantMessage {
                                    content: String::new(),
                                    message_index: message_ix,
                                    is_streaming: true,
                                });
                                panel.streaming_display_item_ix = Some(new_dix);
                                panel.scroll_messages_to_bottom();
                                cx.notify();
                            }

                            StreamEvent::ToolCallResult { id, tool_name, result_preview, result_full, is_error } => {
                                let done_ms = now_ms();
                                let result_full_for_parse = result_full.clone();
                                for item in panel.display_items.iter_mut().rev() {
                                    if let DisplayItem::ToolCallGroup { calls, finished_at_ms, .. } = item {
                                        if let Some(call) = calls.iter_mut().find(|c| c.id == id) {
                                            call.result_preview = Some(result_preview);
                                            call.result_full = Some(result_full.clone());
                                            call.is_error = is_error;
                                            call.finished_at_ms = Some(done_ms);
                                        }
                                        // Update group finished_at if all calls done
                                        if calls.iter().all(|c| c.finished_at_ms.is_some()) {
                                            *finished_at_ms = Some(done_ms);
                                        }
                                        break;
                                    }
                                }

                                if tool_name == "spawn_subagent" && !is_error {
                                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&result_full_for_parse) {
                                        let subagent_id = parsed
                                            .get("subagent_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default();
                                        if !subagent_id.is_empty() {
                                            let name = parsed
                                                .get("name")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("Subagent");
                                            let task = parsed
                                                .get("task")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("Subagent task");
                                            let created_at_ms = parsed
                                                .get("created_at_ms")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(done_ms);
                                            panel.update_subagent_invocation_started(
                                                subagent_id,
                                                name,
                                                task,
                                                created_at_ms,
                                                cx,
                                            );
                                        }
                                    }
                                }

                                cx.notify();
                            }

                            StreamEvent::Finished(Ok(response)) => {
                                panel.is_request_in_flight = false;
                                panel.streaming_message_ix = None;
                                panel.cancel_tx = None;

                                if panel.is_processing_subagent_event {
                                    panel.complete_active_subagent_processing(true);
                                    panel.is_processing_subagent_event = false;
                                    panel.processing_subagent_id = None;
                                }

                                if let Some(dix) = panel.streaming_display_item_ix.take() {
                                    let is_empty = panel
                                        .display_items
                                        .get(dix)
                                        .map(|item| matches!(item,
                                            DisplayItem::AssistantMessage { content, .. }
                                            if content.is_empty()
                                        ))
                                        .unwrap_or(false);

                                    if is_empty && dix + 1 == panel.display_items.len() {
                                        // Drop the trailing empty bubble — the turn ended with a
                                        // tool call group and no follow-up text from the AI.
                                        panel.display_items.pop();
                                        panel.display_item_heights.remove(&dix);
                                    } else if let Some(DisplayItem::AssistantMessage {
                                        content,
                                        is_streaming,
                                        ..
                                    }) = panel.display_items.get_mut(dix)
                                    {
                                        *is_streaming = false;
                                        if content.is_empty() {
                                            if let Some(text) = response.assistant_message.as_ref() {
                                                *content = text.clone();
                                            }
                                        }
                                        panel.display_item_heights.remove(&dix);
                                    }
                                }

                                if let Some(message) = panel.messages.get_mut(message_ix) {
                                    if message.content.is_empty() {
                                        if let Some(text) = response.assistant_message {
                                            message.content = text;
                                        }
                                    }
                                    panel.message_row_heights.remove(&message_ix);
                                }
                                panel.save_current_chat();
                                panel.refresh_chat_history_list(cx);
                                if panel.subagent_completion_mode == SubagentCompletionMode::Auto {
                                    panel.maybe_start_next_subagent_processing(cx);
                                }
                                panel.scroll_messages_to_bottom();
                                cx.notify();
                            }

                            StreamEvent::Finished(Err(err)) => {
                                panel.is_request_in_flight = false;
                                panel.streaming_message_ix = None;
                                panel.cancel_tx = None;
                                panel.complete_active_subagent_processing(false);
                                panel.is_processing_subagent_event = false;
                                panel.processing_subagent_id = None;
                                tracing::error!("[agent_chat] error: {err}");
                                let error_text = format!("Request failed: {err}");
                                if let Some(dix) = panel.streaming_display_item_ix.take() {
                                    let is_empty = panel
                                        .display_items
                                        .get(dix)
                                        .map(|item| matches!(item,
                                            DisplayItem::AssistantMessage { content, .. }
                                            if content.is_empty()
                                        ))
                                        .unwrap_or(false);
                                    if is_empty && dix + 1 == panel.display_items.len() {
                                        // Replace the empty bubble with the error message rather
                                        // than leaving a blank card above the error indicator.
                                        if let Some(item) = panel.display_items.get_mut(dix) {
                                            if let DisplayItem::AssistantMessage {
                                                content,
                                                is_streaming,
                                                ..
                                            } = item
                                            {
                                                *is_streaming = false;
                                                *content = error_text.clone();
                                            }
                                        }
                                    } else if let Some(DisplayItem::AssistantMessage {
                                        content,
                                        is_streaming,
                                        ..
                                    }) = panel.display_items.get_mut(dix)
                                    {
                                        *is_streaming = false;
                                        *content = error_text.clone();
                                        panel.display_item_heights.remove(&dix);
                                    }
                                }
                                if let Some(message) = panel.messages.get_mut(message_ix) {
                                    message.content = error_text;
                                    panel.message_row_heights.remove(&message_ix);
                                }
                                panel.save_current_chat();
                                panel.refresh_chat_history_list(cx);
                                if panel.subagent_completion_mode == SubagentCompletionMode::Auto {
                                    panel.maybe_start_next_subagent_processing(cx);
                                }
                                panel.scroll_messages_to_bottom();
                                cx.notify();
                            }

                            StreamEvent::OpenFile(path, waiter) => {
                                let result = panel.open_path_in_default_editor(path, cx);
                                let (lock, cvar) = &*waiter;
                                if let Ok(mut guard) = lock.lock() {
                                    *guard = Some(result);
                                    cvar.notify_all();
                                }
                            }
                            StreamEvent::ActivateOpenEditor(index, waiter) => {
                                let result = panel.activate_open_editor_by_global_index(index, cx);
                                let (lock, cvar) = &*waiter;
                                if let Ok(mut guard) = lock.lock() {
                                    *guard = Some(result);
                                    cvar.notify_all();
                                }
                            }
                        }
                    })
                    .ok();
                })
                .ok();

                if should_break {
                    break;
                }
            }
        })
        .detach();

        // Tick every 500 ms while in-flight: scroll to bottom (when auto_scroll is
        // on) and update live timers in card headers.
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(500)).await;
                let still_active = cx
                    .update(|cx| {
                        this.update(cx, |panel, cx| {
                            if panel.is_request_in_flight {
                                // Auto-scroll: push to bottom every tick unless the
                                // user has explicitly scrolled up.
                                if panel.auto_scroll {
                                    panel.messages_scroll_handle.scroll_to_bottom();
                                }
                                cx.notify();
                                true
                            } else {
                                false
                            }
                        })
                        .ok()
                    })
                    .ok()
                    .flatten()
                    .unwrap_or(false);
                if !still_active {
                    break;
                }
            }
        })
        .detach();

        self.save_current_chat();
        self.refresh_chat_history_list(cx);
        self.scroll_messages_to_bottom();
        cx.notify();
    }

    /// Remove the last assistant response and re-run the provider using the same
    /// message history (which ends with the last user message after truncation).
    pub(super) fn regenerate_response(&mut self, cx: &mut Context<Self>) {
        if self.is_request_in_flight {
            return;
        }
        // Find the last AssistantMessage in display_items and roll back to just before it.
        let last_assistant_dix =
            self.display_items
                .iter()
                .enumerate()
                .rev()
                .find_map(|(dix, item)| match item {
                    DisplayItem::AssistantMessage { message_index, .. } => {
                        Some((dix, *message_index))
                    }
                    _ => None,
                });

        let Some((dix, msg_ix)) = last_assistant_dix else {
            return;
        };

        // Truncate display_items and messages up to but NOT including this assistant.
        self.display_items.truncate(dix);
        self.messages.truncate(msg_ix);
        if self.messages.is_empty() {
            return;
        }
        self.display_item_heights.clear();
        self.message_row_heights.clear();

        let provider_id = self.active_provider().map(|p| p.id).unwrap_or("unknown");
        let Some(provider) = self.provider_registry.get(provider_id).cloned() else {
            return;
        };
        let token = self.auth_token_for_provider(provider_id);
        self.launch_provider_request(provider, token, cx);
    }

    /// Replace a user message at `display_ix` in-place: rolls back to before it
    /// and puts its content into the prompt input ready for editing.
    pub(super) fn edit_user_message(
        &mut self,
        display_ix: usize,
        message_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_request_in_flight {
            return;
        }
        let content = match self.display_items.get(display_ix) {
            Some(DisplayItem::UserMessage { content, .. }) => content.clone(),
            _ => return,
        };
        // Roll back to just before this user message.
        if display_ix == 0 || message_index == 0 {
            return;
        }
        self.display_items.truncate(display_ix);
        self.messages.truncate(message_index);
        self.display_item_heights.clear();
        self.message_row_heights.clear();
        self.streaming_display_item_ix = None;
        self.streaming_message_ix = None;
        self.save_current_chat();
        self.refresh_chat_history_list(cx);
        self.scroll_messages_to_bottom();
        self.prompt_input.update(cx, |input, cx| {
            input.set_value(&content, window, cx);
        });
        cx.notify();
    }
}
