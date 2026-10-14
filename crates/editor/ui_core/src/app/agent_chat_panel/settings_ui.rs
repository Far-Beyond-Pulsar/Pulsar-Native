//! The "Agent settings" strip: pickers for the steward and cascade escalation
//! models, and an editor for the active model's profile override.
//!
//! The values live in the same files as before (`agent_steward.json`,
//! `agent_cascade.json`, `agent_profiles.json`); this is only the UI for them.
//! Like the other agent controls, each picker is a button that cycles through
//! the choices.

use super::*;
use agent_runtime::{ProfileOverride, ProfileOverrides, ToolCalling};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Sizable as _,
};

/// Context sizes the override button steps through.
const CONTEXT_STEPS: [u32; 7] = [4_096, 8_192, 16_384, 32_768, 65_536, 131_072, 200_000];

fn context_label(tokens: u32) -> String {
    if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// The choice after `current` in `[None, options[0], options[1], ...]`, wrapping
/// back to `None` after the last one. A `current` that is no longer an option
/// (the model was removed) restarts the cycle.
fn next_choice(options: &[String], current: Option<&str>) -> Option<String> {
    match current.and_then(|c| options.iter().position(|o| o == c)) {
        None if current.is_none() => options.first().cloned(),
        None => None,
        Some(i) => options.get(i + 1).cloned(),
    }
}

fn next_context(current: Option<u32>) -> Option<u32> {
    match current {
        None => Some(CONTEXT_STEPS[0]),
        Some(c) => CONTEXT_STEPS
            .iter()
            .position(|&s| s == c)
            .and_then(|i| CONTEXT_STEPS.get(i + 1).copied()),
    }
}

fn next_tool_calling(current: Option<ToolCalling>) -> Option<ToolCalling> {
    match current {
        None => Some(ToolCalling::Native),
        Some(ToolCalling::Native) => Some(ToolCalling::Weak),
        Some(ToolCalling::Weak) => Some(ToolCalling::None),
        Some(ToolCalling::None) => None,
    }
}

fn next_vision(current: Option<bool>) -> Option<bool> {
    match current {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}

fn tool_calling_label(value: ToolCalling) -> &'static str {
    match value {
        ToolCalling::Native => "native",
        ToolCalling::Weak => "weak",
        ToolCalling::None => "none (text protocol)",
    }
}

impl AgentChatPanel {
    /// Ids of the active provider's models, in list order.
    fn active_model_ids(&self) -> Vec<String> {
        self.active_provider()
            .map(|p| p.models.iter().map(|m| m.id.to_string()).collect())
            .unwrap_or_default()
    }

    fn cycle_steward_model(&mut self, cx: &mut Context<Self>) {
        let options = self.active_model_ids();
        self.steward_settings.model = next_choice(&options, self.steward_settings.model.as_deref());
        self.save_steward_settings();
        // The steward is rebuilt when its model changes (see `ensure_steward`).
        cx.notify();
    }

    fn cycle_cascade_model(&mut self, cx: &mut Context<Self>) {
        let options = self.active_model_ids();
        self.cascade_settings.escalation_model =
            next_choice(&options, self.cascade_settings.escalation_model.as_deref());
        self.save_cascade_settings();
        cx.notify();
    }

    /// The saved override of the active model (empty when there is none).
    fn active_override(&self) -> ProfileOverride {
        let (Some(path), Some(provider), Some(model)) = (
            Self::profile_overrides_path(),
            self.active_provider(),
            self.active_model(),
        ) else {
            return ProfileOverride::default();
        };
        ProfileOverrides::load(&path)
            .get(provider.id, model.id)
            .cloned()
            .unwrap_or_default()
    }

    /// Change the active model's override and write `agent_profiles.json`.
    /// An override that ends up empty removes the entry.
    fn edit_profile_override(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut ProfileOverride),
    ) {
        let (Some(path), Some(provider_id), Some(model_id)) = (
            Self::profile_overrides_path(),
            self.active_provider().map(|p| p.id),
            self.active_model().map(|m| m.id),
        ) else {
            return;
        };
        let mut store = ProfileOverrides::load(&path);
        let mut value = store.get(provider_id, model_id).cloned().unwrap_or_default();
        edit(&mut value);
        store.set(provider_id, model_id, value);
        if let Err(err) = store.save(&path) {
            tracing::warn!("[agent_chat] could not save agent_profiles.json: {err}");
        }
        cx.notify();
    }

    pub(super) fn render_agent_settings_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("agent-chat-settings-toggle")
            .xsmall()
            .ghost()
            .label("Agent settings")
            .tooltip(
                "Steward and cascade models, and the active model's context size, tool \
                 calling and vision overrides. Click to show or hide.",
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.agent_settings_open = !this.agent_settings_open;
                cx.notify();
            }))
    }

    pub(super) fn render_agent_settings(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.agent_settings_open {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        let profile = self.active_profile();
        let saved = self.active_override();
        let model_name = self
            .active_model()
            .map(|m| m.label.to_string())
            .unwrap_or_else(|| "no model selected".to_string());

        let steward_label = match &self.steward_settings.model {
            Some(model) => format!("Steward model: {model}"),
            None => "Steward model: the model's compact model".to_string(),
        };
        let cascade_label = match &self.cascade_settings.escalation_model {
            Some(model) => format!("Escalation model: {model}"),
            None => "Escalation model: none (cascade stays idle)".to_string(),
        };
        let context_label_text = match saved.context_tokens {
            Some(tokens) => format!("Context: {} (override)", context_label(tokens)),
            None => format!("Context: {} (auto)", context_label(profile.context_tokens)),
        };
        let tools_label = match saved.tool_calling {
            Some(value) => format!("Tool calling: {} (override)", tool_calling_label(value)),
            None => format!("Tool calling: {} (auto)", tool_calling_label(profile.tool_calling)),
        };
        let vision_label = match saved.vision {
            Some(true) => "Vision: on (override)".to_string(),
            Some(false) => "Vision: off (override)".to_string(),
            None => format!("Vision: {} (auto)", if profile.vision { "on" } else { "off" }),
        };

        let row = |id: &'static str, label: String, tip: &'static str| {
            Button::new(id).xsmall().ghost().label(label).tooltip(tip)
        };

        Some(
            v_flex()
                .w_full()
                .gap_1()
                .pb_2()
                .child(
                    h_flex()
                        .w_full()
                        .gap_1()
                        .child(
                            row(
                                "agent-chat-steward-model",
                                steward_label,
                                "Model the AI steward uses. Click to cycle through the active \
                                 provider's models; after the last one it goes back to the \
                                 model's compact model.",
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.cycle_steward_model(cx))),
                        )
                        .child(
                            row(
                                "agent-chat-cascade-model",
                                cascade_label,
                                "Larger model that cascade routing escalates to. Click to \
                                 cycle through the active provider's models.",
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.cycle_cascade_model(cx))),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("Profile overrides for {model_name}")),
                )
                .child(
                    h_flex()
                        .w_full()
                        .gap_1()
                        .child(
                            row(
                                "agent-chat-profile-context",
                                context_label_text,
                                "Context window in tokens. Click to step through common sizes; \
                                 after the last it goes back to auto.",
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.edit_profile_override(cx, |o| {
                                    o.context_tokens = next_context(o.context_tokens)
                                })
                            })),
                        )
                        .child(
                            row(
                                "agent-chat-profile-tools",
                                tools_label,
                                "How reliably the model calls tools. Weak models get a smaller \
                                 tool set; none uses the text protocol. Click to cycle.",
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.edit_profile_override(cx, |o| {
                                    o.tool_calling = next_tool_calling(o.tool_calling)
                                })
                            })),
                        )
                        .child(
                            row(
                                "agent-chat-profile-vision",
                                vision_label,
                                "Whether screenshots are sent to this model. Click to cycle: \
                                 auto, on, off.",
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.edit_profile_override(cx, |o| {
                                    o.vision = next_vision(o.vision)
                                })
                            })),
                        )
                        .child(
                            row(
                                "agent-chat-profile-reset",
                                "Reset".to_string(),
                                "Remove every override for this model.",
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.edit_profile_override(cx, |o| *o = ProfileOverride::default())
                            })),
                        ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[core::prelude::v1::test]
    fn model_choice_cycles_through_models_and_back_to_default() {
        let models = ids(&["small", "large"]);
        let first = next_choice(&models, None);
        assert_eq!(first.as_deref(), Some("small"));
        let second = next_choice(&models, first.as_deref());
        assert_eq!(second.as_deref(), Some("large"));
        assert_eq!(next_choice(&models, second.as_deref()), None);
    }

    #[core::prelude::v1::test]
    fn model_choice_from_a_removed_model_returns_to_default() {
        let models = ids(&["small"]);
        assert_eq!(next_choice(&models, Some("gone")), None);
        assert_eq!(next_choice(&[], None), None);
    }

    #[core::prelude::v1::test]
    fn context_steps_end_at_auto() {
        let mut value = None;
        let mut seen = Vec::new();
        for _ in 0..CONTEXT_STEPS.len() {
            value = next_context(value);
            seen.push(value.unwrap());
        }
        assert_eq!(seen, CONTEXT_STEPS.to_vec());
        assert_eq!(next_context(value), None);
        // A hand-edited value that is not a step falls back to auto.
        assert_eq!(next_context(Some(12_345)), None);
    }

    #[core::prelude::v1::test]
    fn tool_calling_and_vision_cycle_back_to_auto() {
        let mut tools = None;
        for _ in 0..3 {
            tools = next_tool_calling(tools);
            assert!(tools.is_some());
        }
        assert_eq!(next_tool_calling(tools), None);

        let mut vision = None;
        for _ in 0..2 {
            vision = next_vision(vision);
            assert!(vision.is_some());
        }
        assert_eq!(next_vision(vision), None);
    }
}
