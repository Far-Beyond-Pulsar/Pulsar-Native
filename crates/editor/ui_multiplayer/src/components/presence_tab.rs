use gpui::{prelude::FluentBuilder, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, IconName,
};

use crate::screen::MultiplayerWindow;

pub fn render_presence_tab(
    this: &MultiplayerWindow,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    let is_host = this
        .active_session
        .as_ref()
        .and_then(|session| session.connected_users.first())
        .map(|first| Some(first) == this.current_peer_id.as_ref())
        .unwrap_or(false);

    v_flex()
        .size_full()
        .min_h_0()
        .child(
            h_flex()
                .flex_shrink_0()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child("People"),
                )
                .child(
                    div()
                        .px_1p5()
                        .rounded(px(8.))
                        .bg(cx.theme().secondary)
                        .text_size(px(10.))
                        .text_color(cx.theme().muted_foreground)
                        .child(this.user_presences.len().to_string()),
                )
                .child(div().flex_1())
                .when(is_host, |row| {
                    row.child(
                        div()
                            .text_size(px(10.))
                            .text_color(cx.theme().muted_foreground)
                            .child("You are host"),
                    )
                }),
        )
        .child(
            div().flex_1().min_h_0().overflow_y_scroll().p_2().child(
                v_flex()
                    .gap_1()
                    .when(this.user_presences.is_empty(), |list| {
                        list.child(
                            div()
                                .py_5()
                                .text_center()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("No participants yet"),
                        )
                    })
                    .children(this.user_presences.iter().map(|presence| {
                        let is_self = Some(&presence.peer_id) == this.current_peer_id.as_ref();
                        let display_name = if is_self {
                            "You".to_string()
                        } else if presence.peer_id.len() > 14 {
                            format!("{}…", &presence.peer_id[..12])
                        } else {
                            presence.peer_id.clone()
                        };
                        let color = rgb(((presence.color[0].clamp(0., 1.) * 255.) as u32) << 16
                            | ((presence.color[1].clamp(0., 1.) * 255.) as u32) << 8
                            | (presence.color[2].clamp(0., 1.) * 255.) as u32);
                        let jump_id = SharedString::from(format!("jump-{}", presence.peer_id));
                        let kick_id = SharedString::from(format!("kick-{}", presence.peer_id));
                        let jump_peer = presence.peer_id.clone();
                        let kick_peer = presence.peer_id.clone();

                        h_flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_2()
                            .rounded(px(6.))
                            .hover(|row| row.bg(cx.theme().secondary.opacity(0.65)))
                            .child(div().size(px(8.)).rounded_full().bg(color))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(cx.theme().foreground)
                                            .child(display_name),
                                    )
                                    .when_some(presence.editing_file.as_ref(), |row, file| {
                                        row.child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(cx.theme().muted_foreground)
                                                .overflow_hidden()
                                                .child(format!("Editing {file}")),
                                        )
                                    })
                                    .when(presence.editing_file.is_none(), |row| {
                                        row.child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(cx.theme().muted_foreground)
                                                .child(presence.activity_status().to_string()),
                                        )
                                    }),
                            )
                            .when(is_self && is_host, |row| {
                                row.child(
                                    div()
                                        .text_size(px(9.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(cx.theme().primary)
                                        .child("HOST"),
                                )
                            })
                            .when(!is_self, |row| {
                                row.child(
                                    Button::new(jump_id)
                                        .icon(IconName::Eye)
                                        .ghost()
                                        .compact()
                                        .tooltip("Jump to view")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            crate::handlers::on_jump_to_user(
                                                this,
                                                jump_peer.clone(),
                                                window,
                                                cx,
                                            );
                                        })),
                                )
                                .when(is_host, |row| {
                                    row.child(
                                        Button::new(kick_id)
                                            .icon(IconName::Close)
                                            .ghost()
                                            .compact()
                                            .tooltip("Remove participant")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                crate::handlers::on_kick_user(
                                                    this,
                                                    kick_peer.clone(),
                                                    window,
                                                    cx,
                                                );
                                            })),
                                    )
                                })
                            })
                            .into_any_element()
                    })),
            ),
        )
}
