//! Sending prompts to the provider and streaming the reply into the chat.
//!
//! `send_prompt` / `launch_provider_request` are the entry points. A request is
//! assembled by `request_prep.rs`, run on a worker thread by `stream_worker.rs`
//! (which talks to tools via `stream_tools.rs` and splits `<think>` text with
//! `think_parser.rs`), and its events come back as `StreamEvent`s
//! (`stream_event.rs`) that this module's event pump applies on the UI thread.
//! Also holds regenerate/edit helpers that roll the history back and re-send.

use super::request_prep::{PreparedRequest, RequestInputs};
use super::stream_event::StreamEvent;
use super::stream_worker::{run_agent_loop, send_finished_once, WorkerContext};
use super::*;
use agent_chat_core::{ChatMessage, ChatRole};
use editor_task_queue::{TaskDescription, TaskDuration};
use smol::Timer;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use ui::input::Enter;

/// Wall-clock milliseconds since the Unix epoch.
pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl AgentChatPanel {
    pub(super) fn ai_summarize_dropped(
        dropped: &[ChatMessage],
        provider: &Arc<dyn agent_chat_core::ChatProvider>,
        compact_model: &str,
        _token: &str,
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
                    ChatRole::AgentEvent => "Engine",
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
            .chat(summary_request)
            .ok()
            .and_then(|r| r.assistant_message)
            .unwrap_or_else(|| {
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

    fn submit_user_prompt_text(&mut self, raw_prompt: String, cx: &mut Context<Self>) -> bool {
        let user_message_index = self.messages.len();
        self.messages.push(ChatMessage {
            role: ChatRole::User,
            content: raw_prompt.clone(),
            tool_call_id: None,
            tool_calls: vec![],
        });
        self.display_items.push(DisplayItem::UserMessage {
            content: raw_prompt,
            message_index: user_message_index,
        });
        self.scroll_messages_to_bottom();

        let provider_id = self
            .active_provider()
            .map(|p| p.id)
            .unwrap_or("unknown_provider");

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
            self.save_current_chat(cx);
            self.scroll_messages_to_bottom();
            cx.notify();
            return false;
        };

        self.launch_provider_request(provider, Some(user_message_index), cx);
        true
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

        self.prompt_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        let _ = self.submit_user_prompt_text(raw_prompt, cx);
    }

    /// Push an empty streaming assistant bubble and fire off the provider request.
    /// Called by `send_prompt` (after the user message is pushed) and by
    /// `regenerate_response` (after the old assistant message is removed).
    pub(super) fn launch_provider_request(
        &mut self,
        provider: Arc<dyn agent_chat_core::ChatProvider>,
        expand_message: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let message_ix = self.begin_streaming_bubble();

        let model = self
            .active_model()
            .map(|model| model.id)
            .unwrap_or("default");
        let inputs = RequestInputs {
            model: model.to_string(),
            history_budget: self
                .active_context_chars()
                .saturating_sub(Self::COMPACTION_SUMMARY_CHAR_BUDGET),
            compact_model: self
                .active_model()
                .and_then(|model| model.compact_model)
                .unwrap_or(model)
                .to_string(),
            messages: self.messages[..message_ix].to_vec(),
        };
        let workspace_root = engine_state::get_project_path()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let tool_registry = self.tool_registry.clone();
        let plugin_bridge = self.plugin_bridge.clone();

        let (worker_tx, rx) = smol::channel::unbounded::<StreamEvent>();
        let completion_sent = Arc::new(AtomicBool::new(false));

        // Per-request cancel channel — UI sends () to abort.
        let (cancel_tx, cancel_rx) = smol::channel::bounded::<()>(1);
        self.cancel_tx = Some(cancel_tx);

        let worker_completion = completion_sent.clone();
        editor_task_queue::global().submit(
            TaskDescription::new("Process AI chat request", "AI chat", TaskDuration::Long),
            move |task| {
                task.set_phase("Preparing chat request");
                if worker_tx.is_closed() {
                    return Err("Chat request ended before preparation started.".into());
                }
                if task.is_cancelled() || cancel_rx.try_recv().is_ok() {
                    task.mark_cancelled();
                    let reason = "Request cancelled.".to_string();
                    send_finished_once(&worker_completion, &worker_tx, Err(reason.clone()));
                    return Err(reason);
                }
                let mut inputs = inputs;
                if let Some(index) = expand_message {
                    let message = &mut inputs.messages[index];
                    if message
                        .content
                        .split_whitespace()
                        .any(|word| word.starts_with('@'))
                    {
                        task.set_phase("Reading attached files");
                        message.content =
                            Self::expand_file_references(&message.content, &workspace_root);
                        let _ = worker_tx.try_send(StreamEvent::UserContextExpanded {
                            index,
                            content: message.content.clone(),
                        });
                    }
                }
                inputs
                    .messages
                    .retain(|message| !message.content.trim().is_empty());
                if let Some(reason) = super::stream_worker::loop_stop_reason(&cancel_rx, 0) {
                    task.mark_cancelled();
                    send_finished_once(&worker_completion, &worker_tx, Err(reason.clone()));
                    return Err(reason);
                }
                let PreparedRequest {
                    request,
                    history_budget,
                    compact_model,
                    token,
                    compaction_summary,
                } = inputs.prepare(&provider, &tool_registry, &task);
                if let Some(summary) = compaction_summary {
                    let _ = worker_tx.try_send(StreamEvent::ContextCompacted(summary));
                }
                run_agent_loop(
                    WorkerContext {
                        provider,
                        request,
                        token,
                        compact_model,
                        context_budget: history_budget,
                        tool_registry,
                        plugin_bridge,
                        tx: worker_tx,
                        cancel_rx,
                        completion_sent: worker_completion,
                    },
                    &task,
                )
            },
        );

        Self::spawn_stream_event_pump(rx, completion_sent, message_ix, cx);
        Self::spawn_in_flight_tick(cx);

        self.save_current_chat(cx);
        self.scroll_messages_to_bottom();
        cx.notify();
    }

    /// Push the empty assistant message and its streaming display bubble, mark
    /// the request in flight and return the new message's index.
    fn begin_streaming_bubble(&mut self) -> usize {
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
        message_ix
    }

    /// UI-thread task that applies worker events until `Finished` arrives.
    fn spawn_stream_event_pump(
        rx: smol::channel::Receiver<StreamEvent>,
        completion_sent: Arc<AtomicBool>,
        message_ix: usize,
        cx: &mut Context<Self>,
    ) {
        let deadline = Instant::now() + Duration::from_secs(600);
        cx.spawn(async move |this, cx| {
            editor_task_queue::tracked(
                TaskDescription::new("Apply AI chat updates", "AI chat", TaskDuration::Short),
                async move {
                    if let Some(task) = editor_task_queue::current() {
                        task.set_phase("Delivering chat updates to the editor");
                    }
                    loop {
                        let first = smol::future::or(
                            async {
                                rx.recv().await.unwrap_or_else(|_| {
                                    StreamEvent::Finished(Err(
                                        "Chat task ended before returning a response.".into(),
                                    ))
                                })
                            },
                            async {
                                Timer::at(deadline).await;
                                completion_sent.store(true, Ordering::SeqCst);
                                StreamEvent::Finished(Err(
                                    "Request timed out after 10 minutes.".into()
                                ))
                            },
                        )
                        .await;
                        if matches!(
                            &first,
                            StreamEvent::Chunk(_) | StreamEvent::ThinkingChunk(_)
                        ) {
                            Timer::after(Duration::from_millis(16)).await;
                        }
                        let mut events = vec![first];
                        for _ in 1..64 {
                            if matches!(events.last(), Some(StreamEvent::Finished(_))) {
                                break;
                            }
                            let Ok(event) = rx.try_recv() else { break };
                            match (events.last_mut(), event) {
                                (Some(StreamEvent::Chunk(text)), StreamEvent::Chunk(chunk))
                                | (
                                    Some(StreamEvent::ThinkingChunk(text)),
                                    StreamEvent::ThinkingChunk(chunk),
                                ) => {
                                    text.push_str(&chunk);
                                }
                                (_, event) => events.push(event),
                            }
                        }
                        let finished = matches!(events.last(), Some(StreamEvent::Finished(_)));
                        let active = cx.update(|cx| {
                            this.update(cx, |panel, cx| {
                                for event in events {
                                    panel.apply_stream_event(event, message_ix, cx);
                                }
                                true
                            })
                            .unwrap_or(false)
                        });
                        if finished || !active {
                            rx.close();
                            break;
                        }
                        Timer::after(Duration::from_millis(1)).await;
                    }
                },
            )
            .await;
        })
        .detach();
    }

    /// Tick every 500 ms while in-flight: scroll to bottom (when auto_scroll is
    /// on) and update live timers in card headers.
    fn spawn_in_flight_tick(cx: &mut Context<Self>) {
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
                    })
                    .unwrap_or(false);
                if !still_active {
                    break;
                }
            }
        })
        .detach();
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

        let provider_id = self.active_provider().map(|p| p.id).unwrap_or("unknown");
        let Some(provider) = self.provider_registry.get(provider_id).cloned() else {
            return;
        };
        self.launch_provider_request(provider, None, cx);
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
        self.streaming_display_item_ix = None;
        self.streaming_message_ix = None;
        self.save_current_chat(cx);
        self.scroll_messages_to_bottom();
        self.prompt_input.update(cx, |input, cx| {
            input.set_value(&content, window, cx);
        });
        cx.notify();
    }
}
