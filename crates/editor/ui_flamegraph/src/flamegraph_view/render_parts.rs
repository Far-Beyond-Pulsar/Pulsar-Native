//! Named pieces of `FlamegraphView::render`.
//!
//! Owns the element-building helpers and GPU paint steps that `render` in the
//! parent module composes: the framerate strip with its crop selection, the
//! timeline area with its mouse/scroll handlers, the WGPU surface display and
//! the canvas driver whose prepaint/paint callbacks create the surface and draw
//! a frame. Entry points: `render_graph_strip`, `render_timeline_area`,
//! `render_gpu_display`, `render_driver_canvas`.

use super::*;
use crate::rendering::types::RectInstance;
use std::collections::BTreeMap;

impl FlamegraphView {
    /// Surface that shows the GPU-rendered flamegraph, or a themed placeholder until it exists.
    pub(super) fn render_gpu_display(&self, background: Hsla) -> AnyElement {
        if let Some(ref s) = self.surface {
            wgpu_surface(s.clone())
                .defer_resize_until_mouse_up(true)
                .absolute()
                .inset_0()
                .into_any_element()
        } else {
            div().absolute().inset_0().bg(background).into_any_element()
        }
    }

    /// Canvas that creates the WGPU surface before paint and draws a flamegraph frame into it.
    pub(super) fn render_driver_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Combined WGPU surface driver — renders ruler + main canvas in one pass
        let entity = cx.entity().clone();
        let entity_pre = entity.clone();
        let entity_paint = entity.clone();

        gpui::canvas(
            move |bounds, window, cx| {
                let sw = bounds.size.width.as_f32().max(1.0) as u32;
                let sh = bounds.size.height.as_f32().max(1.0) as u32;

                entity_pre.update(cx, |view, cx| {
                    view.ensure_surface(sw, sh, window, cx);
                });
            },
            move |_bounds, _pre, window, cx| {
                entity_paint.update(cx, |view, _cx| {
                    view.paint_frame(window);
                });
            },
        )
        .absolute()
        .inset_0()
        .size_full()
    }

    /// Creates the WGPU surface when missing and drops it after a GPU device loss.
    fn ensure_surface(
        &mut self,
        width: u32,
        height: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The GPU device was lost: the surface and the
        // renderer's buffers are on it. Start over on the
        // window's new device.
        if self
            .surface
            .as_ref()
            .is_some_and(WgpuSurfaceHandle::is_device_lost)
        {
            self.surface = None;
            self.renderer = FlamegraphRenderer::new();
        }
        if self.surface.is_none() {
            // Until the window has a new device, a new
            // surface is on the lost one too; the window
            // renders every view again once it has one.
            if let Some(s) = window
                .create_wgpu_surface(
                    width.max(64),
                    height.max(64),
                    wgpu::TextureFormat::Bgra8UnormSrgb,
                )
                .filter(|s| !s.is_device_lost())
            {
                self.surface = Some(s);
                cx.notify();
            }
        }
    }

    /// Draws one flamegraph frame (spans, labels, overlays) into the surface's back buffer.
    fn paint_frame(&mut self, window: &Window) {
        // Window resize/fullscreen reconfiguration takes the
        // device's exclusive submit side. Do not submit a
        // flamegraph frame while the window is in that
        // transition; the compositor will repaint once the
        // new size is committed.
        if window.is_window_resizing() {
            return;
        }
        // ── Clone surface handle to avoid borrow conflicts ──
        let surface_clone = match &self.surface {
            Some(s) => s.clone(),
            None => return,
        };
        if surface_clone.is_resize_pending() {
            return;
        }
        // The flamegraph shares the application's WGPU
        // device with the main viewport. Never enqueue a new
        // frame while the compositor still owns the previous
        // one. Without this gate GPUI repaint cadence can
        // outrun composition, growing GPU work until Helio
        // stops presenting; opening another surface then
        // makes the entire app appear deadlocked.
        if surface_clone.has_unconsumed_frame() {
            return;
        }
        // Surface reconfiguration and external rendering use
        // the same device. Hold the shared submit guard for
        // the entire acquire/encode/submit/publish sequence
        // so an exclusive resize cannot observe in-flight
        // flamegraph work.
        let Some(_gpu_submit_guard) = surface_clone.try_submit_guard() else {
            return;
        };
        let Some((tex_view, (w, h))) = surface_clone.back_view_with_size() else {
            return;
        };
        let surface_device = surface_clone.device();
        let surface_queue = surface_clone.queue();
        let surface_fmt = surface_clone.format();

        // ── Read live frame + cache from view (never stale) ──
        let (frame, cache) = self.get_or_build_cache();

        let level = self.refresh_lod_spans(w, h, &frame, &cache);

        // GPU spans — cached per-LOD, zero per-frame work
        let span_slice: &[GpuSpan] = self.lod_spans.as_ref().map_or(&[], |a| a.as_slice());
        let spans = crate::components::flamegraph_canvas::build_instances(span_slice);

        // Use LATEST view state (not the captured stale copy)
        let vs = &self.view_state;

        let text_all = Self::collect_overlay_rects(&frame, &cache, vs, level, w as f32, h as f32);

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

        self.renderer.render_frame(
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
    }

    /// Picks the LOD level for the canvas width and rebuilds the cached GPU spans when
    /// the level or vertical tile changed. Returns the LOD level.
    fn refresh_lod_spans(
        &mut self,
        w: u32,
        h: u32,
        frame: &TraceFrame,
        cache: &SpanCache,
    ) -> usize {
        // ── LOD selection ──
        let level = self.lod_level_for(w as f32, frame);
        // Vertical world-Y window visible to the canvas,
        // padded by a full tile so panning within a tile never
        // pops spans. Mirrors `build_text_instances`'s math.
        let y_adj = -GRAPH_HEIGHT;
        let y_min_world = -y_adj - self.view_state.pan_y - ROW_HEIGHT;
        let y_max_world = (h as f32) - y_adj - self.view_state.pan_y;
        let vertical_tile =
            (crate::state::TILE_ROW_HEIGHT.max(1.0).recip() * y_min_world).floor() as i64;
        if self.lod_level != Some(level) || self.lod_vertical_key != Some((level, vertical_tile)) {
            self.rebuild_lod(
                level,
                vertical_tile,
                frame,
                &cache.lod_tree,
                y_min_world - crate::state::TILE_ROW_HEIGHT,
                y_max_world + crate::state::TILE_ROW_HEIGHT,
            );
        }
        level
    }

    /// Builds the rect instances drawn over the spans, in draw order: ruler, grid and
    /// thread separators, frame boundaries, labels, dependency arrows, debug overlay.
    fn collect_overlay_rects(
        frame: &TraceFrame,
        cache: &SpanCache,
        vs: &ViewState,
        level: usize,
        w: f32,
        h: f32,
    ) -> Vec<RectInstance> {
        use crate::components::flamegraph_canvas as canvas;

        // Text labels — query LOD tree directly (only visible buckets)
        let text_rects = canvas::build_text_instances(frame, &cache.lod_tree, level, vs, w, h);

        // Overlay rects (grid lines + thread separators)
        let overlay_rects = canvas::build_overlay_instances(frame, &cache.thread_offsets, vs, w, h);

        // Ruler instances
        let ruler_rects = canvas::build_ruler_instances(frame, vs, w);

        // Frame boundary lines (thin vertical lines marking end/start
        // of each frame, across all threads)
        let frame_line_rects = canvas::build_frame_boundary_instances(frame, vs, w, h);

        // Cross-thread wait/block arrows for the double-clicked span
        let arrow_rects = vs
            .selected_span
            .map(|sel| canvas::build_dependency_arrows(frame, cache, sel, vs, w, h))
            .unwrap_or_default();

        // Debug overlay (stats)
        let debug_rects = canvas::build_debug_overlay(frame, &cache.lod_tree, level, vs, w);

        // Combine overlays + frame lines + labels + arrows + debug into
        // one rects vec (drawn in order, arrows last = on top).
        let mut combined = ruler_rects;
        combined.extend(overlay_rects);
        combined.extend(frame_line_rects);
        combined.extend(text_rects);
        combined.extend(arrow_rects);
        combined.extend(debug_rects);
        combined
    }

    /// Top strip: framerate graph, crop-selection overlay with its drag handlers and the
    /// "Generate 64-Thread Trace" button.
    pub(super) fn render_graph_strip(
        &self,
        frame: &TraceFrame,
        framerate_graph: impl IntoElement,
        accent: Hsla,
        background: Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .relative()
            .h(px(GRAPH_HEIGHT))
            .w_full()
            .on_children_prepainted({
                let graph_width = self.graph_width.clone();
                let graph_origin_x = self.graph_origin_x.clone();
                move |bounds: Vec<Bounds<Pixels>>, _window: &mut Window, _cx: &mut App| {
                    if let Some(graph_bounds) = bounds.first() {
                        store_f32(&graph_width, graph_bounds.size.width.into());
                        store_f32(&graph_origin_x, graph_bounds.origin.x.into());
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
                        self.with_crop_selection(this, frame)
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_graph_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_graph_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_graph_mouse_up))
                    .child(self.render_generate_trace_button(accent, background, cx)),
            )
    }

    /// Adds the translucent crop-selection box to the graph overlay when a drag range is set.
    fn with_crop_selection(&self, overlay: Div, frame: &TraceFrame) -> Div {
        if let (Some(start), Some(end)) = (
            self.view_state.crop_start_time_ns,
            self.view_state.crop_end_time_ns,
        ) {
            let duration = frame.duration_ns().max(1) as f32;
            let graph_width = load_f32(&self.graph_width);
            let start_ratio = (start.min(end).saturating_sub(frame.min_time_ns)) as f32 / duration;
            let end_ratio = (start.max(end).saturating_sub(frame.min_time_ns)) as f32 / duration;
            let left = (start_ratio * graph_width).max(0.0);
            let width = ((end_ratio - start_ratio) * graph_width).max(1.0);

            overlay.child(
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
            overlay
        }
    }

    /// Button in the graph strip that fills the view with a 64-thread stress trace.
    fn render_generate_trace_button(
        &self,
        accent: Hsla,
        background: Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .absolute()
            .top(px(4.0))
            .right(px(4.0))
            .px_2()
            .py_1()
            .rounded(px(4.0))
            .bg(accent)
            .text_color(background)
            .text_xs()
            .font_weight(gpui::FontWeight::MEDIUM)
            .cursor_pointer()
            .hover(|style| style.bg(gpui::hsla(205.0 / 360.0, 0.7, 0.55, 1.0)))
            .child("Generate 64-Thread Trace")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _event, _window, cx| {
                    view.trace_data.generate_massive_trace(64, 5, 500, 8000);
                    cx.notify();
                }),
            )
    }

    /// Starts a crop-selection drag in the graph strip.
    fn on_graph_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let window_x: f32 = event.position.x.into();
        let local_x = window_x - load_f32(&self.graph_origin_x);
        let frame = self.trace_data.get_frame();

        if let Some(time_ns) = self.graph_x_to_time_ns(&frame, local_x) {
            self.view_state.graph_dragging = true;
            self.view_state.graph_drag_start_x = local_x;
            self.view_state.crop_dragging = true;
            self.view_state.crop_start_time_ns = Some(time_ns);
            self.view_state.crop_end_time_ns = Some(time_ns);
            cx.notify();
        }
    }

    /// Extends the crop-selection range while dragging in the graph strip.
    fn on_graph_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.view_state.graph_dragging {
            return;
        }

        let window_x: f32 = event.position.x.into();
        let local_x = window_x - load_f32(&self.graph_origin_x);
        let frame = self.trace_data.get_frame();

        if let Some(time_ns) = self.graph_x_to_time_ns(&frame, local_x) {
            self.view_state.crop_end_time_ns = Some(time_ns);
            cx.notify();
        }
    }

    /// Ends the graph drag: a short drag centers the view, a long one fits the dragged segment.
    fn on_graph_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.view_state.graph_dragging {
            return;
        }

        let window_x: f32 = event.position.x.into();
        let local_x = window_x - load_f32(&self.graph_origin_x);
        let drag_distance = (local_x - self.view_state.graph_drag_start_x).abs();
        let frame = self.trace_data.get_frame();

        if drag_distance < 4.0 {
            if let Some(center_time) = self
                .view_state
                .crop_end_time_ns
                .or(self.view_state.crop_start_time_ns)
            {
                self.center_bottom_view_on_time(&frame, center_time);
            }
        } else if let (Some(start), Some(end)) = (
            self.view_state.crop_start_time_ns,
            self.view_state.crop_end_time_ns,
        ) {
            self.fit_bottom_view_to_segment(&frame, start, end);
        }

        self.view_state.graph_dragging = false;
        self.view_state.crop_dragging = false;
        self.view_state.crop_start_time_ns = None;
        self.view_state.crop_end_time_ns = None;
        cx.notify();
    }

    /// Main area: GPU display and driver canvas with pan/zoom/select handlers, thread
    /// labels and the hover popup.
    pub(super) fn render_timeline_area(
        &self,
        gpu_display: AnyElement,
        driver: impl IntoElement,
        frame: &Arc<TraceFrame>,
        thread_offsets: &Arc<BTreeMap<u64, f32>>,
        view_state: &ViewState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                        store_f32(&viewport_width, canvas_bounds.size.width.into());
                        store_f32(&viewport_height, canvas_bounds.size.height.into());
                        store_f32(&viewport_origin_x, canvas_bounds.origin.x.into());
                        store_f32(&viewport_origin_y, canvas_bounds.origin.y.into());
                    }
                }
            })
            .child(gpu_display)
            .child(driver)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(Self::on_timeline_right_down),
            )
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_timeline_right_up))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_timeline_left_down))
            .on_mouse_move(cx.listener(Self::on_timeline_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_timeline_scroll))
            .child({
                let viewport_h = load_f32(&self.viewport_height);
                render_thread_labels(frame, thread_offsets, view_state, viewport_h, cx)
            })
            .children(render_hover_popup(
                frame,
                view_state,
                load_f32(&self.viewport_width),
                cx,
            ))
    }

    /// Starts a right-button pan drag on the timeline.
    fn on_timeline_right_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.view_state.dragging = true;
        let pos: Point<Pixels> = event.position;
        self.view_state.drag_start_x = pos.x.into();
        self.view_state.drag_start_y = pos.y.into();
        self.view_state.drag_pan_start_x = self.view_state.pan_x;
        self.view_state.drag_pan_start_y = self.view_state.pan_y;
        cx.notify();
    }

    /// Ends the right-button pan drag.
    fn on_timeline_right_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.view_state.dragging = false;
        cx.notify();
    }

    /// Selects the span under the cursor (clearing the selection on empty space).
    fn on_timeline_left_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pos: Point<Pixels> = event.position;
        let window_x: f32 = pos.x.into();
        let window_y: f32 = pos.y.into();
        let local_x = window_x - load_f32(&self.viewport_origin_x);
        let local_y = window_y - load_f32(&self.viewport_origin_y);

        // Clicking a box selects it: its call stack is shown and
        // the cross-thread wait/block dependency arrows to and
        // from it are drawn. Clicking empty space clears it.
        let (frame, cache) = self.get_or_build_cache();
        self.view_state.selected_span = self.pick_span_at(&frame, &cache, local_x, local_y);
        if self.view_state.selected_span.is_some() {
            cx.emit(SpanSelected);
        }
        cx.notify();
    }

    /// Tracks the cursor, panning while dragging and updating the hovered span otherwise.
    fn on_timeline_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pos: Point<Pixels> = event.position;
        let window_x: f32 = pos.x.into();
        let window_y: f32 = pos.y.into();
        let local_x = window_x - load_f32(&self.viewport_origin_x);
        let local_y = window_y - load_f32(&self.viewport_origin_y);

        self.view_state.mouse_x = local_x;
        self.view_state.mouse_y = local_y;

        if self.view_state.dragging {
            let delta_x = window_x - self.view_state.drag_start_x;
            let delta_y = window_y - self.view_state.drag_start_y;

            self.view_state.pan_x = self.view_state.drag_pan_start_x + delta_x;
            self.view_state.pan_y = self.view_state.drag_pan_start_y + delta_y;
        } else {
            let (frame, cache) = self.get_or_build_cache();
            self.view_state.hovered_span = self.pick_span_at(&frame, &cache, local_x, local_y);
        }

        cx.notify();
    }

    /// Wheel: zoom about the cursor with Ctrl/Cmd, pan vertically with Shift, else pan horizontally.
    fn on_timeline_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(1.0));
        let delta_y: f32 = delta.y.into();

        if event.modifiers.control || event.modifiers.platform {
            let cursor_pos: Point<Pixels> = event.position;
            let cursor_x: f32 = cursor_pos.x.into();
            let local_cursor_x = cursor_x - load_f32(&self.viewport_origin_x);

            let old_zoom = self.view_state.zoom;
            let zoom_factor = 1.0 - (delta_y * 0.01);
            let new_zoom = old_zoom * zoom_factor;

            let world_x = (local_cursor_x - self.view_state.pan_x) / old_zoom;

            self.view_state.zoom = new_zoom;
            self.view_state.pan_x = local_cursor_x - (world_x * new_zoom);
        } else if event.modifiers.shift {
            self.view_state.pan_y -= delta_y * 10.0;
        } else {
            self.view_state.pan_x -= delta_y * 10.0;
        }

        cx.notify();
    }
}
