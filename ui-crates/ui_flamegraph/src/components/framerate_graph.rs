//! Framerate graph component showing frame times over the session

use gpui::*;
use std::sync::Arc;
use crate::trace_data::TraceFrame;
use crate::state::ViewState;
use crate::constants::*;
use crate::coordinates::visible_range;
use ui::ActiveTheme;

/// Render the framerate graph at the top of the view
pub fn render_framerate_graph(
    frame: &Arc<TraceFrame>,
    view_state: &ViewState,
    cx: &mut Context<impl Render>,
) -> impl IntoElement {
    let setup_start = std::time::Instant::now();
    let frame_times = frame.frame_times_ms.clone();
    let view_state = view_state.clone();
    let frame_for_indicator = Arc::clone(frame);
    let theme = cx.theme();

    div()
        .h(px(GRAPH_HEIGHT))
        .w_full()
        .bg(theme.list)
        .border_b_1()
        .border_color(theme.border)
        .child(
            canvas(
                move |bounds, _window, _cx| {
                    (bounds, frame_times.clone(), view_state.clone(), Arc::clone(&frame_for_indicator))
                },
                move |bounds, state, window, _cx| {
                    let (bounds, frame_times, view_state, frame) = state;

                    if frame_times.is_empty() {
                        return;
                    }

                    let width: f32 = bounds.size.width.into();
                    let height: f32 = bounds.size.height.into();

                    // Draw background grid
                    window.paint_layer(bounds, |window| {
                        // 60 FPS line
                        let fps_60_y = height * (1.0 - 16.67 / 33.33);
                        let line_bounds = Bounds {
                            origin: point(bounds.origin.x, bounds.origin.y + px(fps_60_y)),
                            size: size(px(width), px(1.0)),
                        };
                        window.paint_quad(fill(line_bounds, hsla(120.0 / 360.0, 0.6, 0.5, 0.3)));

                        // 30 FPS line
                        let fps_30_y = height * (1.0 - 33.33 / 33.33);
                        let line_bounds = Bounds {
                            origin: point(bounds.origin.x, bounds.origin.y + px(fps_30_y)),
                            size: size(px(width), px(1.0)),
                        };
                        window.paint_quad(fill(line_bounds, hsla(0.0, 0.6, 0.5, 0.3)));

                        // Draw frame time graph
                        let point_width = width / frame_times.len() as f32;
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
                                size: size(px(point_width.max(1.0)), px(bar_height)),
                            };
                            window.paint_quad(fill(bar_bounds, color));
                        }

                        // Draw viewport indicator showing visible range in flamegraph
                        if frame.duration_ns() > 0 {
                            let effective_width = width - THREAD_LABEL_WIDTH;
                            
                            // Use the SAME coordinate system as time_to_x
                            // Calculate what times are visible in the flamegraph viewport
                            let zoom = if view_state.zoom == 0.0 {
                                effective_width / frame.duration_ns() as f32
                            } else {
                                view_state.zoom
                            };
                            
                            // Visible time range: from where pan_x = 0 to where pan_x = effective_width
                            // time_to_x formula: (time_offset * zoom) + pan_x + THREAD_LABEL_WIDTH = x
                            // Solve for time when x = THREAD_LABEL_WIDTH (left edge):
                            //   (time_offset * zoom) + pan_x + THREAD_LABEL_WIDTH = THREAD_LABEL_WIDTH
                            //   time_offset * zoom = -pan_x
                            //   time_offset = -pan_x / zoom
                            let visible_start_offset = -view_state.pan_x / zoom;
                            let visible_end_offset = (effective_width - view_state.pan_x) / zoom;
                            
                            let visible_start_ns = (frame.min_time_ns as f64 + visible_start_offset as f64) as u64;
                            let visible_end_ns = (frame.min_time_ns as f64 + visible_end_offset as f64) as u64;
                            
                            // Clamp to frame bounds
                            let visible_start_ns = visible_start_ns.max(frame.min_time_ns).min(frame.min_time_ns + frame.duration_ns());
                            let visible_end_ns = visible_end_ns.max(frame.min_time_ns).min(frame.min_time_ns + frame.duration_ns());
                            
                            // Convert to normalized positions in the framerate graph (0.0 to 1.0)
                            let start_normalized = (visible_start_ns.saturating_sub(frame.min_time_ns)) as f32 / frame.duration_ns() as f32;
                            let end_normalized = (visible_end_ns.saturating_sub(frame.min_time_ns)) as f32 / frame.duration_ns() as f32;
                            
                            // Convert to pixel positions in framerate graph
                            let indicator_start_x = start_normalized * width;
                            let indicator_end_x = end_normalized * width;
                            let indicator_width = (indicator_end_x - indicator_start_x).max(2.0);

                            // Draw semi-transparent overlay for non-visible regions
                            if indicator_start_x > 0.0 {
                                let left_overlay = Bounds {
                                    origin: bounds.origin,
                                    size: size(px(indicator_start_x), px(height)),
                                };
                                window.paint_quad(fill(left_overlay, hsla(0.0, 0.0, 0.0, 0.5)));
                            }

                            if indicator_end_x < width {
                                let right_overlay = Bounds {
                                    origin: point(bounds.origin.x + px(indicator_end_x), bounds.origin.y),
                                    size: size(px(width - indicator_end_x), px(height)),
                                };
                                window.paint_quad(fill(right_overlay, hsla(0.0, 0.0, 0.0, 0.5)));
                            }

                            // Draw border around visible region
                            let border_color = hsla(210.0 / 360.0, 0.8, 0.6, 0.9);
                            let border_width = 2.0;

                            // Top border
                            let top_border = Bounds {
                                origin: point(bounds.origin.x + px(indicator_start_x), bounds.origin.y),
                                size: size(px(indicator_width), px(border_width)),
                            };
                            window.paint_quad(fill(top_border, border_color));

                            // Bottom border
                            let bottom_border = Bounds {
                                origin: point(bounds.origin.x + px(indicator_start_x), bounds.origin.y + px(height - border_width)),
                                size: size(px(indicator_width), px(border_width)),
                            };
                            window.paint_quad(fill(bottom_border, border_color));

                            // Left border
                            let left_border = Bounds {
                                origin: point(bounds.origin.x + px(indicator_start_x), bounds.origin.y),
                                size: size(px(border_width), px(height)),
                            };
                            window.paint_quad(fill(left_border, border_color));

                            // Right border
                            let right_border = Bounds {
                                origin: point(bounds.origin.x + px(indicator_end_x - border_width), bounds.origin.y),
                                size: size(px(border_width), px(height)),
                            };
                            window.paint_quad(fill(right_border, border_color));
                        }

                        // Draw crop selection indicator if dragging
                        if view_state.crop_dragging {
                            if let (Some(start_ns), Some(end_ns)) = 
                                (view_state.crop_start_time_ns, view_state.crop_end_time_ns) {
                                let start = start_ns.min(end_ns);
                                let end = start_ns.max(end_ns);
                                
                                if start >= frame.min_time_ns && end <= frame.max_time_ns {
                                    let normalized_start = (start - frame.min_time_ns) as f32 / 
                                        frame.duration_ns() as f32;
                                    let normalized_end = (end - frame.min_time_ns) as f32 / 
                                        frame.duration_ns() as f32;
                                    
                                    let selection_start_x = normalized_start * width;
                                    let selection_end_x = normalized_end * width;
                                    let selection_width = selection_end_x - selection_start_x;
                                    
                                    // Draw semi-transparent overlay for selection
                                    let selection_bounds = Bounds {
                                        origin: point(bounds.origin.x + px(selection_start_x), bounds.origin.y),
                                        size: size(px(selection_width), px(height)),
                                    };
                                    window.paint_quad(fill(selection_bounds, hsla(210.0 / 360.0, 0.8, 0.5, 0.3)));
                                    
                                    // Draw borders
                                    let border_width = 2.0;
                                    let selection_color = hsla(210.0 / 360.0, 1.0, 0.6, 1.0);
                                    
                                    // Left border
                                    let left_border = Bounds {
                                        origin: point(bounds.origin.x + px(selection_start_x), bounds.origin.y),
                                        size: size(px(border_width), px(height)),
                                    };
                                    window.paint_quad(fill(left_border, selection_color));
                                    
                                    // Right border
                                    let right_border = Bounds {
                                        origin: point(bounds.origin.x + px(selection_end_x - border_width), bounds.origin.y),
                                        size: size(px(border_width), px(height)),
                                    };
                                    window.paint_quad(fill(right_border, selection_color));
                                }
                            }
                        }
                    });
                },
            )
            .size_full()
        )
}
