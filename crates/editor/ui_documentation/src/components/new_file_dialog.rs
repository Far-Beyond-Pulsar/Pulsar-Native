use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Icon, IconName, Sizable, StyledExt,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    v_flex,
};

use crate::DocumentationWindow;
use crate::handlers;

pub fn render_new_file_dialog(
    window: &DocumentationWindow,
    theme: &ui::ThemeColor,
    window_handle: &mut Window,
    cx: &mut Context<DocumentationWindow>,
) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::black().opacity(0.6))
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                handlers::close_new_file_dialog(this);
                cx.notify();
            }),
        )
        .child(
            div()
                .w(px(480.0))
                .bg(theme.background)
                .border_1()
                .border_color(theme.border)
                .rounded_xl()
                .shadow_2xl()
                .overflow_hidden()
                .on_mouse_down(gpui::MouseButton::Left, |_event, _phase, cx| {
                    cx.stop_propagation();
                })
                .child(
                    v_flex()
                        .child(dialog_header(window, theme, cx))
                        .child(dialog_body(window, theme))
                        .child(dialog_footer(window, theme, window_handle, cx)),
                ),
        )
}

fn dialog_header(
    window: &DocumentationWindow,
    theme: &ui::ThemeColor,
    cx: &mut Context<DocumentationWindow>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(px(56.0))
        .px_6()
        .items_center()
        .justify_between()
        .bg(theme.sidebar)
        .border_b_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(Icon::new(IconName::Plus).size_4())
                .child(
                    div()
                        .text_base()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Create Documentation File"),
                ),
        )
        .child(
            Button::new("close-dialog")
                .icon(IconName::Close)
                .ghost()
                .xsmall()
                .on_click(cx.listener(|this, _, _, cx| {
                    handlers::close_new_file_dialog(this);
                    cx.notify();
                })),
        )
}

fn dialog_body(
    window: &DocumentationWindow,
    theme: &ui::ThemeColor,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .p_6()
        .gap_4()
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.foreground)
                        .child("File Name"),
                )
                .child(
                    TextInput::new(&window.new_file_input_state)
                        .w_full()
                        .appearance(true)
                        .bordered(true),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("File will be saved with .md extension"),
                ),
        )
}

fn dialog_footer(
    window: &DocumentationWindow,
    theme: &ui::ThemeColor,
    _window_handle: &mut Window,
    cx: &mut Context<DocumentationWindow>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(px(64.0))
        .px_6()
        .items_center()
        .gap_3()
        .justify_end()
        .bg(theme.sidebar.opacity(0.5))
        .border_t_1()
        .border_color(theme.border)
        .child(
            Button::new("cancel-new-file")
                .label("Cancel")
                .ghost()
                .on_click(cx.listener(|this, _, _, cx| {
                    handlers::close_new_file_dialog(this);
                    cx.notify();
                })),
        )
        .child(
            Button::new("create-new-file")
                .label("Create File")
                .icon(IconName::Plus)
                .primary()
                .on_click(cx.listener(move |this, _, window, cx| {
                    handlers::create_new_file(this, window, cx);
                    cx.notify();
                })),
        )
}
