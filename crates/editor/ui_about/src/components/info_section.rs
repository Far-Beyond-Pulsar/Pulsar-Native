use chrono::Datelike;

use gpui::*;
use ui::{v_flex, ActiveTheme};

pub fn render_title_version(theme: &ui::Theme) -> impl IntoElement {
    v_flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .text_3xl()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(theme.foreground)
                .child("Pulsar Engine"),
        )
        .child(
            div()
                .px_4()
                .py_1p5()
                .rounded_full()
                .bg(theme.accent.opacity(0.15))
                .border_1()
                .border_color(theme.accent.opacity(0.3))
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Version 0.1.47"),
                ),
        )
}

pub fn render_divider(theme: &ui::Theme) -> impl IntoElement {
    div()
        .w_full()
        .h_px()
        .bg(theme.border.opacity(0.5))
}

pub fn render_description(theme: &ui::Theme) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_3()
        .p_6()
        .rounded_lg()
        .bg(theme.background.opacity(0.5))
        .border_1()
        .border_color(theme.border.opacity(0.3))
        .child(
            div()
                .text_base()
                .text_center()
                .line_height(rems(1.6))
                .text_color(theme.foreground.opacity(0.9))
                .child(
                    "A modern, high-performance game engine built with Rust. Designed for flexibility, speed, and developer experience.",
                ),
        )
}

pub fn render_copyright(theme: &ui::Theme) -> impl IntoElement {
    let current_year = chrono::Local::now().year();
    div()
        .text_sm()
        .text_color(theme.muted_foreground)
        .child(format!("© {} Pulsar Engine Contributors", current_year))
}
