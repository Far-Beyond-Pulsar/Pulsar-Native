use super::*;
use agent_runtime::{CancelToken, EventSink, PinKind, RunConfig, RunEvent};
use engine_state;
use smol::Timer;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use ui::{input::Enter, scroll::ScrollHandleOffsetable};

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl AgentChatPanel {
    /// Task manifest and active sub-agents, or `None` when both are empty.
    fn live_agent_state(&self) -> Option<String> {
        let tasks = agent_chat_tools::get_task_manifest_snapshot();
        let active_subs = self.delegation.store().active();
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
                .map(|run| {
                    format!(
                        "  - {} ({}, id={}): {}",
                        run.harness,
                        run.status.as_str(),
                        run.id,
                        run.task
                    )
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

        let message_ix = self.ledger.push(LedgerEntry::assistant(String::new()));
        self.scroll_messages_to_bottom();
        cx.notify();

        cx.spawn(async move |this, cx| {
            for chunk in chunks {
                cx.update(|cx| {
                    this.update(cx, |panel, cx| {
                        if let Some(text) = panel.ledger.assistant_text_mut(message_ix) {
                            text.push_str(&chunk);
                        }
                        panel.save_current_chat();
                        panel.scroll_messages_to_bottom();
                        cx.notify();
                    });
                });

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
        let user_message_index = self.ledger.push(LedgerEntry::user(prompt_for_model));
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

        let Some(provider) = self.provider_registry.get(provider_id).cloned() else {
            let provider = self
                .active_provider()
                .map(|p| p.label)
                .unwrap_or("Unknown Provider");
            let model = self
                .active_model()
                .map(|m| m.label)
                .unwrap_or("Unknown Model");
            self.ledger.push(LedgerEntry::assistant(format!(
                "Queued with {provider} / {model}."
            )));
            self.save_current_chat();
            self.refresh_chat_history_list(cx);
            self.scroll_messages_to_bottom();
            cx.notify();
            return false;
        };


        self.launch_provider_request(provider, None, cx);
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
        _token: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let message_ix = self.ledger.push(LedgerEntry::assistant(String::new()));
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

        let provider_id = provider.id();
        let model = self
            .active_model()
            .map(|m| m.id.to_string())
            .unwrap_or_else(|| "default".to_string());

        // Context window, tool-calling mode, tier and sampling all come from
        // the model's profile (table, descriptor, user override).
        let profile = self.active_profile();

        // Resolve the compact model: use the specified one or fall back to the current model.
        let compact_model: String = self
            .active_model()
            .and_then(|m| m.compact_model)
            .unwrap_or_else(|| model.as_str())
            .to_string();

        // The run works on its own copy; the panel's ledger only grows with
        // the streamed assistant text.
        let mut run_ledger = self.ledger.clone();

        // Inject live orchestration context — task manifest and active sub-agents.
        // This survives compaction because it is rebuilt fresh on every request,
        // so the agent is always oriented even after context shrinks.
        if let Some(live_state) = self.live_agent_state() {
            run_ledger.push(LedgerEntry::pinned(PinKind::LiveState, live_state));
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
        tracing::error!(
            "[agent_chat] start provider={} model={} entries={}",
            provider_id,
            model,
            run_ledger.len()
        );

        let workspace_root = match engine_state::get_project_path() {
            Some(path) => PathBuf::from(path),
            None => PathBuf::from("."),
        };
        let mut config = RunConfig::new(
            provider.clone(),
            model.clone(),
            self.tool_registry.clone(),
            workspace_root,
        )
        .with_profile(profile);
        config.compact_model = compact_model;
        config.tools = tools;
        config.tool_extras = agent_chat_tools::PulsarToolExtras {
            plugin_bridge: self.plugin_bridge.clone(),
            open_file_request: None,
            query_open_editors: Some(Arc::new(|| Ok(crate::app::open_editors::snapshot_json()))),
            activate_open_editor_request: None,
        };
        config.safety.policy = self.confirm_policy;

        // "Use AI to keep long chats on track": with it off no steward exists
        // and no extra request is made. The steward model is the setting's
        // choice, else the model's compact model.
        let steward_model = self
            .steward_settings
            .model
            .clone()
            .unwrap_or_else(|| config.compact_model.clone());
        let steward_json_mode =
            Self::profile_for_model(self.active_provider(), &steward_model).json_mode;
        if let Some(steward) =
            self.ensure_steward(provider.clone(), steward_model, steward_json_mode, cx)
        {
            config.steward = Some(steward);
            config.structured_summary = true;
        }

        // Sub-agents are child runs of this one; the template they derive
        // from is the config as built above.
        self.delegation.set_harnesses(Self::load_harnesses());
        let limits = agent_runtime::DelegationLimits::for_profile(&config.profile);
        self.delegation.set_budget(agent_runtime::RunBudget {
            max_depth: limits.max_depth,
            ..agent_runtime::RunBudget::default()
        });
        self.delegation.set_limits(limits);
        // A sub-agent on another model gets that model's profile (window,
        // tool-calling mode, tier), resolved the way the chat's own is.
        let provider_definition = self.active_provider().cloned();
        self.delegation
            .set_profile_resolver(Some(Arc::new(move |model: &str| {
                Self::profile_for_model(provider_definition.as_ref(), model)
            })));
        config.delegation = Some(self.delegation.root_handle(config.clone()));

        let (tx, rx) = smol::channel::unbounded::<RunEvent>();
        let completion_sent = Arc::new(AtomicBool::new(false));
        // Finished is delivered once, whether it comes from the loop or the watchdog.
        let sink: EventSink = {
            let tx = tx.clone();
            let completion_sent = completion_sent.clone();
            Arc::new(move |event| {
                if matches!(event, RunEvent::Finished(_))
                    && completion_sent.swap(true, Ordering::SeqCst)
                {
                    return;
                }
                let _ = tx.try_send(event);
            })
        };

        // Stop button: the UI cancels this token.
        let cancel = CancelToken::new();
        self.cancel = Some(cancel.clone());
        agent_runtime::spawn_run(config, run_ledger, cancel.clone(), sink.clone());

        let completion_for_timeout = completion_sent.clone();
        cx.background_spawn(async move {
            // 10-minute ceiling — long enough for extended agentic runs.
            Timer::after(Duration::from_secs(600)).await;
            if !completion_for_timeout.load(Ordering::SeqCst) {
                tracing::error!("[agent_chat] watchdog: 10-minute limit reached");
                cancel.cancel();
                sink(RunEvent::Finished(Err(
                    "Request timed out after 10 minutes.".to_string(),
                )));
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let should_break = matches!(event, RunEvent::Finished(_));
                cx.update(|cx| {
                    this.update(cx, |panel, cx| {
                        match event {
                            RunEvent::Chunk(chunk) => {
                                // Update provider history message
                                if let Some(text) = panel.ledger.assistant_text_mut(message_ix) {
                                    text.push_str(&chunk);
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

                            RunEvent::ThinkingStarted => {
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

                            RunEvent::ThinkingChunk(chunk) => {
                                for (_ix, item) in panel.display_items.iter_mut().enumerate().rev() {
                                    if let DisplayItem::ThinkingBlock { content, .. } = item {
                                        content.push_str(&chunk);
                                        // height cache preserved — canvas corrects after layout
                                        break;
                                    }
                                }
                                cx.notify();
                            }

                            RunEvent::ThinkingDone => {
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

                            RunEvent::ContextCompacted(summary) => {
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

                            RunEvent::ContextNotice(line) => {
                                panel.note_context_event(line, cx);
                            }
                            RunEvent::FilesTouched(paths) => {
                                panel.note_files_touched(paths, cx);
                            }

                            RunEvent::ToolCallGroup(calls) => {
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
                                    checkpoint_run_id: None,
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

                            RunEvent::ToolCallResult { id, tool_name: _, result_preview, result_full, is_error } => {
                                let done_ms = now_ms();
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

                                cx.notify();
                            }

                            RunEvent::Finished(Ok(response)) => {
                                panel.drop_pending_confirms();
                                panel.is_request_in_flight = false;
                                panel.streaming_message_ix = None;
                                panel.cancel = None;

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

                                if let Some(entry_text) = panel.ledger.assistant_text_mut(message_ix) {
                                    if entry_text.is_empty() {
                                        if let Some(text) = response.assistant_message {
                                            *entry_text = text;
                                        }
                                    }
                                }
                                // Background reports that finished during the run
                                // join the ledger; the next request carries them.
                                panel.deliver_idle_subagent_reports();
                                panel.save_current_chat();
                                panel.refresh_chat_history_list(cx);
                                panel.scroll_messages_to_bottom();
                                cx.notify();
                            }

                            RunEvent::Finished(Err(err)) => {
                                panel.drop_pending_confirms();
                                panel.is_request_in_flight = false;
                                panel.streaming_message_ix = None;
                                panel.cancel = None;
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
                                if let Some(text) = panel.ledger.assistant_text_mut(message_ix) {
                                    *text = error_text;
                                }
                                panel.deliver_idle_subagent_reports();
                                panel.save_current_chat();
                                panel.refresh_chat_history_list(cx);
                                panel.scroll_messages_to_bottom();
                                cx.notify();
                            }

                            RunEvent::ConfirmRequest { id, tool, args, tainted, reply } => {
                                panel.queue_confirm(id, tool, args, tainted, reply, cx);
                            }
                            RunEvent::CheckpointCreated { run_id } => {
                                panel.attach_checkpoint(run_id, cx);
                            }
                            // Child runs report through the delegation host's
                            // sink (see `subagent.rs`), never through a request's.
                            RunEvent::ChildSpawned { .. }
                            | RunEvent::ChildProgress { .. }
                            | RunEvent::ChildFinished { .. } => {}

                            RunEvent::OpenFile(path, reply) => {
                                reply.reply(panel.open_path_in_default_editor(path, cx));
                            }
                            RunEvent::ActivateOpenEditor(index, reply) => {
                                reply.reply(panel.activate_open_editor_by_global_index(index, cx));
                            }
                        }
                    })
                    ;
                })
                ;

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
                    })
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

        // Truncate display_items and ledger up to but NOT including this assistant.
        self.display_items.truncate(dix);
        self.ledger.truncate_keep_pins(msg_ix);
        if self.ledger.is_empty() {
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
        self.ledger.truncate_keep_pins(message_index);
        self.display_item_heights.clear();
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
