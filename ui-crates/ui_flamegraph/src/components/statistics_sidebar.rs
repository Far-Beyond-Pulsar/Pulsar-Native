//! Statistics sidebar component showing session statistics

use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use crate::trace_data::TraceFrame;
use crate::constants::STATS_SIDEBAR_WIDTH;
use crate::colors::get_thread_color;
use ui::ActiveTheme;

/// Render the statistics sidebar on the right side
pub fn render_statistics_sidebar(
    frame: &Arc<TraceFrame>,
    cx: &mut Context<impl Render>,
) -> impl IntoElement {
    let setup_start = std::time::Instant::now();
    let duration_ms = frame.duration_ns() as f64 / 1_000_000.0;
    let num_frames = frame.frame_times_ms.len();
    let avg_frame_time = if !frame.frame_times_ms.is_empty() {
        frame.frame_times_ms.iter().sum::<f32>() / frame.frame_times_ms.len() as f32
    } else {
        0.0
    };

    let min_frame_time = frame.frame_times_ms.iter().copied().min_by(|a, b| a.partial_cmp(b).unwrap()).unwrap_or(0.0);
    let max_frame_time = frame.frame_times_ms.iter().copied().max_by(|a, b| a.partial_cmp(b).unwrap()).unwrap_or(0.0);
    let theme = cx.theme();

    let render_start = std::time::Instant::now();
    let result = div()
        .absolute()
        .right_0()
        .top_0()
        .w(px(STATS_SIDEBAR_WIDTH))
        .h_full()
        .bg(theme.sidebar)
        .border_l_1()
        .border_color(theme.sidebar_border)
        .flex()
        .flex_col()
        .child(
            // Header section
            div()
                .px_4()
                .py_3()
                .bg(theme.sidebar_accent.opacity(0.3))
                .border_b_1()
                .border_color(theme.sidebar_border)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.sidebar_foreground)
                        .child(t!("Flamegraph.Statistics").to_string())
                )
        )
        .child(
            // Session stats section
            div()
                .flex()
                .flex_col()
                .px_4()
                .py_3()
                .gap_2p5()
                .child(stat_row_improved(t!("Flamegraph.TotalSpans").to_string(), format!("{}", frame.spans.len()), theme))
                .child(stat_row_improved(t!("Flamegraph.Duration").to_string(), format!("{:.2} ms", duration_ms), theme))
                .child(stat_row_improved(t!("Flamegraph.MaxDepth").to_string(), format!("{}", frame.max_depth), theme))
        )
        .child(
            // Divider
            div()
                .h(px(1.0))
                .w_full()
                .bg(theme.sidebar_border)
        )
        .child(
            // Frame stats section
            div()
                .flex()
                .flex_col()
                .px_4()
                .py_3()
                .gap_2p5()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.sidebar_foreground.opacity(0.7))
                        .child(t!("Flamegraph.FrameStats").to_string())
                )
                .child(stat_row_improved(t!("Flamegraph.Frames").to_string(), format!("{}", num_frames), theme))
                .child(stat_row_improved(t!("Flamegraph.AvgFrame").to_string(), format!("{:.2} ms", avg_frame_time), theme))
                .child(stat_row_improved(t!("Flamegraph.MinFrame").to_string(), format!("{:.2} ms", min_frame_time), theme))
                .child(stat_row_improved(t!("Flamegraph.MaxFrame").to_string(), format!("{:.2} ms", max_frame_time), theme))
        )
        .child(
            // Divider
            div()
                .h(px(1.0))
                .w_full()
                .bg(theme.sidebar_border)
        )
        .child(
            // Threads section header
            div()
                .px_4()
                .py_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.sidebar_foreground.opacity(0.7))
                                .child(t!("Flamegraph.Threads").to_string())
                        )
                        .child(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .rounded(px(4.0))
                                .bg(theme.sidebar_accent.opacity(0.2))
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.sidebar_accent_foreground)
                                .child(format!("{}", frame.threads.len()))
                        )
                )
        )
        .child(
            // Threads list
            div()
                .flex()
                .flex_col()
                .px_4()
                .pb_4()
                .gap_2()
                .children(
                    frame.threads.values().take(10).map(|thread| {
                        let thread_color = get_thread_color(thread.id);

                        div()
                            .flex()
                            .items_center()
                            .gap_2p5()
                            .px_2()
                            .py_1p5()
                            .rounded(px(6.0))
                            .hover(|style| style.bg(theme.sidebar_accent.opacity(0.1)))
                            .child(
                                div()
                                    .w(px(10.0))
                                    .h(px(10.0))
                                    .bg(thread_color)
                                    .rounded(px(3.0))
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.sidebar_foreground)
                                    .child(thread.name.clone())
                            )
                    })
                )
        );
    result
}

/// Helper to render an improved stat row with better styling
fn stat_row_improved(
    label: String,
    value: String,
    theme: &ui::theme::Theme,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .px_2()
        .py_1()
        .rounded(px(4.0))
        .hover(|style| style.bg(theme.sidebar_accent.opacity(0.05)))
        .child(
            div()
                .text_xs()
                .text_color(theme.sidebar_foreground.opacity(0.7))
                .child(label)
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.sidebar_foreground)
                .child(value)
        )
}

/// Helper function to create a statistics row (legacy)
fn stat_row(label: impl Into<SharedString>, value: String, label_color: Hsla, value_color: Hsla) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .child(
            div()
                .text_xs()
                .text_color(label_color)
                .child(label.into())
        )
        .child(
            div()
                .text_xs()
                .text_color(value_color)
                .child(value)
        )
}
