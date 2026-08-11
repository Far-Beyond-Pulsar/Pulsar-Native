use gpui::{prelude::FluentBuilder as _, Corner, *};
use ui::{
    button::{Button, ButtonVariants as _},
    dropdown::{SearchableList, SearchableListEvent},
    h_flex, v_flex, ActiveTheme as _, Disableable, Icon, IconName, Sizable, Size, StyledExt,
    popover::Popover,
    menu::popup_menu::PopupMenuExt,
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

        // ── Left: Provider / Model ──
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

        let provider_model_row = h_flex()
            .items_center()
            .gap_1()
            .child(provider_popover)
            .child(
                div()
                    .text_color(cx.theme().border)
                    .text_sm()
                    .child("/"),
            )
            .child(model_popover);

        // ── Right: Chat History dropdown ──
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
                .anchor(Corner::TopRight)
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

        // ── Three-dots menu ──
        let panel = cx.entity().clone();
        let has_subagents = self.pending_subagent_events.len() > 0 || self.is_processing_subagent_event;
        let queued_count = self.pending_subagent_events.len();
        let is_processing = self.is_processing_subagent_event;
        let is_in_flight = self.is_request_in_flight;

        let more_menu = Button::new("agent-chat-more-trigger")
            .small()
            .ghost()
            .icon(IconName::Ellipsis)
            .tooltip("More actions")
            .popup_menu_with_anchor(Corner::TopRight, {
                let p = panel.clone();
                move |menu, window, cx| {
                    let menu = menu
                        .menu_handler_with_icon("Import Chat", IconName::Upload, {
                            let p = p.clone();
                            move |_, cx| { p.update(cx, |this, cx| this.import_chat(cx)); }
                        })
                        .menu_handler_with_icon("Export Chat", IconName::Download, {
                            let p = p.clone();
                            move |_, cx| { p.update(cx, |this, _| this.export_current_chat()); }
                        })
                        .separator()
                        .menu_handler_with_icon("Refresh Models", IconName::Refresh, {
                            let p = p.clone();
                            move |_, cx| { p.update(cx, |this, cx| this.refresh_models_for_active_provider(cx)); }
                        })
                        .menu_handler_with_icon("Add Provider", IconName::Plus, {
                            let p = p.clone();
                            move |window, cx| { p.update(cx, |this, cx| this.start_add_provider_prompt(window, cx)); }
                        });

                    if has_subagents {
                        menu
                            .separator()
                            .menu_handler_with_icon_and_disabled(
                                format!("Process Next ({})", queued_count),
                                IconName::Play,
                                is_in_flight || is_processing,
                                {
                                    let p = p.clone();
                                    move |_, cx| { p.update(cx, |this, cx| this.process_next_subagent_completion_now(cx)); }
                                },
                            )
                            .menu_handler_with_icon("Auto Queue", IconName::Play, {
                                let p = p.clone();
                                move |_, cx| {
                                    p.update(cx, |this, cx| {
                                        this.subagent_completion_mode = super::panel::SubagentCompletionMode::Auto;
                                        this.maybe_start_next_subagent_processing(cx);
                                        cx.notify();
                                    });
                                }
                            })
                            .menu_handler_with_icon("Manual Queue", IconName::Pause, {
                                let p = p.clone();
                                move |_, cx| {
                                    p.update(cx, |this, cx| {
                                        this.subagent_completion_mode = super::panel::SubagentCompletionMode::Manual;
                                        cx.notify();
                                    });
                                }
                            })
                    } else {
                        menu
                    }
                }
            });

        v_flex()
            .w_full()
            .gap(px(2.0))
            .px_3()
            .py(px(4.0))
            // ── Row 1: Provider/Model left · Chat History + New right ──
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .child(provider_model_row)
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(chat_history_popover)
                            .child(
                                Button::new("agent-chat-new-chat")
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Plus)
                                    .tooltip("Start a new conversation")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.start_new_chat(cx);
                                    })),
                            ),
                    ),
            )
            // ── Row 2: Context meter · three-dots menu ──
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(self.render_context_meter(cx))
                    .flex_1()
                    .child(more_menu),
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
