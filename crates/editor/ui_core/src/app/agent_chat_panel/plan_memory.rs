//! Panel side of plan mode and project memory.
//!
//! The runtime does the work (`agent_runtime::plan`, `agent_runtime::memory`).
//! Here: the Plan toggle, the plan card (Approve / Edit / Reject), the memory
//! proposals (Accept / Reject) and the memory editor. Nothing the model or
//! the steward proposes is applied until the user presses a button here.

use super::*;
use agent_runtime::{
    MemoryContext, MemoryHook, MemoryInbox, MemoryScope, MemoryStore, Plan, PlanMode, RunConfig,
};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
};

/// What the panel keeps for plan mode and memory.
pub struct PlanMemoryState {
    /// "Plan" toggle: the next request is a read-only planning run.
    pub plan_mode: bool,
    /// The slot of the planning run in flight.
    run_slot: Option<PlanMode>,
    /// A submitted plan waiting for Approve / Edit / Reject.
    pub submitted: Option<Plan>,
    pub editing_plan: bool,
    pub plan_error: Option<String>,
    pub plan_edit_input: Entity<InputState>,
    /// Proposals from `remember` and the steward, shared with the runs.
    pub inbox: MemoryInbox,
    pub memory_open: bool,
    pub memory_scope: MemoryScope,
    pub memory_status: Option<String>,
    pub memory_edit_input: Entity<InputState>,
    /// Project files (memory, harnesses, MCP servers) waiting for the user's
    /// trust; see `trust.rs`.
    pub pending_trust: Vec<agent_runtime::trust::PendingItem>,
}

impl PlanMemoryState {
    pub fn new(plan_edit_input: Entity<InputState>, memory_edit_input: Entity<InputState>) -> Self {
        Self {
            plan_mode: false,
            run_slot: None,
            submitted: None,
            editing_plan: false,
            plan_error: None,
            plan_edit_input,
            inbox: MemoryInbox::new(),
            memory_open: false,
            memory_scope: MemoryScope::Project,
            memory_status: None,
            memory_edit_input,
            pending_trust: Vec::new(),
        }
    }

    /// New chat or new request: a plan that was never decided is dropped.
    pub fn clear_plan(&mut self) {
        self.submitted = None;
        self.editing_plan = false;
        self.plan_error = None;
        self.run_slot = None;
    }
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{}\u{2026}", cut.trim_end())
    }
}

impl AgentChatPanel {
    fn memory_store() -> Option<MemoryStore> {
        let root = engine_state::get_project_path().map(PathBuf::from)?;
        let user_dir = dirs::config_dir().map(|dir| dir.join("pulsar"));
        // The project's memory file is only used once the user trusted it.
        Some(MemoryStore::new(&root, user_dir.as_deref()).with_trust(super::trust::trust_store(), &root))
    }

    /// Called for every request: plan mode when toggled, project memory
    /// always, and the steward's memory proposals when the steward is on.
    pub(super) fn configure_plan_memory(&mut self, config: &mut RunConfig) {
        // A new request supersedes an undecided plan.
        self.plan_memory.clear_plan();
        if self.plan_memory.plan_mode {
            let mode = PlanMode::new();
            self.plan_memory.run_slot = Some(mode.clone());
            config.plan_mode = Some(mode);
        }
        if let Some(store) = Self::memory_store() {
            if let Some(steward) = &config.steward {
                steward.enable_memory(MemoryHook {
                    store: store.clone(),
                    inbox: self.plan_memory.inbox.clone(),
                });
            }
            config.memory = Some(MemoryContext::new(store, self.plan_memory.inbox.clone()));
        }
    }

    /// The run ended: pick up the plan it submitted, if any.
    pub(super) fn collect_submitted_plan(&mut self, cx: &mut Context<Self>) {
        let Some(slot) = self.plan_memory.run_slot.take() else {
            return;
        };
        if let Some(plan) = slot.take() {
            self.plan_memory.submitted = Some(plan);
            self.scroll_messages_to_bottom();
            cx.notify();
        }
    }

    // ----- plan actions ------------------------------------------------------

    fn toggle_plan_mode(&mut self, cx: &mut Context<Self>) {
        self.plan_memory.plan_mode = !self.plan_memory.plan_mode;
        cx.notify();
    }

    fn approve_plan(&mut self, cx: &mut Context<Self>) {
        if self.is_request_in_flight {
            return;
        }
        let Some(plan) = self.plan_memory.submitted.take() else {
            return;
        };
        self.plan_memory.editing_plan = false;
        self.plan_memory.plan_error = None;
        // The approved plan becomes the user's pins, then an edit-capable
        // run starts (plan mode is off for it).
        plan.apply_to(&mut self.ledger);
        self.plan_memory.plan_mode = false;
        self.push_steward_log(format!("Approved plan: {}", clip(&plan.goal, 80)));
        let shown = format!("Approved plan: {}", plan.goal);
        let _ = self.submit_user_prompt_text(shown, plan.approval_message(), true, cx);
        cx.notify();
    }

    fn reject_plan(&mut self, cx: &mut Context<Self>) {
        if let Some(plan) = self.plan_memory.submitted.take() {
            self.push_steward_log(format!("Rejected plan: {}", clip(&plan.goal, 80)));
        }
        self.plan_memory.editing_plan = false;
        self.plan_memory.plan_error = None;
        cx.notify();
    }

    fn begin_plan_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(plan) = &self.plan_memory.submitted else {
            return;
        };
        let text = plan.to_markdown();
        self.plan_memory.editing_plan = true;
        self.plan_memory.plan_error = None;
        self.plan_memory
            .plan_edit_input
            .update(cx, |input, cx| input.set_value(&text, window, cx));
        cx.notify();
    }

    fn save_plan_edit(&mut self, cx: &mut Context<Self>) {
        let text = self.plan_memory.plan_edit_input.read(cx).text().to_string();
        match Plan::from_markdown(&text) {
            Ok(plan) => {
                self.plan_memory.submitted = Some(plan);
                self.plan_memory.editing_plan = false;
                self.plan_memory.plan_error = None;
            }
            Err(err) => self.plan_memory.plan_error = Some(err),
        }
        cx.notify();
    }

    // ----- memory actions ----------------------------------------------------

    fn accept_memory(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(proposal) = self.plan_memory.inbox.take(index) else {
            return;
        };
        let Some(store) = Self::memory_store() else {
            return;
        };
        match store.accept(&proposal, &proposal.text) {
            Ok(true) => self.push_steward_log(format!("Saved to memory: {}", clip(&proposal.text, 80))),
            Ok(false) => self.push_steward_log("Memory already has that note (or is full)."),
            Err(err) => self.push_steward_log(format!("Could not save memory: {err}")),
        }
        cx.notify();
    }

    fn reject_memory(&mut self, index: usize, cx: &mut Context<Self>) {
        self.plan_memory.inbox.take(index);
        cx.notify();
    }

    fn load_memory_into_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = Self::memory_store()
            .map(|store| store.read(self.plan_memory.memory_scope))
            .unwrap_or_default();
        self.plan_memory
            .memory_edit_input
            .update(cx, |input, cx| input.set_value(&text, window, cx));
    }

    fn toggle_memory_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.plan_memory.memory_open = !self.plan_memory.memory_open;
        self.plan_memory.memory_status = None;
        if self.plan_memory.memory_open {
            self.load_memory_into_editor(window, cx);
        }
        cx.notify();
    }

    fn set_memory_scope(&mut self, scope: MemoryScope, window: &mut Window, cx: &mut Context<Self>) {
        self.plan_memory.memory_scope = scope;
        self.plan_memory.memory_status = None;
        self.load_memory_into_editor(window, cx);
        cx.notify();
    }

    fn save_memory(&mut self, cx: &mut Context<Self>) {
        let text = self.plan_memory.memory_edit_input.read(cx).text().to_string();
        let scope = self.plan_memory.memory_scope;
        self.plan_memory.memory_status = Some(match Self::memory_store() {
            Some(store) => match store.write(scope, &text) {
                Ok(()) => "Saved. New chats and requests use it.".to_string(),
                Err(err) => format!("Could not save: {err}"),
            },
            None => "No project is open.".to_string(),
        });
        cx.notify();
    }

    // ----- rendering ---------------------------------------------------------

    /// The Plan and Memory buttons for the controls row.
    pub(super) fn render_plan_memory_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.plan_memory.plan_mode;
        let pending = self.plan_memory.inbox.len();
        h_flex()
            .gap_1()
            .child({
                let button = Button::new("agent-chat-plan-mode")
                    .xsmall()
                    .label(if on { "Plan: on" } else { "Plan: off" })
                    .tooltip(
                        "Plan mode: the next request runs read-only and ends with a plan you can \
                         approve, edit or reject. Nothing is changed until you approve.",
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_plan_mode(cx)));
                if on {
                    button.primary()
                } else {
                    button.ghost()
                }
            })
            .child(
                Button::new("agent-chat-memory")
                    .xsmall()
                    .ghost()
                    .label(if pending > 0 {
                        format!("Memory ({pending})")
                    } else {
                        "Memory".to_string()
                    })
                    .tooltip("View and edit the notes loaded into every chat (.pulsar/agent_memory.md).")
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_memory_editor(window, cx))),
            )
    }

    /// The submitted plan, or `None` when there is none.
    pub(super) fn render_plan_card(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let plan = self.plan_memory.submitted.clone()?;
        let muted = cx.theme().muted_foreground;
        let warn = cx.theme().warning;
        let mut card = v_flex()
            .id("agent-chat-plan-card")
            .w_full()
            .max_h(px(320.0))
            .overflow_y_scroll()
            .gap_1()
            .p_2()
            .mb_2()
            .rounded(px(6.0))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Plan for review (read-only run, nothing changed yet)"),
            );

        if self.plan_memory.editing_plan {
            card = card.child(TextInput::new(&self.plan_memory.plan_edit_input).w_full());
            if let Some(err) = &self.plan_memory.plan_error {
                card = card.child(div().text_xs().text_color(warn).child(err.clone()));
            }
            return Some(
                card.child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("plan-edit-save")
                                .xsmall()
                                .primary()
                                .label("Save")
                                .on_click(cx.listener(|this, _, _, cx| this.save_plan_edit(cx))),
                        )
                        .child(
                            Button::new("plan-edit-cancel")
                                .xsmall()
                                .ghost()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.plan_memory.editing_plan = false;
                                    this.plan_memory.plan_error = None;
                                    cx.notify();
                                })),
                        ),
                ),
            );
        }

        card = card.child(div().text_xs().child(format!("Goal: {}", plan.goal)));
        for (i, step) in plan.steps.iter().enumerate() {
            let mut column = v_flex().w_full().child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(format!("{}. {}", i + 1, step.title)),
            );
            if !step.detail.is_empty() {
                column = column.child(div().text_xs().text_color(muted).pl_3().child(step.detail.clone()));
            }
            if !step.files.is_empty() {
                column = column.child(
                    div()
                        .text_xs()
                        .text_color(muted.opacity(0.8))
                        .pl_3()
                        .child(format!("files: {}", step.files.join(", "))),
                );
            }
            card = card.child(column);
        }
        for risk in &plan.risks {
            card = card.child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(Icon::new(IconName::TriangleAlert).size_3().text_color(warn))
                    .child(div().flex_1().min_w_0().text_xs().text_color(warn).child(risk.clone())),
            );
        }
        Some(
            card.child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("plan-approve")
                            .xsmall()
                            .primary()
                            .label("Approve and start")
                            .tooltip("Pin this plan and start an edit-capable run that follows it")
                            .disabled(self.is_request_in_flight)
                            .on_click(cx.listener(|this, _, _, cx| this.approve_plan(cx))),
                    )
                    .child(
                        Button::new("plan-edit")
                            .xsmall()
                            .ghost()
                            .label("Edit")
                            .on_click(cx.listener(|this, _, window, cx| this.begin_plan_edit(window, cx))),
                    )
                    .child(
                        Button::new("plan-reject")
                            .xsmall()
                            .ghost()
                            .label("Reject")
                            .on_click(cx.listener(|this, _, _, cx| this.reject_plan(cx))),
                    ),
            ),
        )
    }

    /// Memory proposals and the memory editor, or `None` when neither shows.
    pub(super) fn render_memory_strip(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let proposals = self.plan_memory.inbox.snapshot();
        if proposals.is_empty() && !self.plan_memory.memory_open {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        let warn = cx.theme().warning;
        let mut strip = v_flex()
            .id("agent-chat-memory-strip")
            .w_full()
            .max_h(px(260.0))
            .overflow_y_scroll()
            .gap_1()
            .p_2()
            .mb_2()
            .rounded(px(6.0))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Project memory"),
            );

        for (i, proposal) in proposals.iter().enumerate() {
            let untrusted = proposal.untrusted;
            strip = strip.child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .child(
                        Icon::new(if untrusted { IconName::TriangleAlert } else { IconName::Plus })
                            .size_3()
                            .text_color(if untrusted { warn } else { muted }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .when(untrusted, |el| el.text_color(warn))
                            .child(format!(
                                "{} suggests remembering ({}): {}{}",
                                proposal.origin.label(),
                                proposal.scope.label(),
                                proposal.text,
                                if untrusted { " (made while untrusted content was in context)" } else { "" }
                            )),
                    )
                    .child(
                        Button::new(SharedString::from(format!("mem-ok-{i}")))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Check)
                            .tooltip("Save to memory")
                            .on_click(cx.listener(move |this, _, _, cx| this.accept_memory(i, cx))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("mem-no-{i}")))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Discard")
                            .on_click(cx.listener(move |this, _, _, cx| this.reject_memory(i, cx))),
                    ),
            );
        }

        if self.plan_memory.memory_open {
            let scope = self.plan_memory.memory_scope;
            let has_user = Self::memory_store().is_some_and(|s| s.has_user_file());
            let scope_button = |id: &'static str, label: &'static str, target: MemoryScope, cx: &mut Context<Self>| {
                let button = Button::new(id)
                    .xsmall()
                    .label(label)
                    .on_click(cx.listener(move |this, _, window, cx| this.set_memory_scope(target, window, cx)));
                if scope == target {
                    button.primary()
                } else {
                    button.ghost()
                }
            };
            let mut tabs = h_flex().gap_1().child(scope_button("mem-scope-project", "This project", MemoryScope::Project, cx));
            if has_user {
                tabs = tabs.child(scope_button("mem-scope-user", "All my projects", MemoryScope::User, cx));
            }
            strip = strip
                .child(tabs)
                .child(TextInput::new(&self.plan_memory.memory_edit_input).w_full())
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Button::new("mem-save")
                                .xsmall()
                                .primary()
                                .label("Save")
                                .on_click(cx.listener(|this, _, _, cx| this.save_memory(cx))),
                        )
                        .child(
                            Button::new("mem-close")
                                .xsmall()
                                .ghost()
                                .label("Close")
                                .on_click(cx.listener(|this, _, window, cx| this.toggle_memory_editor(window, cx))),
                        )
                        .children(
                            self.plan_memory
                                .memory_status
                                .clone()
                                .map(|status| div().text_xs().text_color(muted).child(status)),
                        ),
                );
        }
        Some(strip)
    }
}
