use gpui::{prelude::*, *};
use ui::{h_flex, v_flex};

/// Render an info section with key-value pairs
pub fn render_info_section(title: &str, items: Vec<(&str, String)>, theme: &ui::theme::Theme) -> impl IntoElement {
    v_flex()
        .gap_2()
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(title.to_string())
        )
        .child(
            v_flex()
                .gap_2()
                .p_4()
                .border_1()
                .border_color(theme.border)
                .rounded_lg()
                .bg(theme.sidebar)
                .children(items.into_iter().map(|(key, value)| {
                    h_flex()
                        .justify_between()
                        .gap_4()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(key.to_string())
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child(value)
                        )
                }))
        )
}

/// Render a size bar visualization
pub fn render_size_bar(label: &str, size: u64, total: u64, color: Hsla, theme: &ui::theme::Theme) -> impl IntoElement {
    let percentage = if total > 0 {
        ((size as f64 / total as f64) * 100.0) as f32
    } else {
        0.0
    };
    
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.foreground)
                        .child(label.to_string())
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(format!("{} ({:.1}%)", crate::entry_screen::views::project_settings::types::format_size(Some(size)), percentage))
                )
        )
        .child(
            div()
                .w_full()
                .h(px(8.))
                .bg(theme.border)
                .rounded_full()
                .child(
                    div()
                        .w(relative(percentage / 100.0))
                        .h_full()
                        .bg(color)
                        .rounded_full()
                )
        )
}
