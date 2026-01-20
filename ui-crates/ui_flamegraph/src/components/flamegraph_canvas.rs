//! Main flamegraph canvas component with span rendering

use gpui::*;
use std::sync::Arc;
use std::collections::BTreeMap;
use crate::trace_data::TraceFrame;
use crate::state::ViewState;
use crate::lod_tree::LODTree;
use crate::constants::*;
use crate::coordinates::{visible_range, time_to_x};
use crate::colors::get_palette;

/// Render the main flamegraph canvas with all spans
pub fn render_flamegraph_canvas(
    frame: Arc<TraceFrame>,
    lod_tree: Arc<LODTree>,
    thread_offsets: Arc<BTreeMap<u64, f32>>,
    view_state: ViewState,
    palette: Vec<Hsla>,
) -> impl IntoElement {
    let setup_start = std::time::Instant::now();
    canvas(
        {
            let clone_start = std::time::Instant::now();
            let frame = Arc::clone(&frame);
            let lod_tree = Arc::clone(&lod_tree);
            let thread_offsets = Arc::clone(&thread_offsets);
            move |bounds, _window, _cx| {
                let closure_start = std::time::Instant::now();
                let viewport_width: f32 = bounds.size.width.into();
                let viewport_height: f32 = bounds.size.height.into();
                let result = (bounds, Arc::clone(&frame), Arc::clone(&lod_tree), Arc::clone(&thread_offsets), view_state.clone(), viewport_width, viewport_height, palette.clone());
                result
            }
        },
        move |bounds, state, window, _cx| {
            let paint_start = std::time::Instant::now();
            let (bounds_prep, frame, lod_tree, thread_offsets, view_state, viewport_width, viewport_height, palette) = state;

            if frame.spans.is_empty() {
                return;
            }

            let visible_range_start = std::time::Instant::now();
            let visible_time = visible_range(&frame, viewport_width, &view_state);

            let paint_layer_start = std::time::Instant::now();
            window.paint_layer(bounds, |window| {
                // Draw vertical grid lines aligned with timeline
                let visible_range_for_grid = visible_range(&frame, viewport_width, &view_state);
                let visible_duration = visible_range_for_grid.end.saturating_sub(visible_range_for_grid.start);
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
                let first_marker = (visible_range_for_grid.start / marker_interval_ns) * marker_interval_ns;
                let mut current_time = first_marker;

                while current_time <= visible_range_for_grid.end {
                    if current_time >= frame.min_time_ns {
                        let x = time_to_x(current_time, &frame, viewport_width, &view_state);

                        if x >= THREAD_LABEL_WIDTH && x <= viewport_width {
                            let grid_bounds = Bounds {
                                origin: point(bounds.origin.x + px(x), bounds.origin.y),
                                size: size(px(1.0), px(viewport_height)),
                            };
                            window.paint_quad(fill(grid_bounds, hsla(0.0, 0.0, 0.25, 0.15)));
                        }
                    }
                    current_time += marker_interval_ns;
                }

                // Draw thread separators
                for (idx, (thread_id, y_offset)) in thread_offsets.iter().enumerate() {
                    if idx > 0 {
                        let separator_y = y_offset - THREAD_ROW_PADDING / 2.0 + view_state.pan_y;
                        if separator_y >= 0.0 && separator_y < viewport_height {
                            let separator_bounds = Bounds {
                                origin: point(
                                    bounds.origin.x + px(THREAD_LABEL_WIDTH),
                                    bounds.origin.y + px(separator_y)
                                ),
                                size: size(
                                    px(viewport_width - THREAD_LABEL_WIDTH),
                                    px(1.0)
                                ),
                            };
                            window.paint_quad(fill(separator_bounds, hsla(0.0, 0.0, 0.3, 0.3)));
                        }
                    }
                }

                // ========================================================================
                // OFFSCREEN CULLING with careful coordinate handling
                // Cull spans outside viewport to improve performance
                // Uses padding to prevent edge popping during pan/zoom
                // ========================================================================

                // LOD QUERY: Get pre-merged spans at appropriate detail level
                // O(output) complexity - independent of total dataset size!
                let lod_start = std::time::Instant::now();

                let vertical_min = -CULL_PADDING - view_state.pan_y;
                let vertical_max = viewport_height + CULL_PADDING - view_state.pan_y;

                // Debug: Print visible range to understand culling
                static mut FRAME_COUNT: u32 = 0;
                unsafe {
                    FRAME_COUNT += 1;
                    if FRAME_COUNT % 60 == 0 {  // Log every 60 frames
                        let duration_ms = (visible_time.end - visible_time.start) as f64 / 1_000_000.0;
                        println!("[FLAMEGRAPH] Visible range: {} - {} ({:.2}ms), pan_x: {:.1}, zoom: {:.8}", 
                            visible_time.start, visible_time.end, duration_ms, view_state.pan_x, view_state.zoom);
                    }
                }

                let merged_spans = lod_tree.query_dynamic(
                    visible_time.start,
                    visible_time.end,
                    vertical_min,
                    vertical_max,
                    viewport_width - THREAD_LABEL_WIDTH,
                );

                unsafe {
                    if FRAME_COUNT % 60 == 0 {
                        println!("[FLAMEGRAPH] LOD returned {} spans", merged_spans.len());
                    }
                }

                // Paint pre-merged spans directly - NO additional merging needed!
                let paint_start = std::time::Instant::now();
                let palette = get_palette();

                for merged_span in merged_spans {
                    let x1 = time_to_x(merged_span.start_ns, &frame, viewport_width, &view_state);
                    let x2 = time_to_x(merged_span.end_ns, &frame, viewport_width, &view_state);
                    let width = x2 - x1;

                    // NEVER skip - always draw something, even if tiny
                    // Minimum 1 pixel so spans don't disappear when zoomed out
                    let rendered_width = if width < 1.0 {
                        1.0  // Draw as 1px line when zoomed way out
                    } else if width < 3.0 {
                        width  // Draw actual width for small spans
                    } else {
                        (width - PADDING * 2.0).max(MIN_SPAN_WIDTH)  // Normal padding for larger spans
                    };

                    let y = merged_span.y + view_state.pan_y;

                    let base_color = palette[merged_span.color_index % palette.len()];

                    // Darken if multiple spans merged - visual feedback for LOD
                    let color = if merged_span.span_count > 1 {
                        hsla(base_color.h, base_color.s * 0.9, base_color.l * 0.85, 1.0)
                    } else {
                        base_color
                    };

                    let span_bounds = Bounds {
                        origin: point(
                            bounds.origin.x + px(x1 + if width < 3.0 { 0.0 } else { PADDING }),
                            bounds.origin.y + px(y + PADDING)
                        ),
                        size: size(px(rendered_width), px(ROW_HEIGHT - PADDING * 2.0)),
                    };

                    window.paint_quad(fill(span_bounds, color));

                    // Add borders for larger spans only
                    if rendered_width > 4.0 {
                        let highlight_color = hsla(color.h, color.s * 0.7, (color.l * 1.15).min(0.95), 0.4);
                        let top_border = Bounds {
                            origin: span_bounds.origin,
                            size: size(px(rendered_width), px(1.0)),
                        };
                        window.paint_quad(fill(top_border, highlight_color));

                        let shadow_color = hsla(0.0, 0.0, 0.0, 0.3);
                        let bottom_border = Bounds {
                            origin: point(span_bounds.origin.x, span_bounds.origin.y + span_bounds.size.height - px(1.0)),
                            size: size(px(rendered_width), px(1.0)),
                        };
                        window.paint_quad(fill(bottom_border, shadow_color));
                    }

                    // Badge for heavily merged spans
                    if merged_span.span_count > 5 && width > 20.0 {
                        let badge_bounds = Bounds {
                            origin: point(bounds.origin.x + px(x1 + width - 8.0), bounds.origin.y + px(y + PADDING)),
                            size: size(px(6.0), px(6.0)),
                        };
                        window.paint_quad(fill(badge_bounds, hsla(0.0, 0.0, 1.0, 0.3)));
                    }
                }
            });
        },
    )
    .size_full()
}
