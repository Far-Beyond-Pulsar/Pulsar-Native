use gpui::{prelude::FluentBuilder as _, *};
use ui::{
    ActiveTheme as _, Disableable, Icon, IconName, Sizable, Size, StyledExt,
    button::{Button, ButtonVariants as _},
    dropdown::SearchableList,
    h_flex,
    input::TextInput,
    popover::Popover,
    v_flex,
};

use super::panel::AgentChatPanel;
use super::types::*;

impl AgentChatPanel {
    pub(crate) fn render_config_overlay(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let configuring = self.configuring_provider.clone()?;
        let entry = self.provider_entries.get(&configuring);
        let fields = self.config_fields_for(&configuring);
        let field_ix = self.configuring_field_index;
        let field = fields.get(field_ix);
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
                .child(
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
                                    (None, _) => configuring.clone(),
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
                        ),
                )
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
                .child(
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
                                    let value =
                                        this.custom_provider_input.read(cx).text().to_string();
                                    let pid = this.configuring_provider.clone();
                                    if let Some(ref id) = pid {
                                        let fields = this.config_fields_for(id);
                                        let field_key = fields
                                            .get(this.configuring_field_index)
                                            .map(|f| f.key)
                                            .unwrap_or("value")
                                            .to_string();
                                        this.config_values.insert(field_key, value);
                                        this.configuring_field_index += 1;
                                        if this.configuring_field_index >= fields.len() {
                                            let values = this.config_values.drain().collect();
                                            match this.apply_provider_config(id, values, cx) {
                                                Ok(()) => {
                                                    this.configuring_provider = None;
                                                    this.config_error = None;
                                                    if this.active_provider_ix
                                                        < this.provider_catalog.len()
                                                    {
                                                        this.fetch_models_in_background(
                                                            this.active_provider_ix,
                                                            cx,
                                                        );
                                                    }
                                                }
                                                Err(e) => {
                                                    this.config_error = Some(e.to_string());
                                                    this.configuring_field_index = 0;
                                                    this.catalog_for_current_provider(cx);
                                                }
                                            }
                                            this.custom_provider_input.update(cx, |input, cx| {
                                                input.set_value("", window, cx);
                                            });
                                        } else {
                                            this.custom_provider_input.update(cx, |input, cx| {
                                                input.set_value("", window, cx);
                                            });
                                        }
                                        cx.notify();
                                    }
                                })),
                        ),
                ),
        )
    }
}
