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
        .p_6()
        .child(
            v_flex()
                .items_center()
                .gap_3()
                .max_w(px(360.))
                .p_6()
                .rounded_xl()
                .bg(cx.theme().secondary.opacity(0.18))
                .border_1()
                .border_color(cx.theme().border.opacity(0.45))
                .child(
                    Icon::new(IconName::Puzzle)
                        .size(px(48.))
                        .text_color(cx.theme().muted_foreground.opacity(0.65)),
                )
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child("No plugins available"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_center()
                        .text_color(cx.theme().muted_foreground)
                        .child("Add plugin DLLs to the plugins/editor directory to get started."),
                ),
        )
}
