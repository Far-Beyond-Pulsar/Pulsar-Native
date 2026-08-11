use gpui::{prelude::FluentBuilder as _, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Disableable, Icon, IconName, Sizable, Size, StyledExt,
    input::TextInput,
    popover::Popover,
    dropdown::SearchableList,
};

use super::panel::AgentChatPanel;
use super::types::*;

impl AgentChatPanel {
    pub(crate) fn render_config_overlay(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let configuring = self.configuring_provider.clone()?;
        let entry = self.provider_entries.get(&configuring);
        let fields = entry.map(|e| &e.config_fields[..]).unwrap_or(&[]);
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
                                    (Some(e), Some(f)) => format!("{} \u{2014} {}", e.display_name, f.label),
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
                .child(TextInput::new(&self.custom_provider_input).w_full().xsmall())
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
                                    let value = this.custom_provider_input.read(cx).text().to_string();
                                    let pid = this.configuring_provider.clone();
                                    if let Some(ref id) = pid {
                                        let entry = this.provider_entries.get(id);
                                        let fields_from_entry = entry.map(|e| e.config_fields.clone()).unwrap_or_default();

                                        let field_key = fields_from_entry.get(this.configuring_field_index).map(|f| f.key).unwrap_or("value").to_string();
                                        this.config_values.insert(field_key, value);
                                        this.configuring_field_index += 1;
                                        if this.configuring_field_index >= fields_from_entry.len() {
                                            let config = agent_chat_core::ProviderConfig {
                                                values: this.config_values.drain().collect(),
                                            };
                                            let mut validated = false;
                                            for c in &this.crate_instances {
                                                if let Ok(p) = c.create(id, config.clone()) {
                                                    match p.validate_config() {
                                                        Ok(()) => {
                                                            this.provider_registry.register(std::sync::Arc::from(p));
                                                            this.provider_states.insert(id.clone(), ProviderState::Ready);
                                                            this.provider_states_shared.borrow_mut().insert(id.clone(), ProviderState::Ready);
                                                            this.provider_entries.remove(id);
                                                            this.configuring_provider = None;
                                                            this.config_error = None;
                                                            this.refresh_provider_catalog(cx);
                                                            if this.active_provider_ix < this.provider_catalog.len() {
                                                                this.fetch_models_in_background(this.active_provider_ix, cx);
                                                            }
                                                            validated = true;
                                                        }
                                                        Err(e) => {
                                                            this.config_error = Some(e.to_string());
                                                            this.configuring_field_index = 0;
                                                            this.custom_provider_input.update(cx, |input, cx| {
                                                                input.set_value("", window, cx);
                                                            });
                                                        }
                                                    }
                                                    break;
                                                }
                                            }
                                            if !validated {
                                                this.catalog_for_current_provider(cx);
                                            }
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

    pub(crate) fn render_custom_provider_wizard(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let add_provider_prompt = self
            .pending_custom_provider_step
            .map(|s| Self::add_provider_prompt_title(s).to_string())?;

        Some(
            v_flex()
                .w_full()
                .gap_1()
                .p_2()
                .rounded(px(6.0))
                .bg(cx.theme().primary.opacity(0.08))
                .border_1()
                .border_color(cx.theme().primary.opacity(0.25))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().primary)
                        .child(add_provider_prompt),
                )
                .child(
                    TextInput::new(&self.custom_provider_input)
                        .w_full()
                        .xsmall(),
                )
                .child(
                    h_flex()
                        .w_full()
                        .gap_1()
                        .child(
                            Button::new("agent-chat-add-provider-cancel")
                                .xsmall()
                                .ghost()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.cancel_add_provider_prompt(cx);
                                })),
                        )
                        .child(
                            Button::new("agent-chat-add-provider-next")
                                .xsmall()
                                .primary()
                                .label("Save Provider")
                                .disabled(
                                    self.custom_provider_input
                                        .read(cx)
                                        .text()
                                        .to_string()
                                        .trim()
                                        .is_empty(),
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let step = this.pending_custom_provider_step;
                                    if let Some(s) = step {
                                        let value = this.custom_provider_input.read(cx).text().to_string();
                                        match s {
                                            AddProviderPromptStep::ProviderLabel => {
                                                if let Some(ref mut p) = this.pending_custom_provider {
                                                    p.label = value;
                                                }
                                                this.pending_custom_provider_step = Some(AddProviderPromptStep::Endpoint);
                                                this.custom_provider_input.update(cx, |input, cx| {
                                                    input.set_value("", window, cx);
                                                });
                                            }
                                            AddProviderPromptStep::Endpoint => {
                                                if let Some(ref mut p) = this.pending_custom_provider {
                                                    p.endpoint = value;
                                                }
                                                this.submit_custom_provider(window, cx);
                                            }
                                            _ => {}
                                        }
                                    }
                                    cx.notify();
                                })),
                        ),
                ),
        )
    }
}
