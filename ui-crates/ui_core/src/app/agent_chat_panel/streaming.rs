use super::*;
use agent_chat_core::{
    AvailabilityState, ChatMessage, ChatRequest, ChatRole,
    ProcessEnvironment,
};
use engine_state;
use smol::Timer;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use ui::input::Enter;

impl AgentChatPanel {
    const CONTEXT_CHAR_BUDGET: usize = 24_000;
    const COMPACTION_SUMMARY_CHAR_BUDGET: usize = 2_400;

    fn build_provider_history_messages(&self) -> Vec<ChatMessage> {
        self.messages
            .iter()
            .filter(|m| !m.content.trim().is_empty())
            .map(|m| ChatMessage {
                role: m.role,
                content: m.content.clone(),
                tool_call_id: m.tool_call_id.clone(),
            })
            .collect()
    }

    fn compact_provider_messages(
        messages: Vec<ChatMessage>,
        max_chars: usize,
    ) -> (Vec<ChatMessage>, bool) {
        let total_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        if total_chars <= max_chars {
            return (messages, false);
        }

        let mut system_messages = Vec::new();
        let mut dialog_messages = Vec::new();
        for message in messages {
            if message.role == ChatRole::System {
                system_messages.push(message);
            } else {
                dialog_messages.push(message);
            }
        }

        let system_chars: usize = system_messages
            .iter()
            .map(|m| m.content.chars().count())
            .sum();

        let kept_dialog_budget = max_chars
            .saturating_sub(system_chars)
            .saturating_sub(Self::COMPACTION_SUMMARY_CHAR_BUDGET)
            .max(1_500);

        let mut kept_dialog_reversed = Vec::new();
        let mut kept_chars = 0usize;

        for message in dialog_messages.iter().rev() {
            let len = message.content.chars().count();
            if kept_dialog_reversed.is_empty() || kept_chars + len <= kept_dialog_budget {
                kept_chars += len;
                kept_dialog_reversed.push(message.clone());
            } else {
                break;
            }
        }

        kept_dialog_reversed.reverse();
        let dropped_count = dialog_messages
            .len()
            .saturating_sub(kept_dialog_reversed.len());

        if dropped_count == 0 {
            let mut merged = system_messages;
            merged.extend(kept_dialog_reversed);
            return (merged, false);
        }

        let dropped = &dialog_messages[..dropped_count];
        let mut summary_lines = Vec::new();
        for message in dropped.iter().take(18) {
            let role = match message.role {
                ChatRole::User => "user",
                ChatRole::Assistant => "assistant",
                ChatRole::Tool => "tool",
                ChatRole::System => "system",
            };

            let snippet = message
                .content
                .replace('\n', " ")
                .chars()
                .take(220)
                .collect::<String>();
            summary_lines.push(format!("- {role}: {snippet}"));
        }

        let mut compacted = system_messages;
        compacted.push(ChatMessage {
            role: ChatRole::System,
            content: format!(
                "Conversation summary (auto-compacted to fit context window):\n{}",
                summary_lines.join("\n")
            ),
            tool_call_id: None,
        });
        compacted.extend(kept_dialog_reversed);

        (compacted, true)
    }

    pub(super) fn scroll_messages_to_bottom(&self) {
        self.messages_scroll_handle.scroll_to_bottom();
    }

    pub(super) fn message_row_height(message: &ChatMessage) -> Pixels {
        let explicit_lines = message.content.lines().collect::<Vec<_>>();
        let visual_lines: usize = explicit_lines
            .iter()
            .map(|line| {
                // Use a conservative wrap estimate so rows never under-size and overlap.
                let chars = line.chars().count().max(1);
                chars.div_ceil(64)
            })
            .sum::<usize>()
            .max(1);

        // Header + paddings + line-height budget + row gap.
        let estimated = 10.0 + 14.0 + 14.0 + (visual_lines as f32 * 18.0) + 6.0;
        px(estimated.min(520.0))
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

    pub(super) fn send_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_request_in_flight {
            println!("[agent_chat] send_prompt ignored: request already in flight");
            return;
        }

        let prompt = self.prompt_input.read(cx).text().to_string();
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return;
        }

        let request_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        self.messages.push(ChatMessage {
            role: ChatRole::User,
            content: prompt.clone(),
            tool_call_id: None,
        });
        self.scroll_messages_to_bottom();

        let provider_id = self
            .active_provider()
            .map(|p| p.id)
            .unwrap_or("unknown_provider");
        println!(
            "[agent_chat][request={}] send_prompt started provider={} prompt_len={}",
            request_id,
            provider_id,
            prompt.len()
        );

        if self.wip_providers.contains_key(provider_id) {
            self.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: "Selected provider is still WIP and not yet executable.".to_string(),
                tool_call_id: None,
            });
            self.prompt_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
            self.save_current_chat();
            self.refresh_chat_history_list(cx);
            self.scroll_messages_to_bottom();
            cx.notify();
            return;
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
            });
            self.prompt_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
            self.save_current_chat();
            self.refresh_chat_history_list(cx);
            self.scroll_messages_to_bottom();
            cx.notify();
            return;
        };

        let token = self.auth_token_for_provider(provider_id);
        let model = self
            .active_model()
            .map(|m| m.id.to_string())
            .unwrap_or_else(|| "default".to_string());
        let availability = provider.availability(&ProcessEnvironment);

        if matches!(availability.state, AvailabilityState::RequiresAuth) && token.is_none() {
            self.pending_auth_provider = Some(provider_id);
            self.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: "Authentication required. Paste token in the auth row above.".to_string(),
                tool_call_id: None,
            });
            self.prompt_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
            self.save_current_chat();
            self.refresh_chat_history_list(cx);
            self.scroll_messages_to_bottom();
            cx.notify();
            return;
        }

        let message_ix = self.messages.len();
        self.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content: String::new(),
            tool_call_id: None,
        });
        self.is_request_in_flight = true;
        self.streaming_message_ix = Some(message_ix);

        let provider_messages = self.build_provider_history_messages();
        let (provider_messages, was_compacted) =
            Self::compact_provider_messages(provider_messages, Self::CONTEXT_CHAR_BUDGET);

        let token = token.unwrap_or_default();
        let tool_schemas = self.tool_registry.available_tools_schema();
        
        // Validate and convert tool schemas
        let tools: Vec<agent_chat_core::ToolDefinition> = tool_schemas
            .iter()
            .filter_map(|schema| {
                let name = schema.get("name")?.as_str()?.to_string();
                let description = schema.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());
                let params = schema.get("parameters")?.clone();
                
                // Validate that parameters is an object with a "type" field
                if params.get("type").and_then(|v| v.as_str()) != Some("object") {
                    eprintln!("[agent_chat] WARNING: Tool {} has invalid parameters schema, skipping", name);
                    return None;
                }
                
                Some(agent_chat_core::ToolDefinition {
                    name,
                    description,
                    parameters_json_schema: params,
                })
            })
            .collect();
        
        eprintln!("[agent_chat] Validated {} tools for sending to provider", tools.len());
        
        let request = ChatRequest {
            model,
            messages: provider_messages,
            // Enable tool calls for agentic loop
            enable_tool_calls: !tools.is_empty(),
            tools,
            temperature: Some(0.2),
            top_p: Some(1.0),
            max_tokens: Some(1024),
        };
        println!(
            "[agent_chat][request={}] dispatched provider={} model={} entering in-flight compacted={} message_count={}",
            request_id,
            provider_id,
            request.model,
            was_compacted,
            request.messages.len()
        );

        enum StreamEvent {
            Chunk(String),
            Finished(Result<agent_chat_core::ChatResponse, String>),
        }

        let (tx, rx) = smol::channel::unbounded::<StreamEvent>();
        let tx_for_chunks = tx.clone();
        let tx_for_finish = tx.clone();
        let provider_for_task = provider.clone();
        let tool_registry_for_task = self.tool_registry.clone();
        let completion_sent = Arc::new(AtomicBool::new(false));

        let completion_for_worker = completion_sent.clone();
        let worker_request_id = request_id;
        std::thread::spawn(move || {
            println!(
                "[agent_chat][request={}] background worker started",
                worker_request_id
            );
            
            // Agentic loop: keep requesting until no more tool calls
            let mut current_messages = request.messages.clone();
            let mut iteration = 0;
            const MAX_ITERATIONS: usize = 10;
            
            loop {
                if iteration >= MAX_ITERATIONS {
                    println!(
                        "[agent_chat][request={}] max iterations reached",
                        worker_request_id
                    );
                    break;
                }
                iteration += 1;
                
                println!(
                    "[agent_chat][request={}] iteration {} starting with {} messages",
                    worker_request_id,
                    iteration,
                    current_messages.len()
                );
                
                for (idx, msg) in current_messages.iter().enumerate() {
                    println!(
                        "[agent_chat][request={}] msg[{}]: role={:?}, content_len={}, tool_call_id={:?}",
                        worker_request_id,
                        idx,
                        msg.role,
                        msg.content.len(),
                        msg.tool_call_id.as_deref()
                    );
                }
                
                let mut current_request = ChatRequest {
                    model: request.model.clone(),
                    messages: current_messages.clone(),
                    enable_tool_calls: request.enable_tool_calls,
                    tools: request.tools.clone(),
                    temperature: request.temperature,
                    top_p: request.top_p,
                    max_tokens: request.max_tokens,
                };
                
                // DEBUG: Log exact request details
                println!(
                    "[agent_chat][request={}] sending to provider: enable_tool_calls={}, tools_count={}",
                    worker_request_id,
                    current_request.enable_tool_calls,
                    current_request.tools.len()
                );
                
                for (idx, msg) in current_request.messages.iter().enumerate() {
                    if msg.role == ChatRole::Tool {
                        println!(
                            "[agent_chat][request={}] TOOL MESSAGE[{}]: tool_call_id={:?}, content_len={}",
                            worker_request_id,
                            idx,
                            msg.tool_call_id.as_deref(),
                            msg.content.len()
                        );
                    }
                }
                
                let mut pending_chunk = String::new();
                let mut last_emit = Instant::now();
                let mut on_chunk = |chunk: String| {
                    pending_chunk.push_str(&chunk);

                    let should_emit = pending_chunk.len() >= 256
                        || pending_chunk.contains('\n')
                        || last_emit.elapsed() >= Duration::from_millis(24);

                    if should_emit {
                        let chunk = std::mem::take(&mut pending_chunk);
                        let _ = tx_for_chunks.try_send(StreamEvent::Chunk(chunk));
                        last_emit = Instant::now();
                    }
                };

                let result = provider_for_task
                    .chat_completion_streaming(&token, &current_request, &mut on_chunk)
                    .map_err(|err| err.to_string());

                if !pending_chunk.is_empty() {
                    let _ = tx_for_chunks.try_send(StreamEvent::Chunk(pending_chunk));
                }

                match result {
                    Ok(response) => {
                        println!(
                            "[agent_chat][request={}] iteration {} provider response: assistant_msg={}, tool_calls={}, finish_reason={:?}",
                            worker_request_id,
                            iteration,
                            response.assistant_message.as_ref().map(|m| m.len()).unwrap_or(0),
                            response.tool_calls.len(),
                            response.finish_reason
                        );
                        
                        // Check if response has tool calls
                        if !response.tool_calls.is_empty() {
                            println!(
                                "[agent_chat][request={}] iteration {} got {} tool calls",
                                worker_request_id,
                                iteration,
                                response.tool_calls.len()
                            );
                            
                            // Always add assistant message (even if empty) so tool results can follow
                            let assistant_text = response.assistant_message.clone().unwrap_or_default();
                            current_messages.push(ChatMessage {
                                role: ChatRole::Assistant,
                                content: assistant_text.clone(),
                                tool_call_id: None,
                            });
                            
                            // Show assistant text to user (if any)
                            if !assistant_text.is_empty() {
                                let _ = tx_for_chunks.try_send(StreamEvent::Chunk(assistant_text));
                            }
                            
                            // Show tool calls as messages to user (italicized in UI)
                            let tool_calls_text = response.tool_calls
                                .iter()
                                .map(|call| format!("*Calling {}*", call.name))
                                .collect::<Vec<_>>()
                                .join("\n");
                            let _ = tx_for_chunks.try_send(StreamEvent::Chunk(format!("\n\n{}\n\n", tool_calls_text)));
                            
                            // Create tool context for execution
                            let workspace_root = match engine_state::get_project_path() {
                                Some(path) => PathBuf::from(path),
                                None => PathBuf::from("."),
                            };
                            let tool_context = agent_chat_tools::ToolContext {
                                workspace_root,
                                plugin_bridge: None,
                                current_file: None,
                            };
                            
                            // Execute each tool call and show results
                            let mut all_results = Vec::new();
                            for tool_call in &response.tool_calls {
                                println!(
                                    "[agent_chat][request={}] executing tool: {}",
                                    worker_request_id, tool_call.name
                                );
                                
                                let result = tool_registry_for_task.execute(
                                    &tool_call.name,
                                    tool_call.arguments_json.clone(),
                                    &tool_context,
                                );
                                
                                let tool_result = match result {
                                    Ok(value) => value.to_string(),
                                    Err(err) => format!("Tool error: {}", err),
                                };
                                
                                // Show tool result to user
                                let result_display = format!(
                                    "**{}**: {}\n\n",
                                    tool_call.name,
                                    tool_result.chars().take(200).collect::<String>()
                                );
                                let _ = tx_for_chunks.try_send(StreamEvent::Chunk(result_display));
                                
                                // Store tool result with call ID for proper threading
                                all_results.push((tool_call.id.clone(), tool_call.name.clone(), tool_result));
                            }
                            
                            // Add all tool results to message history for LLM to read
                            println!(
                                "[agent_chat][request={}] adding {} tool results to history",
                                worker_request_id,
                                all_results.len()
                            );
                            for (tool_call_id, tool_name, tool_result) in all_results {
                                println!(
                                    "[agent_chat][request={}] adding tool result: id={}, name={}",
                                    worker_request_id, tool_call_id, tool_name
                                );
                                current_messages.push(ChatMessage {
                                    role: ChatRole::Tool,
                                    content: tool_result,
                                    tool_call_id: Some(tool_call_id),
                                });
                            }
                            
                            println!(
                                "[agent_chat][request={}] total messages before provider call: {}",
                                worker_request_id,
                                current_messages.len()
                            );
                            
                            // Loop again with updated message history
                            continue;
                        } else {
                            // No tool calls, finish with response
                            println!(
                                "[agent_chat][request={}] iteration {} finishing: msg_len={}, finish_reason={:?}",
                                worker_request_id,
                                iteration,
                                response.assistant_message.as_ref().map(|m| m.len()).unwrap_or(0),
                                response.finish_reason
                            );
                            if !completion_for_worker.swap(true, Ordering::SeqCst) {
                                println!(
                                    "[agent_chat][request={}] iteration {} finished (no tool calls)",
                                    worker_request_id, iteration
                                );
                                let _ = tx_for_finish.try_send(StreamEvent::Finished(Ok(response)));
                            }
                            break;
                        }
                    }
                    Err(err) => {
                        if !completion_for_worker.swap(true, Ordering::SeqCst) {
                            println!(
                                "[agent_chat][request={}] iteration {} error: {}",
                                worker_request_id, iteration, err
                            );
                            let _ = tx_for_finish.try_send(StreamEvent::Finished(Err(err)));
                        }
                        break;
                    }
                }
            }
        });

        let tx_timeout = tx.clone();
        let completion_for_timeout = completion_sent.clone();
        let timeout_request_id = request_id;
        cx.background_spawn(async move {
            Timer::after(Duration::from_secs(75)).await;
            if !completion_for_timeout.swap(true, Ordering::SeqCst) {
                eprintln!(
                    "[agent_chat][request={}] watchdog timeout fired",
                    timeout_request_id
                );
                let _ = tx_timeout.try_send(StreamEvent::Finished(Err(
                    "Provider response timed out.".to_string(),
                )));
            }
        })
        .detach();

        let consume_request_id = request_id;
        cx.spawn(async move |this, cx| {
            let mut saw_first_chunk = false;
            while let Ok(event) = rx.recv().await {
                let should_break = matches!(event, StreamEvent::Finished(_));
                cx.update(|cx| {
                    this.update(cx, |panel, cx| {
                        match event {
                            StreamEvent::Chunk(chunk) => {
                                if !saw_first_chunk {
                                    println!(
                                        "[agent_chat][request={}] first chunk received len={}",
                                        consume_request_id,
                                        chunk.len()
                                    );
                                    saw_first_chunk = true;
                                }
                                if panel.is_request_in_flight {
                                    panel.is_request_in_flight = false;
                                    println!(
                                        "[agent_chat][request={}] in-flight cleared on first chunk",
                                        consume_request_id
                                    );
                                }
                                if let Some(message) = panel.messages.get_mut(message_ix) {
                                    message.content.push_str(&chunk);
                                    panel.message_row_heights.remove(&message_ix);
                                }
                                panel.scroll_messages_to_bottom();
                                cx.notify();
                            }
                            StreamEvent::Finished(Ok(response)) => {
                                panel.is_request_in_flight = false;
                                panel.streaming_message_ix = None;
                                println!(
                                    "[agent_chat][request={}] stream finished ok had_chunks={} fallback_msg={}",
                                    consume_request_id,
                                    saw_first_chunk,
                                    response.assistant_message.is_some()
                                );

                                if let Some(message) = panel.messages.get_mut(message_ix) {
                                    if message.content.is_empty() {
                                        if let Some(text) = response.assistant_message {
                                            message.content = text;
                                        } else {
                                            message.content =
                                                "Provider returned an empty response.".to_string();
                                        }
                                    }
                                    panel.message_row_heights.remove(&message_ix);
                                }
                                panel.save_current_chat();
                                panel.refresh_chat_history_list(cx);
                                panel.scroll_messages_to_bottom();
                                cx.notify();
                            }
                            StreamEvent::Finished(Err(err)) => {
                                panel.is_request_in_flight = false;
                                panel.streaming_message_ix = None;
                                eprintln!(
                                    "[agent_chat][request={}] stream finished with error: {}",
                                    consume_request_id,
                                    err
                                );
                                if let Some(message) = panel.messages.get_mut(message_ix) {
                                    message.content = format!("Provider request failed: {err}");
                                    panel.message_row_heights.remove(&message_ix);
                                }
                                panel.save_current_chat();
                                panel.refresh_chat_history_list(cx);
                                panel.scroll_messages_to_bottom();
                                cx.notify();
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

        self.prompt_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.save_current_chat();
        self.refresh_chat_history_list(cx);
        self.scroll_messages_to_bottom();
        cx.notify();
    }
}
