use gpui::*;
use ui::{button::Button, clipboard::Clipboard, h_flex, v_flex, ActiveTheme as _, Icon, IconName};

use crate::screen::MultiplayerWindow;
use crate::utils::types::ActiveSession;

fn label(text: &'static str, cx: &Context<MultiplayerWindow>) -> Div {
    div()
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

pub fn render_session_info_tab(
    _this: &MultiplayerWindow,
    session: &ActiveSession,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    let session_id = session.session_id.clone();
    let token = session.join_token.clone();
    let server = session.server_address.clone();
    v_flex()
        .flex_shrink_0()
        .gap_3()
        .p_4()
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::Network)
                        .size(px(15.))
                        .text_color(cx.theme().primary),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child("Session details"),
                )
                .child(
                    div()
                        .ml_auto()
                        .px_2()
                        .py_0p5()
                        .rounded(px(8.))
                        .bg(cx.theme().success.opacity(0.12))
                        .text_size(px(10.))
                        .text_color(cx.theme().success)
                        .child("CONNECTED"),
                ),
        )
        .child(
            v_flex().gap_2().child(label("SESSION ID", cx)).child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .child(session_id.clone()),
                    )
                    .child(Clipboard::new("copy-session-id").value_fn({
                        let id = session_id.clone();
                        move |_, _| SharedString::from(id.clone())
                    })),
            ),
        )
        .child(
            h_flex()
                .gap_3()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(label("PASSWORD", cx))
                        .child(Clipboard::new("copy-password").value_fn({
                            let token = token.clone();
                            move |_, _| SharedString::from(token.clone())
                        })),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(label("SERVER", cx))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().foreground)
                                .child(server),
                        ),
                ),
        )
        .child(Clipboard::new("copy-join-command").value_fn({
            let id = session_id.clone();
            let token = token.clone();
            move |_, _| SharedString::from(format!("Session: {id}\nPassword: {token}"))
        }))
        .child(
            Button::new("disconnect")
                .label("Disconnect")
                .icon(IconName::Close)
                .w_full()
                .on_click(cx.listener(|this, _, window, cx| {
                    crate::handlers::on_disconnect(this, window, cx);
                })),
        )
}
