//! Main flamegraph view orchestration

use gpui::*;
use ui::v_flex;
use crate::trace_data::{TraceData, TraceFrame};
use std::sync::Arc;
use ui::ActiveTheme;

// Import modules
use crate::constants::*;
use crate::colors::get_palette;
use crate::state::{ViewState, SpanCache};
use crate::coordinates::{time_to_x, visible_range};
use crate::components::*;

pub struct FlamegraphView {
    trace_data: TraceData,
    view_state: ViewState,
    cache: Option<(Arc<TraceFrame>, SpanCache)>,
    viewport_width: Arc<std::sync::RwLock<f32>>,
    viewport_height: Arc<std::sync::RwLock<f32>>,
}

impl FlamegraphView {
    pub fn new(trace_data: TraceData) -> Self {
        Self {
            trace_data,
            view_state: ViewState::default(),
            cache: None,
            viewport_width: Arc::new(std::sync::RwLock::new(1920.0)),
            viewport_height: Arc::new(std::sync::RwLock::new(1080.0)),
        }
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

        let (frame_ref, cache_ref) = self.cache.as_ref().unwrap();
        (frame_ref, cache_ref)
    }
}

impl Render for FlamegraphView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let start = std::time::Instant::now();

        let cache_start = std::time::Instant::now();
        let (frame, cache) = self.get_or_build_cache();

        let clone_start = std::time::Instant::now();
        let frame = Arc::clone(frame);
        let thread_offsets = Arc::clone(&cache.thread_offsets);
        let lod_tree = Arc::clone(&cache.lod_tree);
        let view_state = self.view_state.clone();
        let palette = get_palette();
        let theme = cx.theme().clone();

        let view_state_for_canvas = view_state.clone();
        let palette_for_canvas = palette.clone();

        let tr_start = std::time::Instant::now();
        let timeline_ruler = render_timeline_ruler(&frame, &view_state, cx);

        let result = v_flex()
            .size_full()
            .bg(theme.background)
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
                        move |bounds: Vec<Bounds<Pixels>>, _window: &mut Window, _cx: &mut App| {
                            // Store the canvas viewport dimensions
                            if let Some(canvas_bounds) = bounds.first() {
                                *viewport_width.write().unwrap() = canvas_bounds.size.width.into();
                                *viewport_height.write().unwrap() = canvas_bounds.size.height.into();
                            }
                        }
                    })
                    .child({
                        let canvas_start = std::time::Instant::now();
                        let frame = Arc::clone(&frame);
                        
                        // Track viewport width in view_state
                        let width = *self.viewport_width.read().unwrap();
                        self.view_state.viewport_width = width;
                        
                        let (_, cache) = self.get_or_build_cache();
                        let canvas = render_flamegraph_canvas(
                            Arc::clone(&frame),
                            Arc::clone(&lod_tree),
                            Arc::clone(&thread_offsets),
                            view_state_for_canvas.clone(),
                            palette_for_canvas.clone()
                        );
                        canvas
                    })
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
                        let pos: Point<Pixels> = event.position;
                        let current_x: f32 = pos.x.into();
                        let current_y: f32 = pos.y.into();

                        view.view_state.mouse_x = current_x;
                        view.view_state.mouse_y = current_y;

                        if view.view_state.dragging {
                            let delta_x = current_x - view.view_state.drag_start_x;
                            let delta_y = current_y - view.view_state.drag_start_y;

                            view.view_state.pan_x = view.view_state.drag_pan_start_x + delta_x;
                            view.view_state.pan_y = view.view_state.drag_pan_start_y + delta_y;
                        } else {
                            // Detect hovered span
                            // Mouse position is window-relative, need to account for timeline at top
                            // Using hardcoded value to verify it's being used
                            let canvas_offset_y = 125.0; // TIMELINE_HEIGHT
                            let canvas_y = current_y - canvas_offset_y;

                            // Copy view_state values before borrowing
                            let view_state_copy = view.view_state.clone();
                            let viewport_width = *view.viewport_width.read().unwrap();
                            let (frame, cache) = view.get_or_build_cache();

                            let mut new_hovered_span = None;

                            // Only check if mouse is within the canvas area
                            if canvas_y >= 0.0 && current_x >= THREAD_LABEL_WIDTH {
                                for (idx, span) in frame.spans.iter().enumerate() {
                                    let thread_y_offset = cache.thread_offsets.get(&span.thread_id).copied().unwrap_or(0.0);
                                    let y = thread_y_offset + (span.depth as f32 * ROW_HEIGHT) + view_state_copy.pan_y;

                                    if canvas_y >= y && canvas_y <= y + ROW_HEIGHT {
                                        let x1 = time_to_x(span.start_ns, &frame, viewport_width, &view_state_copy);
                                        let x2 = time_to_x(span.end_ns(), &frame, viewport_width, &view_state_copy);

                                        if current_x >= x1 && current_x <= x2 {
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

                            let old_zoom = view.view_state.zoom;
                            let zoom_factor = 1.0 - (delta_y * 0.01);
                            let new_zoom = old_zoom * zoom_factor;

                            // Calculate world position under cursor before zoom (X only)
                            let world_x = (cursor_x - view.view_state.pan_x) / old_zoom;

                            // Update zoom
                            view.view_state.zoom = new_zoom;

                            // Adjust pan_x so the same world position stays under cursor
                            // Keep pan_y unchanged
                            view.view_state.pan_x = cursor_x - (world_x * new_zoom);
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
                        let tl_start = std::time::Instant::now();
                        let labels = render_thread_labels(&frame, &*thread_offsets, &view_state, cx);
                        labels
                    })
                    .child({
                        let ss_start = std::time::Instant::now();
                        let sidebar = render_statistics_sidebar(&frame, cx);
                        sidebar
                    })
                    .children({
                        let hp_start = std::time::Instant::now();
                        let popup = render_hover_popup(&frame, &view_state, *self.viewport_width.read().unwrap(), cx);
                        popup
                    })
            )
            .child(
                // Status bar at bottom
                div()
                    .h(px(40.0))
                    .w_full()
                    .bg(theme.list_head)
                    .border_t_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .px_4()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(format!("Spans: {}", frame.spans.len()))
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(format!("Threads: {}", frame.threads.len()))
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(format!("Duration: {:.2}ms", frame.duration_ns() as f64 / 1_000_000.0))
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(format!("Zoom: {:.1}x", view_state.zoom))
                    )
            );

        result
    }
}
