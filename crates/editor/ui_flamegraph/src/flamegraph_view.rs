use crate::components::*;
use crate::constants::*;
use crate::coordinates::time_to_x;
use crate::lod_tree::LODTree;
use crate::rendering::renderer::FlamegraphRenderer;
use crate::rendering::types::{FlamegraphUniforms, GpuSpan};
use crate::state::{SpanCache, ViewState};
use crate::trace_data::{TraceData, TraceFrame};
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::sync::Arc;
use ui::v_flex;
use ui::ActiveTheme;
use ui::PixelsExt;

const SPAN_HOVER_HEIGHT_SCALE: f32 = 0.8;

pub struct FlamegraphView {
    trace_data: TraceData,
    view_state: ViewState,
    cache: Option<(Arc<TraceFrame>, Arc<SpanCache>)>,
    viewport_width: Arc<std::sync::RwLock<f32>>,
    viewport_height: Arc<std::sync::RwLock<f32>>,
    viewport_origin_x: Arc<std::sync::RwLock<f32>>,
    viewport_origin_y: Arc<std::sync::RwLock<f32>>,
    graph_width: Arc<std::sync::RwLock<f32>>,
    graph_origin_x: Arc<std::sync::RwLock<f32>>,
    surface: Option<WgpuSurfaceHandle>,
    renderer: FlamegraphRenderer,
    /// Current LOD level index (cached from last paint).
    lod_level: Option<usize>,
    /// Cached GpuSpans for the current LOD level — rebuilt only when LOD changes.
    lod_spans: Option<Arc<Vec<GpuSpan>>>,
}

impl FlamegraphView {
    /// Compute the LOD level index from view state and viewport width.
    fn lod_level_for(&self, viewport_w: f32, frame: &TraceFrame) -> usize {
        let zoom = if self.view_state.zoom == 0.0 && frame.duration_ns() > 0 {
            viewport_w / frame.duration_ns() as f32
        } else {
            self.view_state.zoom
        };
        let pixels_per_ns = zoom.max(1e-10) as f64;
        // Can't call LODTree::select_level without the tree — we compute inline.
        // Target: bucket >= 4.0 / pixels_per_ns
        let min_bucket = (4.0 / pixels_per_ns) as u64;
        let sizes: &[u64] = &[
            50_000,
            100_000,
            500_000,
            1_000_000,
            5_000_000,
            10_000_000,
            50_000_000,
            100_000_000,
            200_000_000,
            500_000_000,
            1_000_000_000,
        ];
        if min_bucket >= sizes[sizes.len() - 1] {
            return sizes.len() - 1;
        }
        let mut best = sizes.len() - 1;
        let mut best_diff = u64::MAX;
        for (i, &s) in sizes.iter().enumerate() {
            if s >= min_bucket {
                let diff = s - min_bucket;
                if diff < best_diff {
                    best_diff = diff;
                    best = i;
                }
            }
        }
        best
    }

    /// Rebuild cached spans from the LOD tree at the given level.
    fn rebuild_lod(&mut self, level: usize, frame: &TraceFrame, lod_tree: &LODTree) {
        let spans = Arc::new(lod_tree.collect_level_gpu_spans(level, frame.min_time_ns));
        self.lod_level = Some(level);
        self.lod_spans = Some(spans);
    }
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
            surface: None,
            renderer: FlamegraphRenderer::new(),
            lod_level: None,
            lod_spans: None,
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

    fn get_or_build_cache(&mut self) -> (Arc<TraceFrame>, Arc<SpanCache>) {
        let frame = self.trace_data.get_frame();

        let needs_rebuild = match &self.cache {
            Some((cached_frame, _)) => !Arc::ptr_eq(cached_frame, &frame),
            None => true,
        };

        if needs_rebuild {
            let cache = Arc::new(SpanCache::build(&frame));

            if self.view_state.zoom == 0.0 && frame.duration_ns() > 0 {
                let effective_width = self.view_state.viewport_width - THREAD_LABEL_WIDTH;
                self.view_state.zoom = effective_width / frame.duration_ns() as f32;
                self.view_state.pan_x = 0.0;
            }

            self.cache = Some((Arc::clone(&frame), cache));
        }

        let (frame_ref, cache_ref) = self
            .cache
            .as_ref()
            .expect("Cache should be populated by get_or_build_cache");
        (Arc::clone(frame_ref), Arc::clone(cache_ref))
    }
}

impl Render for FlamegraphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (frame, cache) = self.get_or_build_cache();

        let frame_for_graph = Arc::clone(&frame);
        let thread_offsets = Arc::clone(&cache.thread_offsets);
        let view_state = self.view_state.clone();
        let theme = cx.theme().clone();

        let view_state_for_canvas = view_state.clone();

        let framerate_graph = render_framerate_graph(&frame_for_graph, &view_state, cx);

        // Combined WGPU surface driver — renders ruler + main canvas in one pass
        let entity = cx.entity().clone();
        let driver = {
            let entity_pre = entity.clone();
            let entity_paint = entity.clone();

            gpui::canvas(
                move |bounds, window, cx| {
                    let sw = bounds.size.width.as_f32().max(1.0) as u32;
                    let sh = bounds.size.height.as_f32().max(1.0) as u32;

                    entity_pre.update(cx, |view, cx| {
                        if view.surface.is_none() {
                            if let Some(s) = window.create_wgpu_surface(
                                sw.max(64),
                                sh.max(64),
                                wgpu::TextureFormat::Bgra8UnormSrgb,
                            ) {
                                view.surface = Some(s);
                                cx.notify();
                            }
                        }
                    });
                },
                move |_bounds, _pre, _window, cx| {
                    entity_paint.update(cx, |view, cx| {
                        // ── Clone surface handle to avoid borrow conflicts ──
                        let surface_clone = match &view.surface {
                            Some(s) => s.clone(),
                            None => return,
                        };
                        if surface_clone.is_resize_pending() {
                            return;
                        }
                        let Some((tex_view, (w, h))) = surface_clone.back_view_with_size() else {
                            return;
                        };
                        let surface_device = surface_clone.device();
                        let surface_queue = surface_clone.queue();
                        let surface_fmt = surface_clone.format();

                        // ── Read live frame + cache from view (never stale) ──
                        let (frame, cache) = view.get_or_build_cache();

                        // ── LOD selection ──
                        let level = view.lod_level_for(w as f32, &frame);
                        if view.lod_level != Some(level) {
                            view.rebuild_lod(level, &frame, &cache.lod_tree);
                        }

                        // GPU spans — cached per-LOD, zero per-frame work
                        let span_slice: &[GpuSpan] =
                            view.lod_spans.as_ref().map_or(&[], |a| a.as_slice());
                        let spans =
                            crate::components::flamegraph_canvas::build_instances(span_slice);

                        // Use LATEST view state (not the captured stale copy)
                        let vs = &view.view_state;

                        // Text labels — query LOD tree directly (only visible buckets)
                        let text_rects = crate::components::flamegraph_canvas::build_text_instances(
                            &frame,
                            &cache.lod_tree,
                            level,
                            vs,
                            w as f32,
                            h as f32,
                        );

                        // Overlay rects (grid lines + thread separators)
                        let overlay_rects =
                            crate::components::flamegraph_canvas::build_overlay_instances(
                                &frame,
                                &cache.thread_offsets,
                                vs,
                                w as f32,
                                h as f32,
                            );

                        // Ruler instances
                        let ruler_rects =
                            crate::components::flamegraph_canvas::build_ruler_instances(
                                &frame, vs, w as f32,
                            );

                        // Debug overlay (stats)
                        let debug_rects = crate::components::flamegraph_canvas::build_debug_overlay(
                            &frame,
                            &cache.lod_tree,
                            level,
                            vs,
                            w as f32,
                        );

                        // Combine overlays + ruler + text + debug into one rects vec
                        let text_all = {
                            let mut combined = ruler_rects;
                            combined.extend(overlay_rects);
                            combined.extend(text_rects);
                            combined.extend(debug_rects);
                            combined
                        };

                        let uniforms = FlamegraphUniforms {
                            viewport_x: w as f32,
                            viewport_y: h as f32,
                            pan_x: vs.pan_x,
                            pan_y: vs.pan_y,
                            zoom: vs.zoom,
                            thread_label_width: THREAD_LABEL_WIDTH,
                            y_adj: -GRAPH_HEIGHT,
                            row_h: ROW_HEIGHT,
                        };

                        view.renderer.render_frame(
                            surface_device,
                            surface_queue,
                            &tex_view,
                            w,
                            h,
                            surface_fmt,
                            &uniforms,
                            spans,
                            &text_all,
                        );
                        drop(tex_view);
                        surface_clone.swap_buffers();
                        let _ = cx;
                    });
                },
            )
            .absolute()
            .inset_0()
            .size_full()
        };

        let gpu_display: AnyElement = if let Some(ref s) = self.surface {
            wgpu_surface(s.clone())
                .defer_resize_until_mouse_up(true)
                .absolute()
                .inset_0()
                .into_any_element()
        } else {
            div()
                .absolute()
                .inset_0()
                .bg(theme.background)
                .into_any_element()
        };

        v_flex()
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
                            .on_mouse_move(cx.listener(
                                |view, event: &MouseMoveEvent, _window, cx| {
                                    if !view.view_state.graph_dragging {
                                        return;
                                    }

                                    let window_x: f32 = event.position.x.into();
                                    let local_x = window_x - *view.graph_origin_x.read().unwrap();
                                    let frame = view.trace_data.get_frame();

                                    if let Some(time_ns) = view.graph_x_to_time_ns(&frame, local_x)
                                    {
                                        view.view_state.crop_end_time_ns = Some(time_ns);
                                        cx.notify();
                                    }
                                },
                            ))
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
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(4.0))
                                    .right(px(4.0))
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.0))
                                    .bg(theme.accent)
                                    .text_color(theme.background)
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style.bg(gpui::hsla(205.0 / 360.0, 0.7, 0.55, 1.0))
                                    })
                                    .child("Generate 64-Thread Trace")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|view, _event, _window, cx| {
                                            view.trace_data
                                                .generate_massive_trace(64, 5, 500, 8000);
                                            cx.notify();
                                        }),
                                    ),
                            ),
                    ),
            )
            .child(
                // Combined surface: ruler + main canvas
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
                            if let Some(canvas_bounds) = bounds.first() {
                                *viewport_width.write().unwrap() = canvas_bounds.size.width.into();
                                *viewport_height.write().unwrap() =
                                    canvas_bounds.size.height.into();
                                *viewport_origin_x.write().unwrap() = canvas_bounds.origin.x.into();
                                *viewport_origin_y.write().unwrap() = canvas_bounds.origin.y.into();
                            }
                        }
                    })
                    .child(gpu_display)
                    .child(driver)
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

                        view.view_state.mouse_x = local_x;
                        view.view_state.mouse_y = local_y;

                        if view.view_state.dragging {
                            let delta_x = window_x - view.view_state.drag_start_x;
                            let delta_y = window_y - view.view_state.drag_start_y;

                            view.view_state.pan_x = view.view_state.drag_pan_start_x + delta_x;
                            view.view_state.pan_y = view.view_state.drag_pan_start_y + delta_y;
                        } else {
                            let view_state_copy = view.view_state.clone();
                            let viewport_width = *view.viewport_width.read().unwrap();
                            let viewport_height = *view.viewport_height.read().unwrap();
                            let (frame, cache) = view.get_or_build_cache();

                            let mut new_hovered_span = None;

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
                                    let y = thread_y_offset - GRAPH_HEIGHT
                                        + (span.depth as f32 * ROW_HEIGHT)
                                        + view_state_copy.pan_y;

                                    if local_y >= y
                                        && local_y
                                            <= y + ((ROW_HEIGHT - PADDING)
                                                * SPAN_HOVER_HEIGHT_SCALE)
                                    {
                                        let x1 = time_to_x(
                                            span.start_ns,
                                            &frame,
                                            viewport_width,
                                            &view_state_copy,
                                        );
                                        let x2 = time_to_x(
                                            span.end_ns(),
                                            &frame,
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

                        if event.modifiers.control || event.modifiers.platform {
                            let cursor_pos: Point<Pixels> = event.position;
                            let cursor_x: f32 = cursor_pos.x.into();
                            let local_cursor_x = cursor_x - *view.viewport_origin_x.read().unwrap();

                            let old_zoom = view.view_state.zoom;
                            let zoom_factor = 1.0 - (delta_y * 0.01);
                            let new_zoom = old_zoom * zoom_factor;

                            let world_x = (local_cursor_x - view.view_state.pan_x) / old_zoom;

                            view.view_state.zoom = new_zoom;
                            view.view_state.pan_x = local_cursor_x - (world_x * new_zoom);
                        } else if event.modifiers.shift {
                            view.view_state.pan_y -= delta_y * 10.0;
                        } else {
                            view.view_state.pan_x -= delta_y * 10.0;
                        }

                        cx.notify();
                    }))
                    .child({ render_thread_labels(&frame, &thread_offsets, &view_state, cx) })
                    .children({
                        let popup = render_hover_popup(
                            &frame,
                            &view_state,
                            *self.viewport_width.read().unwrap(),
                            cx,
                        );
                        popup
                    }),
            )
    }
}
