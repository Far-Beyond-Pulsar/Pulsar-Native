use gpui::*;
use ui::{
    button::{Button, ButtonVariants},
    h_flex, v_flex, ActiveTheme, ContextModal, IconName,
};

/// Open a gpui modal displaying a GitHub device code for the user to enter
/// at the verification URL. Returns immediately; the modal is managed by the
/// window's modal layer.
pub fn open_device_code_modal(
    code: &str,
    verification_url: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let c = code.to_string();
    let u = verification_url.to_string();
    window.open_modal(cx, move |modal, _, cx| {
        let copy_code = c.clone();
        let open_url = u.clone();
        modal
            .width(px(460.))
            .title("GitHub Device Code")
            .show_close(true)
            .overlay_closable(true)
            .on_close(|_, _, _| {})
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Enter this code in the browser window GitHub opened."),
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
                            .font_weight(FontWeight::BOLD)
                            .text_color(cx.theme().foreground)
                            .child(c.clone()),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("device-code-copy")
                                    .primary()
                                    .icon(IconName::Copy)
                                    .label("Copy")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy_code.clone(),
                                        ));
                                    }),
                            )
                            .child(
                                Button::new("device-code-open")
                                    .ghost()
                                    .icon(IconName::ExternalLink)
                                    .label("Open")
                                    .on_click(move |_, _, cx| {
                                        cx.open_url(&open_url);
                                    }),
                            )
                            .child(
                                Button::new("device-code-close")
                                    .ghost()
                                    .icon(IconName::X)
                                    .label("Close")
                                    .on_click(|_, window, cx| {
                                        window.close_modal(cx);
                                    }),
                            ),
                    ),
            )
    });
}
