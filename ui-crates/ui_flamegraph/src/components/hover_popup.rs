//! Hover popup component showing span details

use gpui::*;
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
    let thread_name = frame.threads.get(&span.thread_id).map(|t| t.name.clone()).unwrap_or_else(|| "Unknown".to_string());

    let popup_width = 280.0;
    let mouse_x = view_state.mouse_x;
    let mouse_y = view_state.mouse_y;

    // Position popup horizontally near the mouse cursor
    let popup_x = if mouse_x + popup_width + 20.0 > viewport_width - STATS_SIDEBAR_WIDTH {
        (mouse_x - popup_width - 10.0).max(0.0)
    } else {
        mouse_x + 15.0
    };

    // Mouse Y is already relative to the canvas div (where the popup is also rendered)
    // So no offset needed - just position slightly below the cursor
    let popup_y = mouse_y + 5.0 - 150.0;
    println!("[HP] setup calculations: {:?}", setup_start.elapsed());

    let render_start = std::time::Instant::now();
    let result = Some(
        div()
            .absolute()
            .left(px(popup_x))
            .top(px(popup_y))
            .w(px(popup_width))
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .rounded(px(4.0))
            .shadow_lg()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.foreground)
                    .child(span.name.clone())
            )
            .child(
                div()
                    .w_full()
                    .h(px(1.0))
                    .bg(theme.border)
            )
            .child(popup_row("Duration:", format!("{:.3} ms", duration_ms), theme.muted_foreground, theme.foreground, true))
            .child(popup_row("Start:", format!("{:.3} ms", start_ms), theme.muted_foreground, theme.foreground, false))
            .child(popup_row("End:", format!("{:.3} ms", end_ms), theme.muted_foreground, theme.foreground, false))
            .child(popup_row("Thread:", thread_name, theme.muted_foreground, theme.foreground, false))
            .child(popup_row("Depth:", format!("{}", span.depth), theme.muted_foreground, theme.foreground, false))
    );
    println!("[HP] render popup: {:?}", render_start.elapsed());
    result
}

/// Helper function to create a popup info row
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
