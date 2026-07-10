use gpui::prelude::*;
use gpui::*;
use ui::{button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _};

use crate::screen::EntryScreen;

pub fn render_modal(
    title: impl IntoElement,
    content: impl IntoElement,
    on_close: Option<Box<dyn Fn(&mut Window, &mut Context<EntryScreen>)>>,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();

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
                .max_w(px(480.0))
                .p_6()
                .gap_4()
                .rounded_xl()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .shadow_lg()
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(title),
                        )
                        .when_some(on_close, |this, close| {
                            this.child(Button::new("modal-close").ghost().label("X").on_click(
                                cx.listener(move |this, _, window, cx| {
                                    close(window, cx);
                                    cx.notify();
                                }),
                            ))
                        }),
                )
                .child(content),
        )
        .into_any_element()
}
