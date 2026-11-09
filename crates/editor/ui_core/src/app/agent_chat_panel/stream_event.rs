//! Messages sent from the provider worker thread to the UI thread, and the
//! UI-side handler that applies them.
//!
//! Owns [`StreamEvent`] and `AgentChatPanel::apply_stream_event`, which
//! mutates `messages` / `display_items` for each event (prose chunks, thinking
//! blocks, tool call groups and results, compaction cards, open-file requests
//! and the final `Finished`). The event pump in `streaming.rs` calls
//! `apply_stream_event` once per received event, on the UI thread.

use super::streaming::now_ms;
use super::*;
use agent_chat_core::ToolCall;
use std::sync::{Condvar, Mutex};

/// Shared slot a worker thread blocks on while the UI thread services a request.
pub(super) type UiWaiter = Arc<(Mutex<Option<Result<(), String>>>, Condvar)>;

/// One event from the worker thread; events are applied in channel order.
pub(super) enum StreamEvent {
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
    OpenFile(PathBuf, UiWaiter),
    ActivateOpenEditor(usize, UiWaiter),
    Finished(Result<agent_chat_core::ChatResponse, String>),
}

impl AgentChatPanel {
    /// Apply one worker event to the panel state. `message_ix` is the index of
    /// the assistant message this request is streaming into.
    pub(super) fn apply_stream_event(
        &mut self,
        event: StreamEvent,
        message_ix: usize,
        cx: &mut Context<Self>,
    ) {
        match event {
            StreamEvent::Chunk(chunk) => {
                // Update provider history message
                if let Some(message) = self.messages.get_mut(message_ix) {
                    message.content.push_str(&chunk);
                }
                // Update the streaming display bubble.
                // Do NOT remove the cached height — the canvas measurement from the
                // previous frame is accurate and close to current. Removing it
                // would cause the virtual list to use a capped estimate (e.g. 520px)
                // instead of the actual measured value (e.g. 3000px), making scroll
                // targets wrong until the canvas fires again.
                if let Some(dix) = self.streaming_display_item_ix {
                    if let Some(DisplayItem::AssistantMessage { content, .. }) =
                        self.display_items.get_mut(dix)
                    {
                        content.push_str(&chunk);
                        // height cache intentionally preserved
                    }
                }
                cx.notify();
            }

            StreamEvent::ThinkingStarted => {
                if let Some(dix) = self.streaming_display_item_ix {
                    let is_empty = self.display_items.get(dix).map(|item|
                        matches!(item, DisplayItem::AssistantMessage { content, .. } if content.is_empty())
                    ).unwrap_or(false);
                    if is_empty && dix + 1 == self.display_items.len() {
                        self.display_items.pop();
                        self.display_item_heights.remove(&dix);
                    } else if let Some(DisplayItem::AssistantMessage { is_streaming, .. }) =
                        self.display_items.get_mut(dix)
                    {
                        *is_streaming = false;
                        self.display_item_heights.remove(&dix);
                    }
                }
                self.display_items.push(DisplayItem::ThinkingBlock {
                    content: String::new(),
                    is_expanded: true, // open immediately so user sees tokens arriving
                    is_done: false,
                    started_at_ms: now_ms(),
                    finished_at_ms: None,
                });
                let new_dix = self.display_items.len();
                self.display_items.push(DisplayItem::AssistantMessage {
                    content: String::new(),
                    message_index: message_ix,
                    is_streaming: true,
                });
                self.streaming_display_item_ix = Some(new_dix);
                self.scroll_messages_to_bottom();
                cx.notify();
            }

            StreamEvent::ThinkingChunk(chunk) => {
                for (_ix, item) in self.display_items.iter_mut().enumerate().rev() {
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
                for (ix, item) in self.display_items.iter_mut().enumerate().rev() {
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
                        self.display_item_heights.remove(&ix);
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
                if let Some(dix) = self.streaming_display_item_ix {
                    if dix + 1 == self.display_items.len() {
                        let bubble = self.display_items.pop().unwrap();
                        self.display_items.push(card);
                        self.display_items.push(bubble);
                        self.streaming_display_item_ix = Some(dix + 1);
                    }
                } else {
                    self.display_items.push(card);
                }
                self.scroll_messages_to_bottom();
                cx.notify();
            }

            StreamEvent::ToolCallGroup(calls) => {
                if let Some(dix) = self.streaming_display_item_ix {
                    let is_empty = self.display_items.get(dix).map(|item|
                        matches!(item, DisplayItem::AssistantMessage { content, .. } if content.is_empty())
                    ).unwrap_or(false);
                    if is_empty && dix + 1 == self.display_items.len() {
                        self.display_items.pop();
                        self.display_item_heights.remove(&dix);
                    } else if let Some(DisplayItem::AssistantMessage { is_streaming, .. }) =
                        self.display_items.get_mut(dix)
                    {
                        *is_streaming = false;
                        self.display_item_heights.remove(&dix);
                    }
                }
                let group_start = now_ms();
                self.display_items.push(DisplayItem::ToolCallGroup {
                    calls: calls
                        .iter()
                        .map(|c| {
                            let args_raw =
                                serde_json::to_string(&c.arguments_json).unwrap_or_default();
                            let args_preview = if args_raw.len() > 120 {
                                format!("{}…", &args_raw[..120])
                            } else {
                                args_raw.clone()
                            };
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
                        })
                        .collect(),
                    is_expanded: false,
                    started_at_ms: group_start,
                    finished_at_ms: None,
                });
                let new_dix = self.display_items.len();
                self.display_items.push(DisplayItem::AssistantMessage {
                    content: String::new(),
                    message_index: message_ix,
                    is_streaming: true,
                });
                self.streaming_display_item_ix = Some(new_dix);
                self.scroll_messages_to_bottom();
                cx.notify();
            }

            StreamEvent::ToolCallResult {
                id,
                tool_name,
                result_preview,
                result_full,
                is_error,
            } => {
                let done_ms = now_ms();
                let result_full_for_parse = result_full.clone();
                for item in self.display_items.iter_mut().rev() {
                    if let DisplayItem::ToolCallGroup {
                        calls,
                        finished_at_ms,
                        ..
                    } = item
                    {
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
                    if let Ok(parsed) =
                        serde_json::from_str::<serde_json::Value>(&result_full_for_parse)
                    {
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
                            self.update_subagent_invocation_started(
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
                self.is_request_in_flight = false;
                self.streaming_message_ix = None;
                self.cancel_tx = None;

                if self.is_processing_subagent_event {
                    self.complete_active_subagent_processing(true);
                    self.is_processing_subagent_event = false;
                    self.processing_subagent_id = None;
                }

                if let Some(dix) = self.streaming_display_item_ix.take() {
                    let is_empty = self
                        .display_items
                        .get(dix)
                        .map(|item| {
                            matches!(item,
                                DisplayItem::AssistantMessage { content, .. }
                                if content.is_empty()
                            )
                        })
                        .unwrap_or(false);

                    if is_empty && dix + 1 == self.display_items.len() {
                        // Drop the trailing empty bubble — the turn ended with a
                        // tool call group and no follow-up text from the AI.
                        self.display_items.pop();
                        self.display_item_heights.remove(&dix);
                    } else if let Some(DisplayItem::AssistantMessage {
                        content,
                        is_streaming,
                        ..
                    }) = self.display_items.get_mut(dix)
                    {
                        *is_streaming = false;
                        if content.is_empty() {
                            if let Some(text) = response.assistant_message.as_ref() {
                                *content = text.clone();
                            }
                        }
                        self.display_item_heights.remove(&dix);
                    }
                }

                if let Some(message) = self.messages.get_mut(message_ix) {
                    if message.content.is_empty() {
                        if let Some(text) = response.assistant_message {
                            message.content = text;
                        }
                    }
                }
                self.save_current_chat();
                self.refresh_chat_history_list(cx);
                if self.subagent_completion_mode == SubagentCompletionMode::Auto {
                    self.maybe_start_next_subagent_processing(cx);
                }
                self.scroll_messages_to_bottom();
                cx.notify();
            }

            StreamEvent::Finished(Err(err)) => {
                self.is_request_in_flight = false;
                self.streaming_message_ix = None;
                self.cancel_tx = None;
                self.complete_active_subagent_processing(false);
                self.is_processing_subagent_event = false;
                self.processing_subagent_id = None;
                tracing::error!("[agent_chat] error: {err}");
                let error_text = format!("Request failed: {err}");
                if let Some(dix) = self.streaming_display_item_ix.take() {
                    let is_empty = self
                        .display_items
                        .get(dix)
                        .map(|item| {
                            matches!(item,
                                DisplayItem::AssistantMessage { content, .. }
                                if content.is_empty()
                            )
                        })
                        .unwrap_or(false);
                    if is_empty && dix + 1 == self.display_items.len() {
                        // Replace the empty bubble with the error message rather
                        // than leaving a blank card above the error indicator.
                        if let Some(item) = self.display_items.get_mut(dix) {
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
                    }) = self.display_items.get_mut(dix)
                    {
                        *is_streaming = false;
                        *content = error_text.clone();
                        self.display_item_heights.remove(&dix);
                    }
                }
                if let Some(message) = self.messages.get_mut(message_ix) {
                    message.content = error_text;
                }
                self.save_current_chat();
                self.refresh_chat_history_list(cx);
                if self.subagent_completion_mode == SubagentCompletionMode::Auto {
                    self.maybe_start_next_subagent_processing(cx);
                }
                self.scroll_messages_to_bottom();
                cx.notify();
            }

            StreamEvent::OpenFile(path, waiter) => {
                let result = self.open_path_in_default_editor(path, cx);
                let (lock, cvar) = &*waiter;
                if let Ok(mut guard) = lock.lock() {
                    *guard = Some(result);
                    cvar.notify_all();
                }
            }
            StreamEvent::ActivateOpenEditor(index, waiter) => {
                let result = self.activate_open_editor_by_global_index(index, cx);
                let (lock, cvar) = &*waiter;
                if let Ok(mut guard) = lock.lock() {
                    *guard = Some(result);
                    cvar.notify_all();
                }
            }
        }
    }
}
