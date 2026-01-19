//! Main flamegraph canvas component with span rendering

use gpui::*;
use std::sync::Arc;
use std::collections::BTreeMap;
use crate::trace_data::TraceFrame;
use crate::state::ViewState;
use crate::constants::*;
use crate::coordinates::{visible_range, time_to_x};

/// Render the main flamegraph canvas with all spans
pub fn render_flamegraph_canvas(
    frame: Arc<TraceFrame>,
    thread_offsets: BTreeMap<u64, f32>,
    view_state: ViewState,
    palette: Vec<Hsla>,
) -> impl IntoElement {
    canvas(
        {
            let frame = Arc::clone(&frame);
            let thread_offsets = thread_offsets.clone();
            move |bounds, _window, _cx| {
                let viewport_width: f32 = bounds.size.width.into();
                let viewport_height: f32 = bounds.size.height.into();
                (bounds, Arc::clone(&frame), thread_offsets.clone(), view_state.clone(), viewport_width, viewport_height, palette.clone())
            }
        },
        move |bounds, state, window, _cx| {
            let (bounds_prep, frame, thread_offsets, view_state, viewport_width, viewport_height, palette) = state;

            if frame.spans.is_empty() {
                return;
            }

            let visible_time = visible_range(&frame, viewport_width, &view_state);

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

                // Group visible spans by (thread_id, depth) for continuous rendering
                let mut span_groups: std::collections::HashMap<(u64, u32), Vec<(f32, f32, usize, f32)>> = std::collections::HashMap::new();

                for (idx, span) in frame.spans.iter().enumerate() {
                    // Calculate Y position first for vertical culling
                    // Note: y is relative to canvas top (0 = top of canvas)
                    let thread_y_offset = thread_offsets.get(&span.thread_id).copied().unwrap_or(0.0);
                    let y = thread_y_offset + (span.depth as f32 * ROW_HEIGHT) + view_state.pan_y;

                    // Vertical culling: skip if completely outside viewport (with padding)
                    // y is in canvas-relative coordinates, viewport_height is the canvas height
                    if y + ROW_HEIGHT < -CULL_PADDING || y > viewport_height + CULL_PADDING {
                        continue;
                    }

                    // Time-based culling: check if span is in visible time range
                    if span.end_ns() < visible_time.start || span.start_ns > visible_time.end {
                        continue;
                    }

                    // Calculate X positions (canvas-relative coordinates)
                    let x1 = time_to_x(span.start_ns, &frame, viewport_width, &view_state);
                    let x2 = time_to_x(span.end_ns(), &frame, viewport_width, &view_state);

                    // Horizontal culling: skip if completely outside viewport (with padding)
                    // x1, x2 are canvas-relative, viewport_width is the canvas width
                    if x2 < -CULL_PADDING || x1 > viewport_width + CULL_PADDING {
                        continue;
                    }

                    // Group visible spans for rendering
                    let key = (span.thread_id, span.depth);
                    span_groups.entry(key).or_insert_with(Vec::new).push((x1, x2, idx, y));
                }

                // Process each group - merge adjacent spans into continuous blocks
                for ((thread_id, depth), mut spans) in span_groups {
                    // Sort by x1 position
                    spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

                    let mut i = 0;
                    while i < spans.len() {
                        let (merge_start, mut merge_end, first_idx, y) = spans[i];
                        let mut j = i + 1;
                        let mut span_count = 1;

                        // Merge adjacent spans only when nearly touching
                        // Very minimal merging to preserve detail
                        while j < spans.len() {
                            let (next_start, next_end, _, _) = spans[j];
                            let gap = next_start - merge_end;

                            // Only merge if gap is less than 1 pixel
                            // No relative threshold - just absolute minimum
                            let should_merge = gap < 1.0;

                            if should_merge {
                                merge_end = next_end;
                                span_count += 1;
                                j += 1;
                            } else {
                                // Gap too large, stop merging
                                break;
                            }
                        }

                        // Calculate width and ensure minimum after padding
                        let total_width = merge_end - merge_start;
                        // CRITICAL: Ensure rendered width is at least MIN_SPAN_WIDTH (2px) AFTER padding
                        let rendered_width = (total_width - PADDING * 2.0).max(MIN_SPAN_WIDTH);

                        // Choose color based on first span
                        let first_span = &frame.spans[first_idx];
                        let base_color = palette[(first_span.color_index as usize) % palette.len()];

                        // Darken slightly if multiple spans merged
                        let color = if span_count > 1 {
                            hsla(
                                base_color.h,
                                base_color.s * 0.9,
                                base_color.l * 0.85,
                                1.0
                            )
                        } else {
                            base_color
                        };

                        // Render the span/block with subtle border
                        let span_bounds = Bounds {
                            origin: point(
                                bounds.origin.x + px(merge_start + PADDING),
                                bounds.origin.y + px(y + PADDING)
                            ),
                            size: size(
                                px(rendered_width),
                                px(ROW_HEIGHT - PADDING * 2.0)
                            ),
                        };

                        // Fill with main color
                        window.paint_quad(fill(span_bounds, color));

                        // Add subtle top border for depth
                        if rendered_width > 4.0 {
                            let highlight_color = hsla(
                                color.h,
                                color.s * 0.7,
                                (color.l * 1.15).min(0.95),
                                0.4
                            );
                            let top_border = Bounds {
                                origin: span_bounds.origin,
                                size: size(px(rendered_width), px(1.0)),
                            };
                            window.paint_quad(fill(top_border, highlight_color));

                            // Add subtle bottom shadow
                            let shadow_color = hsla(0.0, 0.0, 0.0, 0.3);
                            let bottom_border = Bounds {
                                origin: point(
                                    span_bounds.origin.x,
                                    span_bounds.origin.y + span_bounds.size.height - px(1.0)
                                ),
                                size: size(px(rendered_width), px(1.0)),
                            };
                            window.paint_quad(fill(bottom_border, shadow_color));
                        }

                        // Visual indicator if many spans merged
                        if span_count > 5 && total_width > 20.0 {
                            let badge_bounds = Bounds {
                                origin: point(
                                    bounds.origin.x + px(merge_start + total_width - 8.0),
                                    bounds.origin.y + px(y + PADDING)
                                ),
                                size: size(px(6.0), px(6.0)),
                            };
                            window.paint_quad(fill(badge_bounds, hsla(0.0, 0.0, 1.0, 0.3)));
                        }

                        i = j;
                    }
                }
            });
        },
    )
    .size_full()
}
