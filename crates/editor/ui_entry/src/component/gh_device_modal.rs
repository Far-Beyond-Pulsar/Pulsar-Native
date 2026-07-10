use gpui::prelude::*;
use gpui::*;
use ui::{button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _};

use crate::screen::EntryScreen;

pub fn render_gh_device_modal(
    user_code: &str,
    verification_url: &str,
    copy_notice: Option<SharedString>,
    on_copy: Box<dyn Fn(&mut Window, &mut Context<EntryScreen>)>,
    on_cancel: Box<dyn Fn(&mut Window, &mut Context<EntryScreen>)>,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let code = user_code.to_string();
    let ver_url = verification_url.to_string();

    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.background.opacity(0.86))
        .child(
            v_flex()
                .w_full()
                .max_w(px(460.0))
                .p_6()
                .gap_4()
                .rounded_xl()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .shadow_lg()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("GitHub Device Code"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "Enter this code at {} to complete sign-in.",
                            ver_url
                        )),
                )
                .child(
                    div()
                        .w_full()
                        .py_3()
                        .rounded_lg()
                        .bg(theme.accent.opacity(0.12))
                        .border_1()
                        .border_color(theme.accent.opacity(0.35))
                        .text_center()
                        .text_2xl()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child(code.clone()),
                )
                .when_some(copy_notice, |this, notice| {
                    this.child(div().text_xs().text_color(theme.success).child(notice))
                })
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("gh-device-cancel")
                                .ghost()
                                .label("Cancel")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    (on_cancel)(window, cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("gh-device-copy")
                                .primary()
                                .label("Copy Code")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    (on_copy)(window, cx);
                                    cx.notify();
                                })),
                        ),
                ),
        )
        .into_any_element()
}
