use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{
    button::Button,
    h_flex,
    v_flex, ActiveTheme as _, Icon, IconName, StyledExt,
};

use crate::screen::MultiplayerWindow;

pub fn render_presence_tab(
    this: &MultiplayerWindow,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    let is_host = this
        .active_session
        .as_ref()
        .and_then(|s| s.connected_users.first())
        .map(|first_peer| Some(first_peer) == this.current_peer_id.as_ref())
        .unwrap_or(false);

    v_flex()
        .size_full()
        .child(
            v_flex()
                .p_4()
                .border_b_1()
                .border_color(cx.theme().border)
                .gap_2()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Activity)
                                .size(px(20.))
                                .text_color(cx.theme().primary),
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_bold()
                                .text_color(cx.theme().foreground)
                                .child("User Presence & Management"),
                        ),
                )
                .when(is_host, |this| {
                    this.child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(4.))
                            .bg(cx.theme().primary.opacity(0.1))
                            .text_xs()
                            .text_color(cx.theme().primary)
                            .child("You have host privileges"),
                    )
                }),
        )
        .child(
            div().flex_1().p_4().child(
                v_flex()
                    .gap_3()
                    .when(this.user_presences.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::User)
                                        .size(px(48.))
                                        .text_color(
                                            cx.theme().muted_foreground.opacity(0.3),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No users connected"),
                                ),
                        )
                    })
                    .children(this.user_presences.iter().map(|presence| {
                        let is_self =
                            Some(&presence.peer_id) == this.current_peer_id.as_ref();
                        let short_id = if presence.peer_id.len() > 8 {
                            format!("{}...", &presence.peer_id[..8])
                        } else {
                            presence.peer_id.clone()
                        };

                        let (r, g, b) =
                            (presence.color[0], presence.color[1], presence.color[2]);
                        let color_value = ((r * 255.0) as u32) << 16
                            | ((g * 255.0) as u32) << 8
                            | ((b * 255.0) as u32);

                        let jump_id =
                            SharedString::from(format!("jump-{}", presence.peer_id));
                        let kick_id =
                            SharedString::from(format!("kick-{}", presence.peer_id));
                        let peer_id_for_jump = presence.peer_id.clone();
                        let peer_id_for_kick = presence.peer_id.clone();

                        v_flex()
                            .gap_3()
                            .px_4()
                            .py_3()
                            .rounded(px(8.))
                            .bg(cx.theme().secondary)
                            .border_l(px(4.))
                            .border_color(rgb(color_value))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().size(px(10.)).rounded_full().bg(
                                        if presence.is_idle {
                                            rgb(0x888888)
                                        } else {
                                            rgb(0x00ff00)
                                        },
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_sm()
                                            .font_bold()
                                            .text_color(cx.theme().foreground)
                                            .child(if is_self {
                                                format!("{} (You)", short_id)
                                            } else {
                                                short_id
                                            }),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_0p5()
                                            .rounded(px(4.))
                                            .bg(if presence.is_idle {
                                                cx.theme().muted
                                            } else {
                                                cx.theme().primary.opacity(0.2)
                                            })
                                            .text_xs()
                                            .text_color(if presence.is_idle {
                                                cx.theme().muted_foreground
                                            } else {
                                                cx.theme().primary
                                            })
                                            .child(presence.activity_status().to_string()),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .when_some(
                                        presence.current_tab.as_ref(),
                                        |this, tab| {
                                            this.child(
                                                h_flex()
                                                    .gap_1()
                                                    .child(
                                                        Icon::new(IconName::Eye)
                                                            .size(px(12.)),
                                                    )
                                                    .child(format!("Viewing: {}", tab)),
                                            )
                                        },
                                    )
                                    .when_some(
                                        presence.editing_file.as_ref(),
                                        |this, file| {
                                            this.child(
                                                h_flex()
                                                    .gap_1()
                                                    .child(
                                                        Icon::new(IconName::Edit)
                                                            .size(px(12.)),
                                                    )
                                                    .child(format!("Editing: {}", file)),
                                            )
                                        },
                                    )
                                    .when_some(
                                        presence.selected_object.as_ref(),
                                        |this, obj| {
                                            this.child(
                                                h_flex()
                                                    .gap_1()
                                                    .child(
                                                        Icon::new(IconName::Check)
                                                            .size(px(12.)),
                                                    )
                                                    .child(format!("Selected: {}", obj)),
                                            )
                                        },
                                    ),
                            )
                            .when(!is_self, |this| {
                                this.child(
                                    div()
                                        .h(px(1.))
                                        .w_full()
                                        .bg(cx.theme().border.opacity(0.5)),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .w_full()
                                        .child(
                                            Button::new(jump_id)
                                                .label("Jump to View")
                                                .icon(IconName::Eye)
                                                .flex_1()
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        crate::handlers::on_jump_to_user(
                                                            this,
                                                            peer_id_for_jump.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                )),
                                        )
                                        .when(is_host, |this| {
                                            this.child(
                                                Button::new(kick_id)
                                                    .label("Kick")
                                                    .icon(IconName::Close)
                                                    .flex_1()
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            crate::handlers::on_kick_user(
                                                                this,
                                                                peer_id_for_kick.clone(),
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    )),
                                            )
                                        }),
                                )
                            })
                            .into_any_element()
                    })),
            ),
        )
}
