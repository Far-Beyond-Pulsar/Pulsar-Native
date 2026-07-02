use gpui::*;
use ui::theme::Theme;
use ui::{h_flex, v_flex, ActiveTheme as _};

use crate::screen::EntryScreen;

pub fn render_info_section(
    items: Vec<(String, String)>,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .gap_2()
        .children(items.into_iter().map(|(label, value)| {
            h_flex()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.secondary.opacity(0.08))
                .child(
                    div()
                        .w(px(140.))
                        .flex_shrink_0()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(theme.foreground)
                        .child(value),
                )
        }))
}

pub fn render_size_bar(
    label: &str,
    size_bytes: u64,
    total_bytes: u64,
    color: gpui::Hsla,
    theme: &ui::Theme,
) -> impl IntoElement {
    let fraction = if total_bytes > 0 { (size_bytes as f32 / total_bytes as f32).min(1.0) } else { 0.0 };
    let formatted = crate::util::formatters::format_size(size_bytes);

    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .child(div().text_xs().text_color(theme.muted_foreground).child(label.to_string()))
                .child(div().text_xs().text_color(theme.muted_foreground).child(formatted)),
        )
        .child(
            div()
                .w_full()
                .h(px(6.))
                .bg(theme.secondary.opacity(0.3))
                .rounded_full()
                .child(
                    div()
                        .h_full()
                        .rounded_full()
                        .bg(color)
                        .w(relative(fraction)),
                ),
        )
}
