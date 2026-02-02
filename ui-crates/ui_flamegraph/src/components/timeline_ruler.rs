//! Timeline ruler component showing time markers

use gpui::*;
use std::sync::Arc;
use crate::trace_data::TraceFrame;
use crate::state::ViewState;
use crate::constants::*;
use crate::coordinates::{visible_range, time_to_x};
use ui::ActiveTheme;

/// Render the timeline ruler with time markers
pub fn render_timeline_ruler(
    frame: &Arc<TraceFrame>,
    view_state: &ViewState,
    cx: &mut Context<impl Render>,
) -> impl IntoElement {
    let setup_start = std::time::Instant::now();
    let frame_for_canvas = Arc::clone(frame);
    let view_state = view_state.clone();
    let theme = cx.theme();

    div()
        .h(px(TIMELINE_HEIGHT))
        .w_full()
        .bg(theme.sidebar.opacity(0.3))
        .border_b_1()
        .border_color(theme.border)
        .child(
            canvas(
                move |bounds, _window, _cx| {
                    let viewport_width: f32 = bounds.size.width.into();
                    (bounds, Arc::clone(&frame_for_canvas), view_state.clone(), viewport_width)
                },
                move |bounds, state, window, _cx| {
                    let paint_start = std::time::Instant::now();
                    let (bounds, frame, view_state, viewport_width) = state;

                    if frame.duration_ns() == 0 {
                        return;
                    }

                    let effective_width = viewport_width - THREAD_LABEL_WIDTH;

                    let paint_layer_start = std::time::Instant::now();
                    window.paint_layer(bounds, |window| {
                        // Calculate visible time range
                        let visible_range_val = visible_range(&frame, viewport_width, &view_state);
                        let visible_duration = visible_range_val.end.saturating_sub(visible_range_val.start);

                        // Determine appropriate time interval for markers
                        let visible_ms = visible_duration as f64 / 1_000_000.0;
                        let marker_interval_ms = if visible_ms < 10.0 {
                            1.0
                        } else if visible_ms < 50.0 {
                            5.0
                        } else if visible_ms < 100.0 {
                            10.0
                        } else if visible_ms < 500.0 {
                            50.0
                        } else {
                            100.0
                        };

                        let marker_interval_ns = (marker_interval_ms * 1_000_000.0) as u64;

                        // Draw time markers
                        let first_marker = (visible_range_val.start / marker_interval_ns) * marker_interval_ns;
                        let mut current_time = first_marker;

                        while current_time <= visible_range_val.end {
                            if current_time >= frame.min_time_ns {
                                let x = time_to_x(current_time, &frame, viewport_width, &view_state);

                                if x >= THREAD_LABEL_WIDTH && x <= viewport_width {
                                    // Draw main tick mark (taller)
                                    let tick_bounds = Bounds {
                                        origin: point(bounds.origin.x + px(x), bounds.origin.y + px(TIMELINE_HEIGHT - 8.0)),
                                        size: size(px(1.0), px(8.0)),
                                    };
                                    window.paint_quad(fill(tick_bounds, hsla(0.0, 0.0, 0.5, 0.6)));

                                    // Draw time label
                                    let time_ms = (current_time - frame.min_time_ns) as f64 / 1_000_000.0;
                                    let _label = format!("{:.1}ms", time_ms);

                                    // TODO: Add text rendering when GPUI text API is available
                                    // For now, just draw the tick marks
                                }
                            }
                            current_time += marker_interval_ns;
                        }
                        
                        // Draw minor tick marks (between major markers)
                        let minor_interval_ns = marker_interval_ns / 5;
                        let first_minor = (visible_range_val.start / minor_interval_ns) * minor_interval_ns;
                        let mut current_minor_time = first_minor;
                        
                        while current_minor_time <= visible_range_val.end {
                            if current_minor_time >= frame.min_time_ns && current_minor_time % marker_interval_ns != 0 {
                                let x = time_to_x(current_minor_time, &frame, viewport_width, &view_state);
                                
                                if x >= THREAD_LABEL_WIDTH && x <= viewport_width {
                                    // Draw minor tick mark (shorter and more transparent)
                                    let tick_bounds = Bounds {
                                        origin: point(bounds.origin.x + px(x), bounds.origin.y + px(TIMELINE_HEIGHT - 4.0)),
                                        size: size(px(1.0), px(4.0)),
                                    };
                                    window.paint_quad(fill(tick_bounds, hsla(0.0, 0.0, 0.5, 0.3)));
                                }
                            }
                            current_minor_time += minor_interval_ns;
                        }
                    });
                },
            )
            .size_full()
        )
}
