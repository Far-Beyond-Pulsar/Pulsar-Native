use gpui::*;
use ui::{
    ActiveTheme as _, Icon, IconName, button::{Button, ButtonVariants as _},
    clipboard::Clipboard, h_flex, v_flex,
};

use crate::screen::MultiplayerWindow;
use crate::utils::types::ActiveSession;

fn meta_label(text: &'static str, cx: &Context<MultiplayerWindow>) -> Div {
    div().text_size(px(10.)).font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground).child(text)
}

pub fn render_session_info_tab(
    _this: &MultiplayerWindow,
    session: &ActiveSession,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    let session_id = session.session_id.clone();
    let join_token = session.join_token.clone();
    let server = session.server_address.clone();
    v_flex()
        .flex_shrink_0()
        .gap_3()
        .p_3()
        .child(
            h_flex().items_center().gap_2()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().foreground).child("Session"))
                .child(h_flex().ml_auto().items_center().gap_1().px_2().py_1().rounded(px(8.))
                    .bg(cx.theme().success.opacity(0.1))
                    .child(div().size(px(6.)).rounded_full().bg(cx.theme().success))
                    .child(div().text_size(px(10.)).text_color(cx.theme().success).child("Connected")))
                .child(Button::new("disconnect-session").icon(IconName::Close).ghost().compact()
                    .tooltip("Leave session")
                    .on_click(cx.listener(|this, _, window, cx| {
                        crate::handlers::on_disconnect(this, window, cx);
                    }))),
        )
        .child(
            v_flex().gap_2().p_3().rounded(px(8.))
                .border_1().border_color(cx.theme().border).bg(cx.theme().sidebar)
                .child(
                    h_flex().items_center().gap_2()
                        .child(Icon::new(IconName::Link).size(px(13.)).text_color(cx.theme().primary))
                        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground).child("Invite to this session"))
                        .child(div().ml_auto().text_size(px(10.))
                            .text_color(cx.theme().muted_foreground).child("Copy details"))
                        .child(Clipboard::new("copy-join-command").value_fn({
                            let id = session_id.clone();
                            let token = join_token.clone();
                            move |_, _| SharedString::from(format!("Session: {id}\nPassword: {token}"))
                        })),
                )
                .child(div().h(px(1.)).w_full().bg(cx.theme().border))
                .child(
                    h_flex().items_center().gap_2()
                        .child(v_flex().flex_1().min_w_0().gap_1()
                            .child(meta_label("SESSION ID", cx))
                            .child(div().text_xs().text_color(cx.theme().foreground).child(session_id.clone())))
                        .child(Clipboard::new("copy-session-id").value_fn({
                            let id = session_id.clone(); move |_, _| SharedString::from(id.clone())
                        })),
                )
                .child(
                    h_flex().items_center().gap_2()
                        .child(v_flex().flex_1().min_w_0().gap_1()
                            .child(meta_label("PASSWORD", cx))
                            .child(div().text_xs().text_color(cx.theme().foreground).child(join_token.clone())))
                        .child(Clipboard::new("copy-password").value_fn({
                            let token = join_token.clone(); move |_, _| SharedString::from(token.clone())
                        })),
                )
                .child(
                    h_flex().items_center().gap_2()
                        .child(Icon::new(IconName::Network).size(px(12.)).text_color(cx.theme().muted_foreground))
                        .child(div().text_size(px(10.)).text_color(cx.theme().muted_foreground).child(server)),
                ),
        )
}
