//! Framerate graph component showing frame times over the session

use gpui::*;
use gpui::prelude::FluentBuilder;
use std::sync::Arc;
use crate::trace_data::TraceFrame;
use crate::state::ViewState;
use crate::constants::*;
use ui::ActiveTheme;

/// Render the framerate graph at the top of the view
pub fn render_framerate_graph(
    frame: &Arc<TraceFrame>,
    view_state: &ViewState,
    cx: &mut Context<impl Render>,
) -> impl IntoElement {
    let frame_times = frame.frame_times_ms.clone();
    let theme = cx.theme();

    div()
        .id("framerate-graph-container")
        .h(px(GRAPH_HEIGHT))
        .w_full()
        .bg(theme.sidebar.opacity(0.5))
        .border_b_2()
        .border_color(theme.border)
        .relative()
        .when(frame_times.is_empty(), |this| {
            this.flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("No frame timing data")
                )
        })
        .when(!frame_times.is_empty(), |this| {
            let max_time = 33.33; // Cap at 30 FPS (33.33ms)
            
            this
                // Reference lines
                .child(
                    div()
                        .absolute()
                        .top(px(GRAPH_HEIGHT * (1.0 - 16.67 / max_time)))
                        .left_0()
                        .right_0()
                        .h(px(1.0))
                        .bg(hsla(120.0 / 360.0, 0.6, 0.5, 0.3))
                )
                .child(
                    div()
                        .absolute()
                        .top(px(GRAPH_HEIGHT * (1.0 - 33.33 / max_time)))
                        .left_0()
                        .right_0()
                        .h(px(1.0))
                        .bg(hsla(0.0, 0.6, 0.5, 0.3))
                )
                // Frame time bars
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .h_full()
                        .w_full()
                        .items_end()
                        .children(
                            frame_times.iter().map(|&time| {
                                let normalized = (time / max_time).min(1.0);
                                let bar_height = GRAPH_HEIGHT * normalized;
                                
                                let color = if time <= 16.67 {
                                    hsla(120.0 / 360.0, 0.7, 0.5, 0.8) // Green for 60fps+
                                } else if time <= 33.33 {
                                    hsla(60.0 / 360.0, 0.7, 0.5, 0.8) // Yellow for 30-60fps
                                } else {
                                    hsla(0.0, 0.7, 0.5, 0.8) // Red for <30fps
                                };
                                
                                div()
                                    .flex_1()
                                    .h(px(bar_height))
                                    .bg(color)
                            })
                        )
                )
        })
}
