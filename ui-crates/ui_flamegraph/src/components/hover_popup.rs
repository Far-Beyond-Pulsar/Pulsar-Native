//! Hover popup component showing span details

use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use crate::trace_data::TraceFrame;
use crate::state::ViewState;
use crate::constants::STATS_SIDEBAR_WIDTH;
use ui::ActiveTheme;

/// Render the hover popup when a span is hovered
pub fn render_hover_popup(
    frame: &Arc<TraceFrame>,
    view_state: &ViewState,
    viewport_width: f32,
    cx: &mut Context<impl Render>,
) -> Option<impl IntoElement> {
    let setup_start = std::time::Instant::now();
    let span_idx = view_state.hovered_span?;
    let span = frame.spans.get(span_idx)?;
    let theme = cx.theme();

    let duration_ms = span.duration_ns as f64 / 1_000_000.0;
    let start_ms = (span.start_ns - frame.min_time_ns) as f64 / 1_000_000.0;
    let end_ms = (span.end_ns() - frame.min_time_ns) as f64 / 1_000_000.0;
    let thread_name = frame.threads.get(&span.thread_id).map(|t| t.name.clone()).unwrap_or_else(|| t!("Flamegraph.Unknown").to_string());

    let popup_width = 300.0;
    let mouse_x = view_state.mouse_x;
    let mouse_y = view_state.mouse_y;

    // Position popup horizontally near the mouse cursor
    let popup_x = if mouse_x + popup_width + 20.0 > viewport_width - STATS_SIDEBAR_WIDTH {
        (mouse_x - popup_width - 10.0).max(0.0)
    } else {
        mouse_x + 15.0
    };

    // Position popup vertically - mouse_y is window-relative
    // Need to subtract timeline height to get canvas-relative position
    let popup_y = (mouse_y - 210.0 + 5.0).max(0.0);

    let render_start = std::time::Instant::now();
    let result = Some(
        div()
            .absolute()
            .left(px(popup_x))
            .top(px(popup_y))
            .w(px(popup_width))
            .bg(theme.popover)
            .border_2()
            .border_color(theme.border.opacity(0.5))
            .rounded(px(8.0))
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                // Header
                div()
                    .px_4()
                    .py_3()
                    .bg(theme.accent.opacity(0.1))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.foreground)
                            .child(span.name.clone())
                    )
            )
            .child(
                // Content
                div()
                    .px_4()
                    .py_3()
                    .flex()
                    .flex_col()
                    .gap_2p5()
                    .child(popup_row_improved(t!("Flamegraph.Duration").to_string(), format!("{:.3} ms", duration_ms), theme, true))
                    .child(popup_row_improved(t!("Flamegraph.Start").to_string(), format!("{:.3} ms", start_ms), theme, false))
                    .child(popup_row_improved(t!("Flamegraph.End").to_string(), format!("{:.3} ms", end_ms), theme, false))
                    .child(
                        div()
                            .h(px(1.0))
                            .w_full()
                            .bg(theme.border.opacity(0.3))
                    )
                    .child(popup_row_improved(t!("Flamegraph.Thread").to_string(), thread_name, theme, false))
                    .child(popup_row_improved(t!("Flamegraph.Depth").to_string(), format!("{}", span.depth), theme, false))
            )
    );
    result
}

/// Helper function to create an improved popup info row
fn popup_row_improved(label: String, value: String, theme: &ui::theme::Theme, bold_value: bool) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .px_2()
        .py_1()
        .rounded(px(4.0))
        .hover(|style| style.bg(theme.accent.opacity(0.05)))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label)
        )
        .child(
            if bold_value {
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.accent)
                    .child(value)
            } else {
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(value)
            }
        )
}

/// Helper function to create a popup info row (legacy)
fn popup_row(label: impl Into<SharedString>, value: String, label_color: Hsla, value_color: Hsla, bold_value: bool) -> impl IntoElement {
    let value_div = if bold_value {
        div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(value_color)
            .child(value)
    } else {
        div()
            .text_xs()
            .text_color(value_color)
            .child(value)
    };

    div()
        .flex()
        .justify_between()
        .child(
            div()
                .text_xs()
                .text_color(label_color)
                .child(label.into())
        )
        .child(value_div)
}
