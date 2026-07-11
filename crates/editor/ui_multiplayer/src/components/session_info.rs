use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{
    button::Button,
    clipboard::Clipboard,
    h_flex,
    v_flex, ActiveTheme as _, Icon, IconName, StyledExt,
};

use crate::screen::MultiplayerWindow;
use crate::utils::types::ActiveSession;

pub fn render_session_info_tab(
    this: &MultiplayerWindow,
    session: &ActiveSession,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    let session_id = session.session_id.clone();
    let join_token = session.join_token.clone();
    let server_address = session.server_address.clone();

    v_flex()
        .gap_3()
        .p_4()
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("SESSION ID"),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(cx.theme().foreground)
                                .child(session_id.clone()),
                        )
                        .child(
                            Clipboard::new("copy-session-id")
                                .value_fn({
                                    let id = session_id.clone();
                                    move |_, _| SharedString::from(id.clone())
                                })
                                .on_copied(|_, _window, _cx| {
                                    tracing::debug!("Session ID copied to clipboard");
                                }),
                        ),
                ),
        )
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("PASSWORD"),
                )
                .child(
                    h_flex().gap_2().items_center().child(
                        Clipboard::new("copy-password")
                            .value_fn({
                                let token = join_token.clone();
                                move |_, _| SharedString::from(token.clone())
                            })
                            .on_copied(|_, _window, _cx| {
                                tracing::debug!("Password copied to clipboard");
                            }),
                    ),
                ),
        )
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("SERVER"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(server_address),
                ),
        )
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("INVITE"),
                )
                .child(
                    Clipboard::new("copy-join-command")
                        .value_fn({
                            let id = session_id.clone();
                            let token = join_token.clone();
                            move |_, _| {
                                SharedString::from(format!(
                                    "Session: {}\nPassword: {}",
                                    id, token
                                ))
                            }
                        })
                        .on_copied(|_, _window, _cx| {
                            tracing::debug!("Join credentials copied to clipboard");
                        }),
                ),
        )
        .child(div().h(px(1.)).w_full().bg(cx.theme().border))
        .child(
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::User)
                                .size(px(16.))
                                .text_color(cx.theme().primary),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_bold()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} PARTICIPANT{}",
                                    session.connected_users.len(),
                                    if session.connected_users.len() == 1 {
                                        ""
                                    } else {
                                        "S"
                                    }
                                )),
                        )
                        .child(
                            div()
                                .ml_auto()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().size(px(6.)).rounded_full().bg(rgb(0x00ff00)))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Active"),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .gap_1()
                    .children(
                        this.format_participants(&session.connected_users)
                            .iter()
                                .map(|user| {
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .px_3()
                                        .py_2()
                                        .rounded(px(6.))
                                        .bg(cx.theme().secondary)
                                        .child(
                                            Icon::new(IconName::User)
                                                .size(px(14.))
                                                .text_color(cx.theme().muted_foreground),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(cx.theme().foreground)
                                                .child(user.clone()),
                                        )
                                        .when(user.contains("(Host)"), |this| {
                                            this.child(
                                                div()
                                                    .ml_auto()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded(px(4.))
                                                    .bg(cx.theme().primary)
                                                    .text_xs()
                                                    .font_bold()
                                                    .text_color(cx.theme().primary_foreground)
                                                    .child("HOST"),
                                            )
                                        })
                                        .into_any_element()
                                }),
                        )
                        .when(session.connected_users.is_empty(), |this| {
                            this.child(
                                div()
                                    .text_sm()
                                    .text_center()
                                    .py_4()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("No participants yet"),
                            )
                        }),
                ),
        )
        .child(
            div().mt_4().child(
                Button::new("disconnect")
                    .label("Disconnect")
                    .w_full()
                    .on_click(cx.listener(|this, _, window, cx| {
                        crate::handlers::on_disconnect(this, window, cx);
                    })),
            ),
        )
}
