//! Main flamegraph view orchestration

use crate::trace_data::{TraceData, TraceFrame};
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::sync::Arc;
use ui::v_flex;
use ui::ActiveTheme;

// Import modules
use crate::colors::get_palette;
use crate::components::*;
use crate::constants::*;
use crate::coordinates::time_to_x;
use crate::state::{SpanCache, ViewState};

const SPAN_HOVER_HEIGHT_SCALE: f32 = 0.8;

pub struct FlamegraphView {
    trace_data: TraceData,
    view_state: ViewState,
    cache: Option<(Arc<TraceFrame>, SpanCache)>,
    viewport_width: Arc<std::sync::RwLock<f32>>,
    viewport_height: Arc<std::sync::RwLock<f32>>,
    viewport_origin_x: Arc<std::sync::RwLock<f32>>,
    viewport_origin_y: Arc<std::sync::RwLock<f32>>,
    graph_width: Arc<std::sync::RwLock<f32>>,
    graph_origin_x: Arc<std::sync::RwLock<f32>>,
}

impl FlamegraphView {
    pub fn new(trace_data: TraceData) -> Self {
        Self {
            trace_data,
            view_state: ViewState::default(),
            cache: None,
            viewport_width: Arc::new(std::sync::RwLock::new(1920.0)),
            viewport_height: Arc::new(std::sync::RwLock::new(1080.0)),
            viewport_origin_x: Arc::new(std::sync::RwLock::new(0.0)),
            viewport_origin_y: Arc::new(std::sync::RwLock::new(0.0)),
            graph_width: Arc::new(std::sync::RwLock::new(1920.0)),
            graph_origin_x: Arc::new(std::sync::RwLock::new(0.0)),
        }
    }

    fn graph_x_to_time_ns(&self, frame: &TraceFrame, local_x: f32) -> Option<u64> {
        let duration_ns = frame.duration_ns();
        if duration_ns == 0 {
            return None;
        }

        let width = (*self.graph_width.read().unwrap()).max(1.0);
        let clamped_x = local_x.clamp(0.0, width);
        let ratio = clamped_x / width;
        Some(frame.min_time_ns + (ratio * duration_ns as f32) as u64)
    }

    fn center_bottom_view_on_time(&mut self, frame: &TraceFrame, time_ns: u64) {
        let duration_ns = frame.duration_ns();
        if duration_ns == 0 {
            return;
        }

        let viewport_width = *self.viewport_width.read().unwrap();
        let effective_width = (viewport_width - THREAD_LABEL_WIDTH).max(1.0);
        let zoom = if self.view_state.zoom == 0.0 {
            effective_width / duration_ns as f32
        } else {
            self.view_state.zoom
        };

        let offset_ns = time_ns.saturating_sub(frame.min_time_ns) as f32;
        self.view_state.pan_x = (effective_width * 0.5) - (offset_ns * zoom);
    }

    fn fit_bottom_view_to_segment(&mut self, frame: &TraceFrame, start_ns: u64, end_ns: u64) {
        let duration_ns = frame.duration_ns();
        if duration_ns == 0 {
            return;
        }

        let start = start_ns.min(end_ns).max(frame.min_time_ns);
        let end = end_ns.max(start_ns).min(frame.min_time_ns + duration_ns);
        let selected_duration = end.saturating_sub(start);
        if selected_duration == 0 {
            return;
        }

        let viewport_width = *self.viewport_width.read().unwrap();
        let effective_width = (viewport_width - THREAD_LABEL_WIDTH).max(1.0);
        let new_zoom = effective_width / selected_duration as f32;

        self.view_state.zoom = new_zoom;
        self.view_state.pan_x = -((start - frame.min_time_ns) as f32 * new_zoom);
    }

    fn get_or_build_cache(&mut self) -> (&Arc<TraceFrame>, &SpanCache) {
        let frame = self.trace_data.get_frame();

        // Check if cache is valid (same Arc pointer)
        let needs_rebuild = match &self.cache {
            Some((cached_frame, _)) => !Arc::ptr_eq(cached_frame, &frame),
            None => true,
        };

        if needs_rebuild {
            let cache = SpanCache::build(&frame);

            // Initialize zoom if this is the first frame
            if self.view_state.zoom == 0.0 && frame.duration_ns() > 0 {
                let effective_width = self.view_state.viewport_width - THREAD_LABEL_WIDTH;
                self.view_state.zoom = effective_width / frame.duration_ns() as f32;
                self.view_state.pan_x = 0.0; // Start at left
            }
            // Note: When new data arrives, zoom stays constant (absolute pixels per nanosecond)
            // This prevents the "zooming out" effect as more data comes in

            self.cache = Some((Arc::clone(&frame), cache));
        }

        let (frame_ref, cache_ref) = self
            .cache
            .as_ref()
            .expect("Cache should be populated by get_or_build_cache");
        (frame_ref, cache_ref)
    }
}

impl Render for FlamegraphView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _start = std::time::Instant::now();

        let _cache_start = std::time::Instant::now();
        let (frame, cache) = self.get_or_build_cache();

        let _clone_start = std::time::Instant::now();
        let frame = Arc::clone(frame);
        let thread_offsets = Arc::clone(&cache.thread_offsets);
        let lod_tree = Arc::clone(&cache.lod_tree);
        let tile_cache = Arc::clone(&cache.tile_cache);
        let view_state = self.view_state.clone();
        let palette = get_palette();
        let theme = cx.theme().clone();

        let view_state_for_canvas = view_state.clone();
        let palette_for_canvas = palette.clone();

        let _tr_start = std::time::Instant::now();
        let framerate_graph = render_framerate_graph(&frame, &view_state, cx);
        let timeline_ruler = render_timeline_ruler(&frame, &view_state, cx);

        let result = v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                div()
                    .relative()
                    .h(px(GRAPH_HEIGHT))
                    .w_full()
                    .on_children_prepainted({
                        let graph_width = self.graph_width.clone();
                        let graph_origin_x = self.graph_origin_x.clone();
                        move |bounds: Vec<Bounds<Pixels>>, _window: &mut Window, _cx: &mut App| {
                            if let Some(graph_bounds) = bounds.first() {
                                *graph_width.write().unwrap() = graph_bounds.size.width.into();
                                *graph_origin_x.write().unwrap() = graph_bounds.origin.x.into();
                            }
                        }
                    })
                    .child(framerate_graph)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .w_full()
                            .h_full()
                            .when(self.view_state.crop_dragging, |this| {
                                if let (Some(start), Some(end)) = (
                                    self.view_state.crop_start_time_ns,
                                    self.view_state.crop_end_time_ns,
                                ) {
                                    let duration = frame.duration_ns().max(1) as f32;
                                    let graph_width = *self.graph_width.read().unwrap();
                                    let start_ratio =
                                        (start.min(end).saturating_sub(frame.min_time_ns)) as f32
                                            / duration;
                                    let end_ratio =
                                        (start.max(end).saturating_sub(frame.min_time_ns)) as f32
                                            / duration;
                                    let left = (start_ratio * graph_width).max(0.0);
                                    let width = ((end_ratio - start_ratio) * graph_width).max(1.0);

                                    this.child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .left(px(left))
                                            .h_full()
                                            .w(px(width))
                                            .bg(hsla(205.0 / 360.0, 0.9, 0.74, 0.24))
                                            .border_1()
                                            .border_color(hsla(205.0 / 360.0, 0.85, 0.7, 0.5)),
                                    )
                                } else {
                                    this
                                }
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, event: &MouseDownEvent, _window, cx| {
                                    let window_x: f32 = event.position.x.into();
                                    let local_x = window_x - *view.graph_origin_x.read().unwrap();
                                    let frame = view.trace_data.get_frame();

                                    if let Some(time_ns) = view.graph_x_to_time_ns(&frame, local_x)
                                    {
                                        view.view_state.graph_dragging = true;
                                        view.view_state.graph_drag_start_x = local_x;
                                        view.view_state.crop_dragging = true;
                                        view.view_state.crop_start_time_ns = Some(time_ns);
                                        view.view_state.crop_end_time_ns = Some(time_ns);
                                        cx.notify();
                                    }
                                }),
                            )
                            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _window, cx| {
                                if !view.view_state.graph_dragging {
                                    return;
                                }

                                let window_x: f32 = event.position.x.into();
                                let local_x = window_x - *view.graph_origin_x.read().unwrap();
                                let frame = view.trace_data.get_frame();

                                if let Some(time_ns) = view.graph_x_to_time_ns(&frame, local_x) {
                                    view.view_state.crop_end_time_ns = Some(time_ns);
                                    cx.notify();
                                }
                            }))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|view, event: &MouseUpEvent, _window, cx| {
                                    if !view.view_state.graph_dragging {
                                        return;
                                    }

                                    let window_x: f32 = event.position.x.into();
                                    let local_x = window_x - *view.graph_origin_x.read().unwrap();
                                    let drag_distance =
                                        (local_x - view.view_state.graph_drag_start_x).abs();
                                    let frame = view.trace_data.get_frame();

                                    if drag_distance < 4.0 {
                                        if let Some(center_time) = view
                                            .view_state
                                            .crop_end_time_ns
                                            .or(view.view_state.crop_start_time_ns)
                                        {
                                            view.center_bottom_view_on_time(&frame, center_time);
                                        }
                                    } else if let (Some(start), Some(end)) = (
                                        view.view_state.crop_start_time_ns,
                                        view.view_state.crop_end_time_ns,
                                    ) {
                                        view.fit_bottom_view_to_segment(&frame, start, end);
                                    }

                                    view.view_state.graph_dragging = false;
                                    view.view_state.crop_dragging = false;
                                    view.view_state.crop_start_time_ns = None;
                                    view.view_state.crop_end_time_ns = None;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .child(timeline_ruler)
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .relative()
                    .on_children_prepainted({
                        let viewport_width = self.viewport_width.clone();
                        let viewport_height = self.viewport_height.clone();
                        let viewport_origin_x = self.viewport_origin_x.clone();
                        let viewport_origin_y = self.viewport_origin_y.clone();
                        move |bounds: Vec<Bounds<Pixels>>, _window: &mut Window, _cx: &mut App| {
                            // Store the canvas viewport dimensions
                            if let Some(canvas_bounds) = bounds.first() {
                                *viewport_width.write().unwrap() = canvas_bounds.size.width.into();
                                *viewport_height.write().unwrap() =
                                    canvas_bounds.size.height.into();
                                *viewport_origin_x.write().unwrap() = canvas_bounds.origin.x.into();
                                *viewport_origin_y.write().unwrap() = canvas_bounds.origin.y.into();
                            }
                        }
                    })
                    .child({
                        let _canvas_start = std::time::Instant::now();
                        let frame = Arc::clone(&frame);

                        // Track viewport width in view_state
                        let width = *self.viewport_width.read().unwrap();
                        self.view_state.viewport_width = width;

                        let (_, _cache) = self.get_or_build_cache();

                        render_flamegraph_canvas(
                            Arc::clone(&frame),
                            Arc::clone(&lod_tree),
                            Arc::clone(&thread_offsets),
                            Arc::clone(&tile_cache),
                            view_state_for_canvas.clone(),
                            palette_for_canvas.clone(),
                        )
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|view, event: &MouseDownEvent, _window, cx| {
                            view.view_state.dragging = true;
                            let pos: Point<Pixels> = event.position;
                            view.view_state.drag_start_x = pos.x.into();
                            view.view_state.drag_start_y = pos.y.into();
                            view.view_state.drag_pan_start_x = view.view_state.pan_x;
                            view.view_state.drag_pan_start_y = view.view_state.pan_y;
                            cx.notify();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(|view, _event: &MouseUpEvent, _window, cx| {
                            view.view_state.dragging = false;
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _window, cx| {
                        let pos: Point<Pixels> = event.position;
                        let window_x: f32 = pos.x.into();
                        let window_y: f32 = pos.y.into();
                        let local_x = window_x - *view.viewport_origin_x.read().unwrap();
                        let local_y = window_y - *view.viewport_origin_y.read().unwrap();

                        // Store mouse in canvas-local coordinates for hover popup and hit tests.
                        view.view_state.mouse_x = local_x;
                        view.view_state.mouse_y = local_y;

                        if view.view_state.dragging {
                            // Drag deltas stay in window space; origin cancels out.
                            let delta_x = window_x - view.view_state.drag_start_x;
                            let delta_y = window_y - view.view_state.drag_start_y;

                            view.view_state.pan_x = view.view_state.drag_pan_start_x + delta_x;
                            view.view_state.pan_y = view.view_state.drag_pan_start_y + delta_y;
                        } else {
                            // Detect hovered span
                            // Copy view_state values before borrowing
                            let view_state_copy = view.view_state.clone();
                            let viewport_width = *view.viewport_width.read().unwrap();
                            let viewport_height = *view.viewport_height.read().unwrap();
                            let (frame, cache) = view.get_or_build_cache();

                            let mut new_hovered_span = None;

                            // Only check if mouse is within the canvas area
                            if local_x >= THREAD_LABEL_WIDTH
                                && local_x <= viewport_width
                                && local_y >= 0.0
                                && local_y <= viewport_height
                            {
                                for (idx, span) in frame.spans.iter().enumerate() {
                                    let thread_y_offset = cache
                                        .thread_offsets
                                        .get(&span.thread_id)
                                        .copied()
                                        .unwrap_or(0.0);
                                    let y = thread_y_offset
                                        + (span.depth as f32 * ROW_HEIGHT)
                                        + view_state_copy.pan_y;

                                    if local_y >= y
                                        && local_y
                                            <= y + ((ROW_HEIGHT - PADDING) * SPAN_HOVER_HEIGHT_SCALE)
                                    {
                                        let x1 = time_to_x(
                                            span.start_ns,
                                            frame,
                                            viewport_width,
                                            &view_state_copy,
                                        );
                                        let x2 = time_to_x(
                                            span.end_ns(),
                                            frame,
                                            viewport_width,
                                            &view_state_copy,
                                        );

                                        if local_x >= x1 && local_x <= x2 {
                                            new_hovered_span = Some(idx);
                                            break;
                                        }
                                    }
                                }
                            }

                            view.view_state.hovered_span = new_hovered_span;
                        }

                        cx.notify();
                    }))
                    .on_scroll_wheel(cx.listener(|view, event: &ScrollWheelEvent, _window, cx| {
                        let delta = event.delta.pixel_delta(px(1.0));
                        let delta_y: f32 = delta.y.into();
                        let _delta_x: f32 = delta.x.into();

                        if event.modifiers.control || event.modifiers.platform {
                            // Zoom around cursor position (horizontal only)
                            let cursor_pos: Point<Pixels> = event.position;
                            let cursor_x: f32 = cursor_pos.x.into();
                            let local_cursor_x = cursor_x - *view.viewport_origin_x.read().unwrap();

                            let old_zoom = view.view_state.zoom;
                            let zoom_factor = 1.0 - (delta_y * 0.01);
                            let new_zoom = old_zoom * zoom_factor;

                            // Calculate world position under cursor before zoom (X only)
                            let world_x = (local_cursor_x - view.view_state.pan_x) / old_zoom;

                            // Update zoom
                            view.view_state.zoom = new_zoom;

                            // Adjust pan_x so the same world position stays under cursor
                            // Keep pan_y unchanged
                            view.view_state.pan_x = local_cursor_x - (world_x * new_zoom);
                        } else if event.modifiers.shift {
                            // Vertical panning with shift
                            view.view_state.pan_y -= delta_y * 10.0;
                        } else {
                            // Horizontal panning (default)
                            view.view_state.pan_x -= delta_y * 10.0;
                        }

                        cx.notify();
                    }))
                    .child({
                        let _tl_start = std::time::Instant::now();

                        render_thread_labels(&frame, &thread_offsets, &view_state, cx)
                    })
                    .children({
                        let _hp_start = std::time::Instant::now();
                        let popup = render_hover_popup(
                            &frame,
                            &view_state,
                            *self.viewport_width.read().unwrap(),
                            cx,
                        );
                        popup
                    }),
            );

        result
    }
}
