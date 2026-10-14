//! The panel's side of delegation. Sub-agents are child runs owned by the
//! chat's `DelegationHost`; this file only turns their `RunEvent`s into
//! `SubagentInvocation` cards and hands finished background reports to the
//! ledger. There is no polling and no state machine over step text.

use agent_runtime::{
    deliver_pending, DelegationHost, DelegationLimits, EventSink, HarnessRegistry, RunEvent,
    RunStatus, RunStore, SubagentStatus, ROOT_RUN_ID,
};
use gpui::*;
use smol::channel;
use std::{path::PathBuf, sync::Arc};

use super::panel::AgentChatPanel;
use super::types::*;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn step_status(status: SubagentStatus) -> SubagentStepStatus {
    match status {
        SubagentStatus::Success => SubagentStepStatus::Success,
        SubagentStatus::Error | SubagentStatus::Cancelled => SubagentStepStatus::Error,
    }
}

impl AgentChatPanel {
    /// A host that is not tied to a chat yet (the panel before its first
    /// `attach_delegation`).
    pub(super) fn detached_delegation() -> Arc<DelegationHost> {
        DelegationHost::new(
            RunStore::in_memory(),
            HarnessRegistry::builtin(),
            DelegationLimits::default(),
            Arc::new(|_| {}),
        )
    }

    /// `<config dir>/pulsar/agents`, the user-global harness folder.
    fn global_agents_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("pulsar").join("agents"))
    }

    /// Built-ins, then the global folder, then the project's `.pulsar/agents`.
    pub(super) fn load_harnesses() -> HarnessRegistry {
        let project_root = engine_state::get_project_path().map(PathBuf::from);
        let registry = HarnessRegistry::load(
            project_root.as_deref(),
            Self::global_agents_dir().as_deref(),
        );
        for (path, err) in &registry.errors {
            tracing::warn!("[agent_chat] ignoring harness {}: {err}", path.display());
        }
        registry
    }

    /// Open (or create) the run store that sits next to the current chat file
    /// and listen to its events. Call whenever `current_chat_id` changes.
    pub(super) fn attach_delegation(&mut self, cx: &mut Context<Self>) {
        // Stop whatever the previous chat still has running.
        self.delegation.cancel_all();

        let store = match Self::ensure_chats_dir() {
            Some(dir) if !self.current_chat_id.is_empty() => {
                RunStore::open(RunStore::path_for_chat(&dir, &self.current_chat_id))
            }
            _ => RunStore::in_memory(),
        };

        let (tx, rx) = channel::unbounded::<RunEvent>();
        let events: EventSink = Arc::new(move |event| {
            let _ = tx.try_send(event);
        });
        self.delegation = DelegationHost::new(
            store,
            Self::load_harnesses(),
            DelegationLimits::default(),
            events,
        );

        let chat_id = self.current_chat_id.clone();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let alive = cx.update(|cx| {
                    this.update(cx, |panel, cx| {
                        panel.apply_run_event(&chat_id, event, cx);
                    })
                    .is_ok()
                });
                if !alive {
                    break;
                }
            }
        })
        .detach();

        self.reconcile_subagent_cards();
        if self.deliver_idle_subagent_reports() {
            self.save_current_chat();
        }
    }

    /// Number of sub-agents that have not finished.
    pub(super) fn running_subagent_count(&self) -> usize {
        self.delegation.store().active().len()
    }

    fn apply_run_event(&mut self, chat_id: &str, event: RunEvent, cx: &mut Context<Self>) {
        // Events of a chat the user has since left only matter once it is
        // reopened, and then the store has the final state.
        if chat_id != self.current_chat_id {
            return;
        }
        let now = now_ms();
        match event {
            RunEvent::ChildSpawned {
                run_id,
                harness,
                task,
                ..
            } => {
                if self.subagent_card(&run_id).is_some() {
                    return;
                }
                self.display_items.push(DisplayItem::SubagentInvocation {
                    subagent_id: run_id.clone(),
                    name: harness,
                    task,
                    steps: vec![SubagentStepDisplay {
                        id: format!("{run_id}:0"),
                        description: "Started".to_string(),
                        details: String::new(),
                        status: SubagentStepStatus::Running,
                        started_at_ms: now,
                        finished_at_ms: None,
                    }],
                    is_expanded: false,
                    status: SubagentStepStatus::Running,
                    started_at_ms: now,
                    finished_at_ms: None,
                });
                self.scroll_messages_to_bottom();
            }
            RunEvent::ChildProgress { run_id, note } => {
                if let Some(DisplayItem::SubagentInvocation { steps, .. }) =
                    self.subagent_card(&run_id)
                {
                    if let Some(last) = steps.last_mut() {
                        if last.finished_at_ms.is_none() {
                            last.status = SubagentStepStatus::Success;
                            last.finished_at_ms = Some(now);
                        }
                    }
                    steps.push(SubagentStepDisplay {
                        id: format!("{run_id}:{}", steps.len()),
                        description: note,
                        details: String::new(),
                        status: SubagentStepStatus::Running,
                        started_at_ms: now,
                        finished_at_ms: None,
                    });
                }
            }
            RunEvent::ChildFinished {
                run_id,
                status,
                summary,
            } => {
                if let Some(DisplayItem::SubagentInvocation {
                    steps,
                    status: card_status,
                    finished_at_ms,
                    ..
                }) = self.subagent_card(&run_id)
                {
                    let outcome = step_status(status);
                    if let Some(last) = steps.last_mut() {
                        if last.finished_at_ms.is_none() {
                            last.status = outcome;
                            last.finished_at_ms = Some(now);
                        }
                    }
                    steps.push(SubagentStepDisplay {
                        id: format!("{run_id}:{}", steps.len()),
                        description: format!("Finished ({})", status.as_str()),
                        details: summary,
                        status: outcome,
                        started_at_ms: now,
                        finished_at_ms: Some(now),
                    });
                    *card_status = outcome;
                    *finished_at_ms = Some(now);
                }
                // A finished background run is seen by the model on its next
                // request; nothing is sent to it now.
                self.deliver_idle_subagent_reports();
                self.save_current_chat();
            }
            _ => {}
        }
        cx.notify();
    }

    fn subagent_card(&mut self, run_id: &str) -> Option<&mut DisplayItem> {
        self.display_items.iter_mut().find(|item| {
            matches!(
                item,
                DisplayItem::SubagentInvocation { subagent_id, .. } if subagent_id == run_id
            )
        })
    }

    /// Append finished background reports to the ledger, unless a request is
    /// running (that run's own tools deliver them). Returns whether anything
    /// was added.
    pub(super) fn deliver_idle_subagent_reports(&mut self) -> bool {
        if self.is_request_in_flight {
            return false;
        }
        deliver_pending(self.delegation.store(), ROOT_RUN_ID, &mut self.ledger) > 0
    }

    /// Cards saved while their run was going (an old chat, or one that was
    /// open when the editor closed) take their final state from the store.
    fn reconcile_subagent_cards(&mut self) {
        let store = self.delegation.store().clone();
        let now = now_ms();
        for item in self.display_items.iter_mut() {
            let DisplayItem::SubagentInvocation {
                subagent_id,
                steps,
                status,
                finished_at_ms,
                ..
            } = item
            else {
                continue;
            };
            if *status != SubagentStepStatus::Running && *status != SubagentStepStatus::Pending {
                continue;
            }
            let (outcome, details) = match store.get(subagent_id) {
                Some(run) if run.status == RunStatus::Success => (
                    SubagentStepStatus::Success,
                    run.report.map(|r| r.summary).unwrap_or_default(),
                ),
                Some(run) if run.is_active() => continue,
                Some(run) => (
                    SubagentStepStatus::Error,
                    run.report.map(|r| r.summary).unwrap_or_default(),
                ),
                // Cards from before runs were stored: the work is gone.
                None => (
                    SubagentStepStatus::Error,
                    "This sub-agent did not finish before the chat was closed.".to_string(),
                ),
            };
            for step in steps.iter_mut() {
                if step.finished_at_ms.is_none() {
                    step.status = outcome;
                    step.finished_at_ms = Some(now);
                }
            }
            if !details.is_empty() {
                steps.push(SubagentStepDisplay {
                    id: format!("{subagent_id}:final"),
                    description: "Finished".to_string(),
                    details,
                    status: outcome,
                    started_at_ms: now,
                    finished_at_ms: Some(now),
                });
            }
            *status = outcome;
            *finished_at_ms = Some(now);
        }
    }
}
