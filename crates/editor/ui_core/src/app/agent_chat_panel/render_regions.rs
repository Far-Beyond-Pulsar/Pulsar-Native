//! Region-level render helpers for `AgentChatPanel`.
//!
//! `Render::render` (in `mod.rs`) composes the panel from three regions built
//! here: the top bar (`render_top_bar`: header + config overlay), the scrolling
//! message list (`render_messages_area`, which dispatches each row through
//! `render_display_row` to the per-variant renderers in `render_items.rs`) and
//! the prompt input area (`render_input_area`).

use super::*;

impl AgentChatPanel {
    /// Header row (provider/model pickers, actions) plus the provider config overlay.
    pub(super) fn render_top_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .px_3()
            .py(px(6.0))
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().tab_bar)
            .child(self.render_header(cx))
            .when_some(self.render_config_overlay(cx), |el, overlay| {
                el.child(overlay)
            })
    }

    /// Virtualised message list with scrollbar, viewport measurement and the
    /// jump-to-bottom button.
    pub(super) fn render_messages_area(
        &mut self,
        display_item_sizes: Rc<Vec<gpui::Size<Pixels>>>,
        display_count: usize,
        render_now_ms: u64,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("agent-chat-scroll-area")
            .relative()
            .flex_1()
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let scrolled_up = match event.delta {
                    ScrollDelta::Pixels(p) => p.y < px(0.0),
                    ScrollDelta::Lines(l) => l.y < 0.0,
                };
                if scrolled_up && this.auto_scroll {
                    this.auto_scroll = false;
                    cx.notify();
                }
            }))
            .child(
                v_virtual_list(
                    cx.entity().clone(),
                    "agent-chat-messages-virtual-list",
                    display_item_sizes,
                    move |this, range: std::ops::Range<usize>, window, cx: &mut Context<Self>| {
                        range
                            .map(|ix| {
                                this.render_display_row(
                                    ix,
                                    display_count,
                                    render_now_ms,
                                    window,
                                    cx,
                                )
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .track_scroll(&self.messages_scroll_handle)
                .size_full(),
            )
            .child(div().absolute().inset_0().child(Scrollbar::vertical(
                &self.messages_scroll_state,
                &self.messages_scroll_handle,
            )))
            .child(
                canvas(
                    {
                        let panel = cx.entity().clone();
                        move |bounds, _, cx| {
                            let h = bounds.size.height;
                            panel.update(cx, |p, cx| {
                                if p.chat_viewport_height != h {
                                    p.chat_viewport_height = h;
                                    cx.notify();
                                }
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .when(self.render_auto_scroll_safety_net(), |el| {
                el.child(
                    div().absolute().bottom(px(16.0)).right(px(28.0)).child(
                        Button::new("agent-chat-jump-bottom")
                            .icon(IconName::ArrowDown)
                            .xsmall()
                            .primary()
                            .tooltip("Jump to bottom (re-enable auto-scroll)")
                            .on_click(cx.listener(|this, _, _, _cx| {
                                this.jump_to_bottom();
                            })),
                    ),
                )
            })
    }

    /// Render one row of the virtual list; `ix == display_count` is the trailing spacer.
    fn render_display_row(
        &self,
        ix: usize,
        display_count: usize,
        render_now_ms: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if ix == display_count {
            return div().h(px(120.0)).into_any_element();
        }

        let Some(item) = self.display_items.get(ix) else {
            return div().h(px(52.0)).into_any_element();
        };

        let panel = cx.entity().clone();

        match item {
            DisplayItem::ToolCallGroup {
                calls,
                is_expanded,
                started_at_ms,
                finished_at_ms,
            } => Self::render_tool_call_group_item(
                ix,
                calls,
                *is_expanded,
                *started_at_ms,
                *finished_at_ms,
                render_now_ms,
                panel,
                cx,
            ),
            DisplayItem::CompactionSummary {
                summary,
                is_expanded,
                started_at_ms,
                finished_at_ms,
            } => Self::render_compaction_summary_item(
                ix,
                summary,
                *is_expanded,
                *started_at_ms,
                *finished_at_ms,
                render_now_ms,
                panel,
                cx,
            ),
            DisplayItem::SystemPrompt {
                content,
                is_expanded,
                is_outdated,
            } => {
                Self::render_system_prompt_item(ix, content, *is_expanded, *is_outdated, panel, cx)
            }
            DisplayItem::ThinkingBlock {
                content,
                is_expanded,
                is_done,
                started_at_ms,
                finished_at_ms,
            } => Self::render_thinking_block_item(
                ix,
                content,
                *is_expanded,
                *is_done,
                *started_at_ms,
                *finished_at_ms,
                render_now_ms,
                panel,
                cx,
            ),
            DisplayItem::SubagentInvocation {
                subagent_id: _,
                name,
                task,
                steps,
                is_expanded,
                status,
                started_at_ms,
                finished_at_ms,
            } => Self::render_subagent_invocation_item(
                ix,
                name,
                task,
                steps,
                *is_expanded,
                *status,
                *started_at_ms,
                *finished_at_ms,
                render_now_ms,
                panel,
                cx,
            ),
            DisplayItem::UserMessage {
                content,
                message_index,
            }
            | DisplayItem::AssistantMessage {
                content,
                message_index,
                ..
            } => {
                let is_user = matches!(item, DisplayItem::UserMessage { .. });
                let is_streaming = matches!(
                    item,
                    DisplayItem::AssistantMessage {
                        is_streaming: true,
                        ..
                    }
                );
                self.render_message_item(
                    ix,
                    display_count,
                    is_user,
                    is_streaming,
                    content,
                    *message_index,
                    panel,
                    window,
                    cx,
                )
            }
        }
    }

    /// Prompt text input with send/stop buttons and the queued-subagent indicator.
    pub(super) fn render_input_area(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let queued_subagent_count = self.pending_subagent_events.len();

        v_flex()
            .w_full()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            // Input row: text input + send/stop
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .items_end()
                    .child(TextInput::new(&self.prompt_input).flex_1().min_w_0())
                    .when(self.is_request_in_flight, |this| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(Spinner::new().with_size(Size::Small))
                                .child(
                                    Button::new("agent-chat-stop")
                                        .icon(IconName::Square)
                                        .xsmall()
                                        .ghost()
                                        .tooltip("Stop generation")
                                        .on_click(cx.listener(|this, _, _, _cx| {
                                            if let Some(tx) = this.cancel_tx.take() {
                                                let _ = tx.try_send(());
                                            }
                                        })),
                                ),
                        )
                    })
                    .child(
                        Button::new("agent-chat-send")
                            .icon(IconName::Send)
                            .label("Send")
                            .primary()
                            .tooltip("Send message (Enter)")
                            .disabled(
                                self.is_request_in_flight
                                    || self
                                        .prompt_input
                                        .read(cx)
                                        .text()
                                        .to_string()
                                        .trim()
                                        .is_empty(),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.send_prompt(window, cx);
                            })),
                    ),
            )
            // Subagent indicator: passive dot + count, only when active
            .when(
                queued_subagent_count > 0 || self.is_processing_subagent_event,
                |el| {
                    el.child(
                        h_flex()
                            .w_full()
                            .gap_1()
                            .items_center()
                            .pt(px(2.0))
                            .child(div().size(px(5.0)).rounded_full().bg(
                                if self.is_processing_subagent_event {
                                    cx.theme().info
                                } else {
                                    cx.theme().warning
                                },
                            ))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground.opacity(0.7))
                                    .child(format!(
                                        "{} subagent{} queued",
                                        queued_subagent_count,
                                        if queued_subagent_count == 1 { "" } else { "s" },
                                    )),
                            ),
                    )
                },
            )
    }
}
