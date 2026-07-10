use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{
    button::Button,
    h_flex,
    input::TextInput,
    v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, StyledExt,
};

use crate::screen::MultiplayerWindow;
use crate::utils::format::format_timestamp;

pub fn render_chat_tab(
    this: &MultiplayerWindow,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    v_flex()
        .size_full()
        .child(
            div().flex_1().p_4().id("chat-messages").child(
                v_flex()
                    .gap_3()
                    .when(this.chat_messages.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::ChatBubble)
                                        .size(px(48.))
                                        .text_color(cx.theme().muted_foreground.opacity(0.3)),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No messages yet"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground.opacity(0.7))
                                        .child("Start chatting with your team!"),
                                ),
                        )
                    })
                    .children(this.chat_messages.iter().map(|msg| {
                        let peer_name = if msg.is_self {
                            "You".to_string()
                        } else {
                            if msg.peer_id.len() > 8 {
                                format!("{}...", &msg.peer_id[..8])
                            } else {
                                msg.peer_id.clone()
                            }
                        };

                        let timestamp_str = format_timestamp(msg.timestamp);

                        v_flex()
                            .gap_1()
                            .when(msg.is_self, |this| this.items_end())
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_baseline()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_medium()
                                            .text_color(cx.theme().foreground)
                                            .child(peer_name),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(timestamp_str),
                                    ),
                            )
                            .child(
                                div()
                                    .max_w(px(400.))
                                    .px_3()
                                    .py_2()
                                    .rounded(px(8.))
                                    .bg(if msg.is_self {
                                        cx.theme().primary
                                    } else {
                                        cx.theme().secondary
                                    })
                                    .text_sm()
                                    .text_color(if msg.is_self {
                                        cx.theme().primary_foreground
                                    } else {
                                        cx.theme().foreground
                                    })
                                    .child(msg.message.clone()),
                            )
                            .into_any_element()
                    })),
            ),
        )
        .child(
            v_flex()
                .gap_2()
                .p_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(TextInput::new(&this.chat_input).flex_1())
                        .child(
                            Button::new("send")
                                .label("Send")
                                .icon(IconName::Send)
                                .disabled(this.chat_input.read(cx).text().len() == 0)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    crate::handlers::on_send_chat(this, window, cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Press Enter to send"),
                ),
        )
}
