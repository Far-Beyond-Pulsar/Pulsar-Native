use gpui::*;
use ui::v_flex;
use crate::trace_data::{TraceData, TraceFrame};
use std::ops::Range;
use std::collections::BTreeMap;

const ROW_HEIGHT: f32 = 20.0;
const MIN_SPAN_WIDTH: f32 = 2.0;
const PADDING: f32 = 2.0;
const GRAPH_HEIGHT: f32 = 100.0;
const THREAD_LABEL_WIDTH: f32 = 120.0;
const THREAD_ROW_PADDING: f32 = 30.0;

fn get_palette() -> Vec<Hsla> {
    vec![
        hsla(0.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(20.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(40.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(60.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(120.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(180.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(200.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(220.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(240.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(260.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(280.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(300.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(320.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(340.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(160.0 / 360.0, 0.7, 0.6, 1.0),
        hsla(100.0 / 360.0, 0.7, 0.6, 1.0),
    ]
}

#[derive(Clone)]
struct ViewState {
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
    dragging: bool,
    drag_start_x: f32,
    drag_start_y: f32,
    drag_pan_start_x: f32,
    drag_pan_start_y: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            dragging: false,
            drag_start_x: 0.0,
            drag_start_y: 0.0,
            drag_pan_start_x: 0.0,
            drag_pan_start_y: 0.0,
        }
    }
}

pub struct FlamegraphView {
    trace_data: TraceData,
    view_state: ViewState,
}

impl FlamegraphView {
    pub fn new(trace_data: TraceData) -> Self {
        Self {
            trace_data,
            view_state: ViewState::default(),
        }
    }

    fn calculate_thread_y_offsets(frame: &TraceFrame) -> BTreeMap<u64, f32> {
        let mut offsets = BTreeMap::new();
        let mut current_y = GRAPH_HEIGHT + THREAD_ROW_PADDING;
        
        // Sort threads: GPU (0) first, Main Thread (1) second, then workers
        let mut thread_ids: Vec<u64> = frame.threads.keys().copied().collect();
        thread_ids.sort_by_key(|id| match id {
            0 => (0, *id), // GPU first
            1 => (1, *id), // Main Thread second
            _ => (2, *id), // Workers after
        });
        
        for thread_id in thread_ids {
            // Calculate max depth for this thread
            let max_depth_for_thread = frame.spans
                .iter()
                .filter(|s| s.thread_id == thread_id)
                .map(|s| s.depth)
                .max()
                .unwrap_or(0);
            
            offsets.insert(thread_id, current_y);
            current_y += (max_depth_for_thread + 1) as f32 * ROW_HEIGHT + THREAD_ROW_PADDING;
        }
        
        offsets
    }

    fn time_to_x(time_ns: u64, frame: &TraceFrame, viewport_width: f32, view_state: &ViewState) -> f32 {
        if frame.duration_ns() == 0 {
            return 0.0;
        }
        let normalized = (time_ns - frame.min_time_ns) as f32 / frame.duration_ns() as f32;
        (normalized * (viewport_width - THREAD_LABEL_WIDTH) * view_state.zoom) + view_state.pan_x + THREAD_LABEL_WIDTH
    }

    fn visible_range(frame: &TraceFrame, viewport_width: f32, view_state: &ViewState) -> Range<u64> {
        if frame.duration_ns() == 0 {
            return 0..0;
        }

        let effective_width = viewport_width - THREAD_LABEL_WIDTH;
        
        // Calculate visible time range based on pan and zoom
        // Don't clamp to 0..1 here - allow wider range to prevent aggressive culling
        let inv_zoom = 1.0 / view_state.zoom;
        let normalized_start = (-(view_state.pan_x + effective_width * 0.5)) / (effective_width * view_state.zoom);
        let normalized_end = ((effective_width * 1.5 - view_state.pan_x) / (effective_width * view_state.zoom));

        let start_ns = (frame.min_time_ns as f64 + (normalized_start as f64 * frame.duration_ns() as f64)) as u64;
        let end_ns = (frame.min_time_ns as f64 + (normalized_end as f64 * frame.duration_ns() as f64)) as u64;

        // Allow rendering beyond visible bounds to prevent culling during pan/zoom
        let padding = frame.duration_ns() / 10; // 10% padding on each side
        start_ns.saturating_sub(padding)..end_ns.saturating_add(padding)
    }
    
    fn render_framerate_graph(&self, frame: &TraceFrame, _cx: &mut Context<Self>) -> impl IntoElement {
        let frame_times = frame.frame_times_ms.clone();
        
        div()
            .h(px(GRAPH_HEIGHT))
            .w_full()
            .bg(rgb(0x181818))
            .border_b_1()
            .border_color(rgb(0x3e3e3e))
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        (bounds, frame_times.clone())
                    },
                    move |bounds, state, window, _cx| {
                        let (bounds, frame_times) = state;
                        
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
                        });
                    },
                )
                .size_full()
            )
    }

    fn render_thread_labels(&self, frame: &TraceFrame, _cx: &mut Context<Self>) -> impl IntoElement {
        let thread_offsets = Self::calculate_thread_y_offsets(frame);
        let view_state = self.view_state.clone();

        div()
            .absolute()
            .left_0()
            .top_0()
            .w(px(THREAD_LABEL_WIDTH))
            .h_full()
            .bg(rgb(0x202020))
            .border_r_1()
            .border_color(rgb(0x3e3e3e))
            .overflow_hidden()
            .children(
                thread_offsets.iter().map(|(thread_id, y_offset)| {
                    let thread = frame.threads.get(thread_id).unwrap();
                    let y = y_offset + view_state.pan_y;
                    
                    div()
                        .absolute()
                        .top(px(y))
                        .left_0()
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .flex()
                        .items_center()
                        .px_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if *thread_id == 0 {
                                    rgb(0xff6b6b) // GPU in red
                                } else if *thread_id == 1 {
                                    rgb(0x51cf66) // Main Thread in green
                                } else {
                                    rgb(0x74c0fc) // Workers in blue
                                })
                                .child(thread.name.clone())
                        )
                })
            )
    }
}

impl Render for FlamegraphView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = self.trace_data.get_frame();
        let view_state = self.view_state.clone();
        let palette = get_palette();

        let frame_for_canvas = frame.clone();
        let view_state_for_canvas = view_state.clone();
        let palette_for_canvas = palette.clone();

        v_flex()
            .size_full()
            .bg(rgb(0x1e1e1e))
            .child(
                // Framerate graph at top
                self.render_framerate_graph(&frame, cx)
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .relative()
                    .child(
                        // Thread labels on the left
                        self.render_thread_labels(&frame, cx)
                    )
                    .child(
                        // Main flamegraph canvas
                        canvas(
                            move |bounds, _window, _cx| {
                                let viewport_width: f32 = bounds.size.width.into();
                                let viewport_height: f32 = bounds.size.height.into();
                                (bounds, frame_for_canvas.clone(), view_state_for_canvas.clone(), viewport_width, viewport_height, palette_for_canvas.clone())
                            },
                            move |bounds, state, window, _cx| {
                                let (bounds_prep, frame, view_state, viewport_width, viewport_height, palette) = state;

                                if frame.spans.is_empty() {
                                    return;
                                }

                                let thread_offsets = Self::calculate_thread_y_offsets(&frame);
                                let visible_time = Self::visible_range(&frame, viewport_width, &view_state);

                                window.paint_layer(bounds, |window| {
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

                                    // Draw spans
                                    for span in &frame.spans {
                                        if span.end_ns() < visible_time.start || span.start_ns > visible_time.end {
                                            continue;
                                        }

                                        let thread_y_offset = thread_offsets.get(&span.thread_id).copied().unwrap_or(0.0);
                                        let y = thread_y_offset + (span.depth as f32 * ROW_HEIGHT) + view_state.pan_y;

                                        if y + ROW_HEIGHT < 0.0 || y > viewport_height {
                                            continue;
                                        }

                                        let x1 = Self::time_to_x(span.start_ns, &frame, viewport_width, &view_state);
                                        let x2 = Self::time_to_x(span.end_ns(), &frame, viewport_width, &view_state);
                                        let width = (x2 - x1).max(MIN_SPAN_WIDTH);

                                        // More lenient culling - allow spans that partially overlap viewport
                                        if x2 < THREAD_LABEL_WIDTH - 100.0 || x1 > viewport_width + 100.0 {
                                            continue;
                                        }

                                        let color = palette[(span.color_index as usize) % palette.len()];

                                        let span_bounds = Bounds {
                                            origin: point(
                                                bounds.origin.x + px(x1 + PADDING),
                                                bounds.origin.y + px(y + PADDING)
                                            ),
                                            size: size(
                                                px(width - PADDING * 2.0),
                                                px(ROW_HEIGHT - PADDING * 2.0)
                                            ),
                                        };

                                        window.paint_quad(fill(span_bounds, color));

                                        // Draw text if span is wide enough
                                        if width > 50.0 {
                                            let text_color = if color.l > 0.5 {
                                                hsla(0.0, 0.0, 0.0, 1.0)
                                            } else {
                                                hsla(0.0, 0.0, 1.0, 1.0)
                                            };

                                            // Text rendering would go here - simplified for now
                                        }
                                    }
                                });
                            },
                        )
                        .size_full()
                    )
                    .on_mouse_down(MouseButton::Right, cx.listener(|view, event: &MouseDownEvent, _window, cx| {
                        view.view_state.dragging = true;
                        let pos: Point<Pixels> = event.position;
                        view.view_state.drag_start_x = pos.x.into();
                        view.view_state.drag_start_y = pos.y.into();
                        view.view_state.drag_pan_start_x = view.view_state.pan_x;
                        view.view_state.drag_pan_start_y = view.view_state.pan_y;
                        cx.notify();
                    }))
                    .on_mouse_up(MouseButton::Right, cx.listener(|view, _event: &MouseUpEvent, _window, cx| {
                        view.view_state.dragging = false;
                        cx.notify();
                    }))
                    .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _window, cx| {
                        if view.view_state.dragging {
                            let pos: Point<Pixels> = event.position;
                            let current_x: f32 = pos.x.into();
                            let current_y: f32 = pos.y.into();
                            let delta_x = current_x - view.view_state.drag_start_x;
                            let delta_y = current_y - view.view_state.drag_start_y;
                            
                            view.view_state.pan_x = view.view_state.drag_pan_start_x + delta_x;
                            view.view_state.pan_y = view.view_state.drag_pan_start_y + delta_y;
                            cx.notify();
                        }
                    }))
                    .on_scroll_wheel(cx.listener(|view, event: &ScrollWheelEvent, _window, cx| {
                        let delta = event.delta.pixel_delta(px(1.0));
                        let delta_y: f32 = delta.y.into();
                        let delta_x: f32 = delta.x.into();

                        if event.modifiers.control || event.modifiers.platform {
                            let zoom_factor = 1.0 - (delta_y * 0.01);
                            view.view_state.zoom = (view.view_state.zoom * zoom_factor).clamp(0.1, 100.0);
                        } else if event.modifiers.shift {
                            view.view_state.pan_x -= delta_y;
                        } else {
                            view.view_state.pan_x -= delta_x;
                            view.view_state.pan_y -= delta_y;
                        }

                        cx.notify();
                    }))
            )
            .child(
                // Status bar at bottom
                div()
                    .h(px(40.0))
                    .w_full()
                    .bg(rgb(0x252525))
                    .border_t_1()
                    .border_color(rgb(0x3e3e3e))
                    .flex()
                    .items_center()
                    .px_4()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xcccccc))
                            .child(format!("Spans: {}", frame.spans.len()))
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x999999))
                            .child(format!("Threads: {}", frame.threads.len()))
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x999999))
                            .child(format!("Duration: {:.2}ms", frame.duration_ns() as f64 / 1_000_000.0))
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x999999))
                            .child(format!("Zoom: {:.1}x", view_state.zoom))
                    )
            )
    }
}
