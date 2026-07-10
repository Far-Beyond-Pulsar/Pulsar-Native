use gpui::prelude::*;
use gpui::*;
use ui::{
    button::{Button, ButtonVariants},
    h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable,
};

use crate::screen::EntryScreen;

/// Render the GitHub device-code modal as a full-screen overlay.
/// Shows the 8-digit code with buttons to open the browser, copy the code, or close.
pub fn render_auth_modal(
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let Some(code) = screen.state.auth.device_code.clone() else {
        return div().into_any_element();
    };
    let verification_url = screen.state.auth.device_verification_url.clone();
    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(cx.theme().background.opacity(0.86))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .child(
            v_flex()
                .w_full()
                .max_w(px(460.))
                .p_6()
                .gap_4()
                .rounded_xl()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .shadow_lg()
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child("GitHub Device Code"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Paste this 8-digit code in the browser window GitHub opened."),
                )
                .child(
                    div()
                        .w_full()
                        .py_3()
                        .rounded_lg()
                        .bg(cx.theme().accent.opacity(0.12))
                        .border_1()
                        .border_color(cx.theme().accent.opacity(0.35))
                        .text_center()
                        .text_2xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(cx.theme().foreground)
                        .child(code.clone()),
                )
                .when_some(
                    screen.state.ui.auth_device_copy_notice.clone(),
                    |this, notice| {
                        this.child(div().text_xs().text_color(cx.theme().success).child(notice))
                    },
                )
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("github-device-code-copy")
                                .primary()
                                .icon(IconName::Copy)
                                .label("Copy Code")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        code.clone(),
                                    ));
                                    this.state.ui.auth_device_copy_notice =
                                        Some("Code copied.".to_string());
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("github-device-code-open")
                                .ghost()
                                .icon(IconName::ExternalLink)
                                .label("Open")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    if let Some(url) = verification_url.clone() {
                                        cx.open_url(&url);
                                    }
                                })),
                        )
                        .child(
                            Button::new("github-device-code-close")
                                .ghost()
                                .icon(IconName::X)
                                .label("Close")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.ui.auth_device_modal_visible = false;
                                    cx.notify();
                                })),
                        ),
                ),
        )
        .into_any_element()
}
