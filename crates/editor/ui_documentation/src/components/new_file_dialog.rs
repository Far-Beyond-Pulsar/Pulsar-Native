use gpui::{prelude::*, *};
use ui::{
    ActiveTheme as _, Disableable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    v_flex,
};

use crate::DocumentationWindow;
use crate::handlers;

/// A small modal asking for the name of a new guide.
pub fn render_new_file_dialog(
    window: &DocumentationWindow,
    cx: &mut Context<DocumentationWindow>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let can_create = !window.new_file_name.trim().is_empty();

    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::black().opacity(0.5))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                handlers::close_new_file_dialog(this);
                cx.notify();
            }),
        )
        .child(
            v_flex()
                .w(px(420.0))
                .gap_4()
                .p_5()
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_lg)
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("New guide"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Saved as a Markdown file in the project's docs folder."),
                        ),
                )
                .child(TextInput::new(&window.new_file_input_state).w_full())
                .child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("cancel-new-guide")
                                .label("Cancel")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    handlers::close_new_file_dialog(this);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("create-new-guide")
                                .label("Create")
                                .primary()
                                .disabled(!can_create)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    handlers::create_new_file(this, window, cx);
                                    cx.notify();
                                })),
                        ),
                ),
        )
}
