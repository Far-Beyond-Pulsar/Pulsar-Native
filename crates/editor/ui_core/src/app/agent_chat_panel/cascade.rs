//! Panel side of cascade routing and best-of-N: the project setting
//! (`.pulsar/agent_cascade.json`), the "escalated" header badge and the notices.
//!
//! The runtime decides when to escalate (`agent_runtime::cascade`); this file
//! only builds the policy for a run and shows what happened.

use super::*;
use agent_runtime::{BestOfReport, CascadeSettings, Escalation};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, ActiveTheme as _, Sizable as _,
};

impl AgentChatPanel {
    pub(super) fn load_cascade_settings() -> CascadeSettings {
        engine_state::get_project_path()
            .map(|root| CascadeSettings::load(&PathBuf::from(root)))
            .unwrap_or_default()
    }

    fn save_cascade_settings(&self) {
        if let Some(root) = engine_state::get_project_path() {
            if let Err(err) = self.cascade_settings.save(&PathBuf::from(root)) {
                tracing::warn!("[agent_chat] could not save agent_cascade.json: {err}");
            }
        }
    }

    /// The policy for the next run: `None` while the setting is off or no
    /// escalation model is configured. Resolves the escalation model's profile
    /// on the active provider.
    pub(super) fn cascade_policy(&self) -> Option<agent_runtime::CascadePolicy> {
        let policy = self.cascade_settings.policy()?;
        let profile = Self::profile_for_model(self.active_provider(), &policy.escalation_model);
        Some(policy.with_profile(profile))
    }

    pub(super) fn note_escalation(&mut self, escalation: Escalation, cx: &mut Context<Self>) {
        self.cascade_badge = Some(format!("Escalated to {}", escalation.to_model));
        cx.notify();
    }

    pub(super) fn note_best_of(&mut self, report: BestOfReport, cx: &mut Context<Self>) {
        // The notice line already went through `ContextNotice`. When a later
        // candidate won after the first one streamed, its text replaces the
        // streamed bubble.
        if let (Some(text), Some(dix)) = (report.replacement, self.streaming_display_item_ix) {
            if let Some(DisplayItem::AssistantMessage { content, .. }) =
                self.display_items.get_mut(dix)
            {
                *content = text;
                self.display_item_heights.remove(&dix);
            }
        }
        cx.notify();
    }

    pub(super) fn toggle_cascade(&mut self, cx: &mut Context<Self>) {
        self.cascade_settings.enabled = !self.cascade_settings.enabled;
        self.save_cascade_settings();
        if !self.cascade_settings.enabled {
            self.cascade_badge = None;
        }
        cx.notify();
    }

    /// Header badge: shown after the run escalated, until the next run starts.
    pub(super) fn render_cascade_badge(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let label = self.cascade_badge.clone()?;
        Some(
            h_flex()
                .px_2()
                .rounded_full()
                .bg(cx.theme().warning.opacity(0.18))
                .text_color(cx.theme().warning)
                .text_xs()
                .child(label),
        )
    }

    pub(super) fn render_cascade_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = match (&self.cascade_settings.enabled, &self.cascade_settings.escalation_model) {
            (true, Some(model)) => format!("Cascade: on ({model})"),
            (true, None) => "Cascade: no model set".to_string(),
            (false, _) => "Cascade: off".to_string(),
        };
        Button::new("agent-chat-cascade")
            .xsmall()
            .ghost()
            .label(label)
            .tooltip(
                "Cascade routing: a larger escalation model takes over when the main model \
                 fails tool calls, loops, is unsure or overflows its context. Set \
                 escalation_model in .pulsar/agent_cascade.json. Off by default.",
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_cascade(cx)))
    }
}
