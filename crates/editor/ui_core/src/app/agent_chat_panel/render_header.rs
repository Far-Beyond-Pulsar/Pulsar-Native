use gpui::{prelude::FluentBuilder as _, Corner, *};
use ui::{
    button::{Button, ButtonVariants as _},
    dropdown::{SearchableList, SearchableListEvent},
    h_flex, v_flex, ActiveTheme as _, Disableable, Icon, IconName, Sizable, Size, StyledExt,
    popover::Popover,
};

use super::panel::AgentChatPanel;
use super::types::*;
use super::chat_storage;

impl AgentChatPanel {
    pub(crate) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let provider_label = self.active_provider()
            .map(|p| p.label.to_string())
            .unwrap_or_else(|| "Provider".to_string());
        let model_label = self.active_model()
            .map(|m| m.label.to_string())
            .unwrap_or_else(|| "Model".to_string());

        let provider_list = self.provider_list.clone();
        let model_list = self.model_list.clone();

        let provider_popover = Popover::<SearchableList<ProviderDefinition>>::new(
            "agent-chat-provider-popover",
        )
        .anchor(Corner::TopLeft)
        .trigger(
            Button::new("agent-chat-provider-trigger")
                .small()
                .ghost()
                .justify_start()
                .tooltip("Select provider")
                .label(provider_label)
                .dropdown_caret(true),
        )
        .content(move |_window, _cx| provider_list.clone());

        let model_popover = Popover::<SearchableList<ModelDefinition>>::new(
            "agent-chat-model-popover",
        )
        .anchor(Corner::TopLeft)
        .trigger(
            Button::new("agent-chat-model-trigger")
                .small()
                .ghost()
                .justify_start()
                .tooltip("Select model")
                .label(model_label)
                .dropdown_caret(true),
        )
        .content(move |_window, _cx| model_list.clone());

        let chat_history_list = self.chat_history_list.clone();
        let current_title = Self::inferred_chat_title(&self.messages);
        let chat_label = if current_title.len() > 40 {
            format!("{}...", &current_title[..37])
        } else if current_title.is_empty() {
            "Chat History".to_string()
        } else {
            current_title
        };
        let chat_history_popover =
            Popover::<SearchableList<ChatHistoryEntry>>::new("agent-chat-history-popover")
                .anchor(Corner::BottomLeft)
                .trigger(
                    Button::new("agent-chat-history-trigger")
                        .small()
                        .ghost()
                        .justify_start()
                        .tooltip("Browse and switch between conversations")
                        .label(chat_label)
                        .dropdown_caret(true),
                )
                .content(move |_window, _cx| chat_history_list.clone());

        v_flex()
            .w_full()
            .gap(px(3.0))
            .px_3()
            .py(px(6.0))
            .child(
                // ── Row 1: Chat management (primary, labeled, prominent) ──
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(chat_history_popover)
                    .child(
                        Button::new("agent-chat-new-chat")
                            .small()
                            .ghost()
                            .icon(IconName::Plus)
                            .label("New Chat")
                            .tooltip("Start a fresh conversation")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_new_chat(cx);
                            })),
                    )
                    .flex_1()
                    .child(
                        Button::new("agent-chat-import")
                            .small()
                            .ghost()
                            .icon(IconName::Upload)
                            .label("Import")
                            .tooltip("Load a conversation from a JSON file")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.import_chat(cx);
                            })),
                    )
                    .child(
                        Button::new("agent-chat-export")
                            .small()
                            .ghost()
                            .icon(IconName::Download)
                            .label("Export")
                            .tooltip("Save this conversation as JSON")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export_current_chat();
                            })),
                    ),
            )
            .child(
                // ── Row 2: Provider / Model (compact, technical) ──
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_1()
                    .child(provider_popover)
                    .child(
                        div()
                            .text_color(cx.theme().border)
                            .text_sm()
                            .child("/"),
                    )
                    .child(model_popover)
                    .child(
                        Button::new("agent-chat-refresh-models")
                            .icon(IconName::Refresh)
                            .xsmall()
                            .ghost()
                            .tooltip("Refresh model list from provider")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh_models_for_active_provider(cx);
                            })),
                    )
                    .child(
                        Button::new("agent-chat-add-provider")
                            .icon(IconName::Plus)
                            .xsmall()
                            .ghost()
                            .tooltip("Add a custom provider endpoint")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_add_provider_prompt(window, cx);
                            })),
                    ),
            )
            .child(
                // ── Row 3: Context meter ──
                self.render_context_meter(cx),
            )
    }

    fn render_context_meter(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let total_chars = self.active_context_chars();
        let sliver_chars = Self::COMPACTION_SUMMARY_CHAR_BUDGET;
        let usable_chars = total_chars.saturating_sub(sliver_chars);
        let used: usize = self.messages.iter().map(|m| m.content.len()).sum();
        let fill_pct = (used as f32 / usable_chars.max(1) as f32).min(1.0);
        let sliver_pct = sliver_chars as f32 / total_chars.max(1) as f32;

        let bar_color = if fill_pct > 0.85 {
            cx.theme().danger
        } else if fill_pct > 0.6 {
            cx.theme().warning
        } else {
            cx.theme().success
        };

        let model_ctx = self.active_model()
            .and_then(|m| {
                if m.context_tokens > 0 {
                    Some(m.context_tokens)
                } else {
                    Self::infer_context_tokens(m.id).map(|t| t as u32)
                }
            })
            .unwrap_or(6_000);

        let ctx_label = if model_ctx >= 1_000_000 {
            format!("{}M ctx", model_ctx / 1_000_000)
        } else if model_ctx >= 1_000 {
            format!("{}k ctx", model_ctx / 1_000)
        } else {
            format!("{} ctx", model_ctx)
        };

        h_flex()
            .w_full()
            .items_center()
            .gap_1()
            .px(px(2.0))
            .child(
                div()
                    .flex_1()
                    .h(px(6.0))
                    .rounded_full()
                    .bg(cx.theme().border.opacity(0.3))
                    .relative()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .rounded_full()
                            .bg(bar_color)
                            .w(relative(fill_pct * (1.0 - sliver_pct))),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .h_full()
                            .right_0()
                            .rounded_r_full()
                            .bg(cx.theme().muted_foreground.opacity(0.25))
                            .w(relative(sliver_pct)),
                    ),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground.opacity(0.7))
                    .text_xs()
                    .child(format!("{}% · {}", (fill_pct * 100.0) as u32, ctx_label)),
            )
    }
}
