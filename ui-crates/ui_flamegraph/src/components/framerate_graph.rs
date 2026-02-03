//! Framerate graph component showing frame times over the session

use gpui::*;
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

    // If no frame times, show empty graph
    if frame_times.is_empty() {
        return div()
            .h(px(GRAPH_HEIGHT))
            .w_full()
            .bg(theme.sidebar.opacity(0.5))
            .border_b_2()
            .border_color(theme.border)
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("No frame timing data")
            );
    }

    div()
        .h(px(GRAPH_HEIGHT))
        .w_full()
        .bg(theme.sidebar.opacity(0.5))
        .border_b_2()
        .border_color(theme.border)
        .child(
            canvas(
                move |bounds, _window, _cx| bounds,
                {
                    let frame_times = frame_times.clone();
                    let theme = theme.clone();
                    move |bounds, _state, window, _cx| {
                        let width: f32 = bounds.size.width.into();
                        let height: f32 = bounds.size.height.into();

                        if width <= 0.0 || height <= 0.0 {
                            return;
                        }

                        window.paint_layer(bounds, |window| {
                            // Draw background
                            window.paint_quad(fill(bounds, theme.sidebar.opacity(0.5)));

                            // 60 FPS line (16.67ms)
                            let fps_60_y = height * (1.0 - 16.67 / 33.33);
                            let line_bounds = Bounds {
                                origin: point(bounds.origin.x, bounds.origin.y + px(fps_60_y)),
                                size: size(px(width), px(1.0)),
                            };
                            window.paint_quad(fill(line_bounds, hsla(120.0 / 360.0, 0.6, 0.5, 0.3)));

                            // 30 FPS line (33.33ms)
                            let fps_30_y = height * (1.0 - 33.33 / 33.33);
                            let line_bounds = Bounds {
                                origin: point(bounds.origin.x, bounds.origin.y + px(fps_30_y)),
                                size: size(px(width), px(1.0)),
                            };
                            window.paint_quad(fill(line_bounds, hsla(0.0, 0.6, 0.5, 0.3)));

                            // Draw frame time bars
                            let point_width = (width / frame_times.len() as f32).max(1.0);
                            for (i, &frame_time) in frame_times.iter().enumerate() {
                                let x = i as f32 * point_width;
                                let normalized_height = (frame_time / 33.33).min(1.0);
                                let bar_height = height * normalized_height;

                                let color = if frame_time <= 16.67 {
                                    hsla(120.0 / 360.0, 0.7, 0.5, 0.8) // Green for 60fps+
                                } else if frame_time <= 33.33 {
                                    hsla(60.0 / 360.0, 0.7, 0.5, 0.8) // Yellow for 30-60fps
                                } else {
                                    hsla(0.0, 0.7, 0.5, 0.8) // Red for <30fps
                                };

                                let bar_bounds = Bounds {
                                    origin: point(
                                        bounds.origin.x + px(x),
                                        bounds.origin.y + px(height - bar_height)
                                    ),
                                    size: size(px(point_width), px(bar_height)),
                                };
                                window.paint_quad(fill(bar_bounds, color));
                            }
                        });
                    }
                },
            )
            .size_full()
        )
}
