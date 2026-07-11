use gpui::*;
use ui::{v_flex, ActiveTheme as _, Icon, IconName, StyledExt};

pub fn render_empty_state(
    cx: &mut Context<crate::screen::PluginManagerWindow>,
) -> impl IntoElement {
    v_flex()
        .flex_1()
        .w_full()
        .items_center()
        .justify_center()
        .gap_3()
        .child(
            Icon::new(IconName::Puzzle)
                .size(px(64.))
                .text_color(cx.theme().muted_foreground.opacity(0.5)),
        )
        .child(
            div()
                .text_lg()
                .text_color(cx.theme().muted_foreground)
                .child("No plugins loaded"),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground.opacity(0.7))
                .child("Place plugin DLLs in the plugins/editor directory"),
        )
}
