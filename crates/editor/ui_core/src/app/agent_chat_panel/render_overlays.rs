//! Overlays drawn over the agent chat panel. Today that is the provider
//! configuration card: [`AgentChatPanel::render_config_overlay`] composes it
//! from [`render_config_heading`](AgentChatPanel::render_config_heading) and
//! [`render_config_actions`](AgentChatPanel::render_config_actions), and
//! [`submit_config_field`](AgentChatPanel::submit_config_field) holds the
//! "Save" step logic so the click closure stays a one-liner.

use gpui::{prelude::FluentBuilder as _, *};
use ui::{
    button::{Button, ButtonVariants as _},
    dropdown::SearchableList,
    h_flex,
    input::TextInput,
    popover::Popover,
    v_flex, ActiveTheme as _, Disableable, Icon, IconName, Sizable, Size, StyledExt,
};

use super::panel::AgentChatPanel;
use super::types::*;

impl AgentChatPanel {
    /// The provider configuration card, or `None` when no provider is being
    /// configured. Turns red while the last attempt's error is shown.
    pub(crate) fn render_config_overlay(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let configuring = self.configuring_provider.clone()?;
        let has_error = self.config_error.is_some();
        let err_text = self.config_error.clone().unwrap_or_default();

        Some(
            v_flex()
                .w_full()
                .gap_2()
                .p_3()
                .rounded(px(8.0))
                .bg(if has_error {
                    cx.theme().colors.danger.opacity(0.08)
                } else {
                    cx.theme().colors.background.opacity(0.5)
                })
                .border_1()
                .border_color(if has_error {
                    cx.theme().colors.danger.opacity(0.35)
                } else {
                    cx.theme().colors.border.opacity(0.5)
                })
                .child(self.render_config_heading(&configuring, cx))
                .child(
                    TextInput::new(&self.custom_provider_input)
                        .w_full()
                        .xsmall(),
                )
                .when(has_error, |el| {
                    el.child(
                        div()
                            .w_full()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(err_text),
                    )
                })
                .child(self.render_config_actions(has_error, cx)),
        )
    }

    /// Provider name and the label/description of the field being asked for.
    fn render_config_heading(&self, configuring: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let entry = self.provider_entries.get(configuring);
        let fields = self.config_fields_for(configuring);
        let field = fields.get(self.configuring_field_index);

        v_flex()
            .w_full()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(match (entry, field) {
                        (Some(e), Some(f)) => {
                            format!("{} \u{2014} {}", e.display_name, f.label)
                        }
                        (Some(e), None) => e.display_name.to_string(),
                        (None, _) => configuring.to_string(),
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(match field {
                        Some(f) => f.description,
                        None => "",
                    }),
            )
    }

    /// The Cancel and Save/Retry buttons.
    fn render_config_actions(&self, has_error: bool, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .justify_end()
            .child(
                Button::new("provider-config-cancel")
                    .xsmall()
                    .ghost()
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.configuring_provider = None;
                        this.config_error = None;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("provider-config-submit")
                    .xsmall()
                    .primary()
                    .label(if has_error { "Retry" } else { "Save" })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.submit_config_field(window, cx);
                    })),
            )
    }

    /// Store the typed value for the current config field and advance; after
    /// the last field, apply the provider config and either close the card
    /// (fetching models) or restart from the first field with the error shown.
    fn submit_config_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.custom_provider_input.read(cx).text().to_string();
        let pid = self.configuring_provider.clone();
        if let Some(ref id) = pid {
            let fields = self.config_fields_for(id);
            let field_key = fields
                .get(self.configuring_field_index)
                .map(|f| f.key)
                .unwrap_or("value")
                .to_string();
            self.config_values.insert(field_key, value);
            self.configuring_field_index += 1;
            if self.configuring_field_index >= fields.len() {
                let values = self.config_values.drain().collect();
                match self.apply_provider_config(id, values, cx) {
                    Ok(()) => {
                        self.configuring_provider = None;
                        self.config_error = None;
                        if self.active_provider_ix < self.provider_catalog.len() {
                            self.fetch_models_in_background(self.active_provider_ix, cx);
                        }
                    }
                    Err(e) => {
                        self.config_error = Some(e.to_string());
                        self.configuring_field_index = 0;
                        self.catalog_for_current_provider(cx);
                    }
                }
            }
            self.custom_provider_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
            cx.notify();
        }
    }
}
