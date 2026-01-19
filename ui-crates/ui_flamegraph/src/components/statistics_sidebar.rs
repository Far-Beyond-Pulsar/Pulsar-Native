//! Statistics sidebar component showing session statistics

use gpui::*;
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

    div()
        .absolute()
        .right_0()
        .top_0()
        .w(px(STATS_SIDEBAR_WIDTH))
        .h_full()
        .bg(theme.popover)
        .border_l_1()
        .border_color(theme.border)
        .flex()
        .flex_col()
        .p_4()
        .gap_3()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.foreground)
                .child("Statistics")
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(stat_row("Total Spans:", format!("{}", frame.spans.len()), theme.muted_foreground, theme.foreground))
                .child(stat_row("Duration:", format!("{:.2} ms", duration_ms), theme.muted_foreground, theme.foreground))
                .child(stat_row("Frames:", format!("{}", num_frames), theme.muted_foreground, theme.foreground))
                .child(stat_row("Avg Frame:", format!("{:.2} ms", avg_frame_time), theme.muted_foreground, theme.foreground))
                .child(stat_row("Min Frame:", format!("{:.2} ms", min_frame_time), theme.muted_foreground, theme.foreground))
                .child(stat_row("Max Frame:", format!("{:.2} ms", max_frame_time), theme.muted_foreground, theme.foreground))
                .child(stat_row("Threads:", format!("{}", frame.threads.len()), theme.muted_foreground, theme.foreground))
                .child(stat_row("Max Depth:", format!("{}", frame.max_depth), theme.muted_foreground, theme.foreground))
        )
        .child(
            div()
                .mt_4()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.foreground)
                .child("Threads")
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .children(
                    frame.threads.values().take(10).map(|thread| {
                        let thread_color = get_thread_color(thread.id);

                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(8.0))
                                    .h(px(8.0))
                                    .bg(thread_color)
                                    .rounded(px(2.0))
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.foreground)
                                    .child(thread.name.clone())
                            )
                    })
                )
        )
}

/// Helper function to create a statistics row
fn stat_row(label: &str, value: String, label_color: Hsla, value_color: Hsla) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .child(
            div()
                .text_xs()
                .text_color(label_color)
                .child(label)
        )
        .child(
            div()
                .text_xs()
                .text_color(value_color)
                .child(value)
        )
}
