use std::collections::VecDeque;
use gpui::*;

use super::panel::AgentChatPanel;
use super::panel::SubagentCompletionMode;
use super::types::*;
use super::context;

impl AgentChatPanel {
    pub(super) fn update_subagent_invocation_started(
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
            details: "Subagent execution started. Completion will be queued when ready."
                .to_string(),
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

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let finished_at_ms = event
            .get("finished_at_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(now);
        let status_raw = event
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("success");
        let status = match status_raw {
            "error" | "cancelled" => SubagentStepStatus::Error,
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

    fn launch_internal_agent_event(&mut self, content: String, cx: &mut Context<Self>) -> bool {
        use agent_chat_core::ChatMessage;
        let provider_id = self
            .active_provider()
            .map(|p| p.id)
            .unwrap_or("unknown_provider");

        let Some(provider) = self.provider_registry.get(provider_id).cloned() else {
            return false;
        };

        self.messages.push(ChatMessage {
            role: agent_chat_core::ChatRole::AgentEvent,
            content,
            tool_call_id: None,
            tool_calls: vec![],
        });

        self.launch_provider_request(provider, None, cx);
        true
    }

    pub(super) fn process_next_subagent_completion_now(&mut self, cx: &mut Context<Self>) {
        if self.is_request_in_flight || self.is_processing_subagent_event {
            return;
        }
        self.maybe_start_next_subagent_processing(cx);
    }

    pub(super) fn complete_active_subagent_processing(&mut self, success: bool) {
        let Some(subagent_id) = self.processing_subagent_id.clone() else {
            return;
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

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
                                "Main agent processing failed; completion remains visible."
                                    .to_string()
                            };
                            last.finished_at_ms = Some(now);
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
}
