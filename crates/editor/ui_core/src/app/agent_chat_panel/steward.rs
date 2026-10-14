//! Panel side of the context steward: the "Pinned context" strip, pin
//! actions on messages, the steward setting and its event inbox.
//!
//! The runtime does the work (`agent_runtime::steward`, `agent_runtime::pins`);
//! this file only shows what it proposes and applies what the user accepts.

use super::*;
use agent_runtime::{
    apply_change, detect_pin_candidates, Acceptance, DriftWarning, PinAdd, PinAuthor, PinChange,
    PinKind, PinOp, Provenance, Steward, StewardConfig, StewardEvent, StewardSettings,
    StewardSink,
};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
};

/// Lines of the audit trail kept.
const MAX_LOG: usize = 6;

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
    fn project_root() -> Option<PathBuf> {
        engine_state::get_project_path().map(PathBuf::from)
    }

    pub(super) fn load_steward_settings() -> StewardSettings {
        Self::project_root()
            .map(|root| StewardSettings::load(&root))
            .unwrap_or_default()
    }

    fn save_steward_settings(&self) {
        if let Some(root) = Self::project_root() {
            if let Err(err) = self.steward_settings.save(&root) {
                tracing::warn!("[agent_chat] could not save steward settings: {err}");
            }
        }
    }

    /// The setting as it applies to this chat.
    pub(super) fn steward_enabled(&self) -> bool {
        self.steward_settings.enabled_for(self.chat_steward_override)
    }

    pub(super) fn push_steward_log(&mut self, line: impl Into<String>) {
        self.steward_log.push(line.into());
        if self.steward_log.len() > MAX_LOG {
            self.steward_log.remove(0);
        }
    }

    /// Chat switch: forget queued steward work and everything it proposed for
    /// the old chat.
    pub(super) fn reset_steward(&mut self) {
        if let Some(steward) = self.steward.take() {
            steward.cancel();
        }
        self.steward_key.clear();
        self.pending_pin_changes.clear();
        self.drift_warning = None;
        self.dismissed_pin_candidates.clear();
        self.pin_edit = None;
    }

    /// The steward for the next request, or `None` while the setting is off
    /// (in which case nothing is built and no extra request can happen).
    pub(super) fn ensure_steward(
        &mut self,
        provider: Arc<dyn agent_chat_core::ChatProvider>,
        model: String,
        json_mode: agent_runtime::JsonMode,
        cx: &mut Context<Self>,
    ) -> Option<Arc<Steward>> {
        if !self.steward_enabled() {
            self.reset_steward();
            return None;
        }
        let drift_check = self.steward_settings.drift_check;
        let key = format!("{}|{model}|{json_mode:?}|{drift_check}", provider.id());
        if self.steward_key == key {
            if let Some(steward) = &self.steward {
                return Some(steward.clone());
            }
        }
        self.reset_steward();

        let (tx, rx) = smol::channel::unbounded::<StewardEvent>();
        let sink: StewardSink = Arc::new(move |event| {
            let _ = tx.try_send(event);
        });
        let steward = Steward::new(
            StewardConfig {
                provider,
                model,
                enabled: true,
                drift_check,
                json_mode,
            },
            sink,
        );
        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                cx.update(|cx| {
                    let _ = this.update(cx, |panel, cx| panel.on_steward_event(event, cx));
                });
            }
        })
        .detach();
        self.steward = Some(steward.clone());
        self.steward_key = key;
        Some(steward)
    }

    fn on_steward_event(&mut self, event: StewardEvent, cx: &mut Context<Self>) {
        match event {
            StewardEvent::Proposals(diff) => {
                let mut changes = diff.changes;
                if self.steward_settings.auto_accept_trusted {
                    let applied_diff = agent_runtime::PinDiff { changes };
                    let (applied, held) = applied_diff.apply(&mut self.ledger, Acceptance::AutoTrusted);
                    if applied > 0 {
                        self.push_steward_log(format!("Applied {applied} trusted pin change(s)."));
                        self.save_current_chat();
                    }
                    changes = held;
                }
                for change in changes {
                    if !self.pending_pin_changes.contains(&change) {
                        self.pending_pin_changes.push(change);
                    }
                }
            }
            StewardEvent::Drift(warning) => self.drift_warning = Some(warning),
            StewardEvent::Notice(note) => self.push_steward_log(note),
            StewardEvent::Failed(err) => {
                tracing::warn!("[agent_chat] steward failed: {err}");
                self.push_steward_log(format!("Steward skipped a turn: {}", clip(&err, 120)));
            }
        }
        cx.notify();
    }

    /// Engine notices from the run (old tool results cleared, files touched).
    pub(super) fn note_context_event(&mut self, line: String, cx: &mut Context<Self>) {
        self.push_steward_log(line);
        cx.notify();
    }

    pub(super) fn note_files_touched(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let mut any = false;
        for path in paths {
            any |= matches!(
                self.ledger.add_pin(PinKind::FileTouched, &path, PinAuthor::Engine),
                PinAdd::Added(_)
            );
        }
        if any {
            self.save_current_chat();
            cx.notify();
        }
    }

    // ----- settings --------------------------------------------------------

    /// Per-chat override: follow the setting, force on, force off.
    pub(super) fn cycle_chat_steward(&mut self, cx: &mut Context<Self>) {
        self.chat_steward_override = match self.chat_steward_override {
            None => Some(true),
            Some(true) => Some(false),
            Some(false) => None,
        };
        if !self.steward_enabled() {
            self.reset_steward();
        }
        self.save_current_chat();
        cx.notify();
    }

    pub(super) fn toggle_steward_default(&mut self, cx: &mut Context<Self>) {
        self.steward_settings.enabled = !self.steward_settings.enabled;
        self.save_steward_settings();
        if !self.steward_enabled() {
            self.reset_steward();
        }
        cx.notify();
    }

    pub(super) fn render_steward_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chat_label = match self.chat_steward_override {
            None => format!(
                "AI steward: {} (default)",
                if self.steward_settings.enabled { "on" } else { "off" }
            ),
            Some(true) => "AI steward: on (this chat)".to_string(),
            Some(false) => "AI steward: off (this chat)".to_string(),
        };
        h_flex()
            .gap_1()
            .child(
                Button::new("agent-chat-steward-chat")
                    .xsmall()
                    .ghost()
                    .label(chat_label)
                    .tooltip(
                        "Use AI to keep long chats on track in this chat: extracts requirements \
                         for you to approve and flags plans that break them. Click to cycle: \
                         default, on, off.",
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.cycle_chat_steward(cx))),
            )
            .child(
                Button::new("agent-chat-steward-default")
                    .xsmall()
                    .ghost()
                    .label(if self.steward_settings.enabled { "Default: on" } else { "Default: off" })
                    .tooltip("Project default for \"Use AI to keep long chats on track\" (off by default).")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_steward_default(cx))),
            )
    }

    // ----- pin actions -----------------------------------------------------

    /// "Pin as requirement" on a message.
    pub(super) fn pin_message(&mut self, message_index: usize, kind: PinKind, cx: &mut Context<Self>) {
        let text = match self.ledger.entries.get(message_index).map(|e| &e.kind) {
            Some(agent_runtime::EntryKind::UserTurn { text })
            | Some(agent_runtime::EntryKind::AssistantTurn { text }) => text.clone(),
            _ => return,
        };
        self.add_user_pin(kind, &text, cx);
    }

    fn add_user_pin(&mut self, kind: PinKind, text: &str, cx: &mut Context<Self>) {
        match self.ledger.add_pin(kind, text, PinAuthor::User) {
            PinAdd::Added(_) => {
                self.push_steward_log(format!("Pinned {}: {}", kind.label(), clip(text, 80)));
                self.save_current_chat();
            }
            PinAdd::Duplicate(_) | PinAdd::Rejected => {}
        }
        cx.notify();
    }

    fn remove_pin(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.ledger.remove_pin(index) {
            if self.pin_edit == Some(PinEditTarget::Pin(index)) {
                self.pin_edit = None;
            }
            self.save_current_chat();
        }
        cx.notify();
    }

    fn accept_pin_change(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.pending_pin_changes.len() {
            return;
        }
        let change = self.pending_pin_changes.remove(index);
        if apply_change(&mut self.ledger, &change, Acceptance::User) {
            self.push_steward_log(format!("Accepted: {}", clip(&change.summary(), 100)));
            self.save_current_chat();
        }
        self.pin_edit = None;
        cx.notify();
    }

    fn reject_pin_change(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.pending_pin_changes.len() {
            let change = self.pending_pin_changes.remove(index);
            self.push_steward_log(format!("Rejected: {}", clip(&change.summary(), 100)));
        }
        self.pin_edit = None;
        cx.notify();
    }

    fn begin_pin_edit(
        &mut self,
        target: PinEditTarget,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pin_edit = Some(target);
        self.pin_edit_input.update(cx, |input, cx| input.set_value(&text, window, cx));
        cx.notify();
    }

    fn commit_pin_edit(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.pin_edit.take() else {
            return;
        };
        let new_text = self.pin_edit_input.read(cx).text().to_string();
        match target {
            PinEditTarget::Pin(index) => {
                if self.ledger.edit_pin(index, &new_text) {
                    self.save_current_chat();
                }
            }
            PinEditTarget::Proposal(index) => {
                if let Some(change) = self.pending_pin_changes.get(index).cloned() {
                    let edited = PinChange {
                        op: match change.op {
                            PinOp::Add { .. } => PinOp::Add { text: new_text },
                            PinOp::Replace { old, .. } => PinOp::Replace { old, new: new_text },
                            remove @ PinOp::Remove { .. } => remove,
                        },
                        // The user wrote the final words.
                        provenance: Provenance::Trusted,
                        ..change
                    };
                    self.pending_pin_changes.remove(index);
                    if apply_change(&mut self.ledger, &edited, Acceptance::User) {
                        self.save_current_chat();
                    }
                }
            }
        }
        cx.notify();
    }

    /// Requirement-like sentences in the latest user message that are not
    /// pinned yet.
    fn current_pin_candidates(&self) -> Vec<agent_runtime::PinCandidate> {
        let Some(text) = self.display_items.iter().rev().find_map(|item| match item {
            DisplayItem::UserMessage { content, .. } => Some(content.as_str()),
            _ => None,
        }) else {
            return Vec::new();
        };
        detect_pin_candidates(text, 3)
            .into_iter()
            .filter(|candidate| {
                self.ledger.find_pin(candidate.kind, &candidate.text).is_none()
                    && !self.dismissed_pin_candidates.contains(&candidate.text)
            })
            .collect()
    }

    // ----- rendering -------------------------------------------------------

    /// The "Pinned context" strip: pins (editable), the steward's proposals,
    /// one-click candidates, the drift chip and the audit trail. Renders
    /// nothing while there is nothing to show.
    pub(super) fn render_pinned_strip(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let pins = self.ledger.pins();
        let candidates = self.current_pin_candidates();
        if pins.is_empty()
            && candidates.is_empty()
            && self.pending_pin_changes.is_empty()
            && self.drift_warning.is_none()
            && self.steward_log.is_empty()
        {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        let warn = cx.theme().warning;
        let border = cx.theme().border;
        let editing = self.pin_edit;

        let edit_row = |cx: &mut Context<Self>| {
            h_flex()
                .w_full()
                .gap_1()
                .items_center()
                .child(TextInput::new(&self.pin_edit_input).flex_1().min_w_0())
                .child(
                    Button::new("pin-edit-save")
                        .xsmall()
                        .primary()
                        .label("Save")
                        .on_click(cx.listener(|this, _, _, cx| this.commit_pin_edit(cx))),
                )
                .child(
                    Button::new("pin-edit-cancel")
                        .xsmall()
                        .ghost()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pin_edit = None;
                            cx.notify();
                        })),
                )
        };

        let mut strip = v_flex()
            .id("agent-chat-pinned-strip")
            .w_full()
            .max_h(px(180.0))
            .overflow_y_scroll()
            .gap_1()
            .p_2()
            .mb_2()
            .rounded(px(6.0))
            .border_1()
            .border_color(border)
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(format!("Pinned context ({})", pins.len())),
            );

        for pin in pins {
            let index = pin.index;
            let text = pin.text.clone();
            if editing == Some(PinEditTarget::Pin(index)) {
                strip = strip.child(edit_row(cx));
                continue;
            }
            strip = strip.child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .w(px(84.0))
                            .flex_shrink_0()
                            .child(pin.kind.title()),
                    )
                    .child(div().flex_1().min_w_0().text_xs().child(pin.text.clone()))
                    .child(
                        Button::new(SharedString::from(format!("pin-edit-{index}")))
                            .xsmall()
                            .ghost()
                            .icon(IconName::EditPencil)
                            .tooltip("Edit this pin")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.begin_pin_edit(PinEditTarget::Pin(index), text.clone(), window, cx);
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("pin-del-{index}")))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Unpin")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_pin(index, cx))),
                    ),
            );
        }

        for (i, change) in self.pending_pin_changes.iter().enumerate() {
            if editing == Some(PinEditTarget::Proposal(i)) {
                strip = strip.child(edit_row(cx));
                continue;
            }
            let untrusted = change.provenance == Provenance::Untrusted;
            let edit_text = match &change.op {
                PinOp::Add { text } | PinOp::Remove { text } => text.clone(),
                PinOp::Replace { new, .. } => new.clone(),
            };
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
                            .child(format!("Steward suggests {}", change.summary())),
                    )
                    .child(
                        Button::new(SharedString::from(format!("chg-ok-{i}")))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Check)
                            .tooltip(if untrusted {
                                "Accept (this came from tool, web or sub-agent content)"
                            } else {
                                "Accept"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.accept_pin_change(i, cx))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("chg-edit-{i}")))
                            .xsmall()
                            .ghost()
                            .icon(IconName::EditPencil)
                            .tooltip("Edit, then accept")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.begin_pin_edit(PinEditTarget::Proposal(i), edit_text.clone(), window, cx);
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("chg-no-{i}")))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Reject")
                            .on_click(cx.listener(move |this, _, _, cx| this.reject_pin_change(i, cx))),
                    ),
            );
        }

        for (i, candidate) in candidates.into_iter().enumerate() {
            let kind = candidate.kind;
            let pin_text = candidate.text.clone();
            let dismiss_text = candidate.text.clone();
            strip = strip.child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new(SharedString::from(format!("cand-pin-{i}")))
                            .xsmall()
                            .label(format!("Pin as {}: {}", kind.label(), clip(&candidate.text, 70)))
                            .tooltip("Keep this in the pinned context so it survives long chats")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.add_user_pin(kind, &pin_text, cx);
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("cand-no-{i}")))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Not a requirement")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.dismissed_pin_candidates.push(dismiss_text.clone());
                                cx.notify();
                            })),
                    ),
            );
        }

        if let Some(DriftWarning { requirement, why }) = self.drift_warning.clone() {
            strip = strip.child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .child(Icon::new(IconName::TriangleAlert).size_3().text_color(warn))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(warn)
                            .child(format!("The last plan may break \"{}\": {}", clip(&requirement, 80), clip(&why, 120))),
                    )
                    .child(
                        Button::new("drift-dismiss")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Dismiss")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.drift_warning = None;
                                cx.notify();
                            })),
                    ),
            );
        }

        if !self.steward_log.is_empty() {
            strip = strip.child(
                v_flex().w_full().children(
                    self.steward_log
                        .iter()
                        .map(|line| div().text_xs().text_color(muted.opacity(0.8)).child(line.clone())),
                ),
            );
        }

        Some(strip)
    }
}
