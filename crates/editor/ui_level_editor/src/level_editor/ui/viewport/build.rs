//! Viewport panel UI construction: the per-frame element tree.

use super::*;

impl ViewportPanel {
    /// Build the complete viewport UI.
    pub(super) fn build_viewport_ui<V>(
        &mut self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        snapshot: Option<EngineFrameSnapshot>,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let _viewport_hovered = self.viewport_hovered.clone();
        let _element_bounds = self.element_bounds.clone();
        let viewport_entity = self.viewport.clone();

        let empty_snapshot = EngineFrameSnapshot::default();
        let snap = snapshot.as_ref().unwrap_or(&empty_snapshot);
        let ui_fps = snap.ui_fps;
        let render_fps = snap.render_fps;

        // Collect metric histories only while the performance overlay can
        // show them; cloning eight Vecs per frame with the overlay closed is
        // pure waste on a hot path.
        let perf_snapshot = if state.overlays.state.show_performance_overlay {
            PerformanceSnapshot::capture(&self.metrics.borrow(), ui_fps, render_fps)
        } else {
            PerformanceSnapshot::empty()
        };

        // Clone for event handlers
        let _input_state_scroll = Arc::clone(&self.input_state);
        let mouse_right_captured = self.mouse_right_captured.clone();
        let mouse_middle_captured = self.mouse_middle_captured.clone();
        // Pointer-event queue for the left-click/left-release handlers below
        // (Pulsar-Native drag-release freeze fix -- see `PendingPointerEvent`'s
        // doc). Fetched once, in the same locked pass as the frame stats --
        // and non-blocking, so a miss just means those two handlers fall back
        // to a no-op this rebuild and correctly pick the queue back up next
        // time (GPUI rebuilds this element tree far more often than a user
        // can actually click). The click/release closures themselves never
        // touch `gpu_engine` again after this.
        let pointer_events_for_click = snap.pointer_events.clone();
        let camera_input_for_prepaint = snap.camera_input.clone();
        let element_bounds_for_prepaint = self.element_bounds.clone();
        let element_bounds_for_click = self.element_bounds.clone();
        let _state_arc_scroll = state_arc.clone();
        let last_mouse_x = self.last_mouse_x.clone();
        let last_mouse_y = self.last_mouse_y.clone();
        let locked_cursor_x = self.locked_cursor_x.clone();
        let locked_cursor_y = self.locked_cursor_y.clone();
        let locked_cursor_screen_x = self.locked_cursor_screen_x.clone();
        let locked_cursor_screen_y = self.locked_cursor_screen_y.clone();
        let cursor_capture =
            ViewportCursorCapture::load(&mouse_right_captured, &mouse_middle_captured);

        // For mouse move tracking
        let element_bounds_move = self.element_bounds.clone();
        let gpu_engine_move = gpu_engine.clone();
        let last_mouse_pos = Rc::new(RefCell::new(None::<(f32, f32)>));

        // Main viewport container
        div()
            .size_full()
            .relative()
            // Cursor requests are paint state in GPUI. Event handlers update the
            // capture atomics and refresh the window; paint owns presentation.
            .when(cursor_capture.is_active(), |viewport| {
                viewport.cursor(CursorStyle::None)
            })
            // TRANSPARENT - no background for direct Helio rendering
            .rounded(cx.theme().radius)
            // CRITICAL: Capture element bounds and update Helio camera viewport.
            // Runs every frame, cached or not — geometry stashing belongs here,
            // not in on_children_prepainted (which is skipped on cache hits).
            .on_frame(move |geom, _window, _cx| {
                *element_bounds_for_prepaint.borrow_mut() = Some(geom.bounds);

                // Update Helio camera viewport to match GPUI viewport bounds
                if let Some(cam) = &camera_input_for_prepaint {
                    if let Ok(mut camera_input) = cam.try_lock() {
                        let origin_x: f32 = geom.bounds.origin.x.into();
                        let origin_y: f32 = geom.bounds.origin.y.into();
                        let width: f32 = geom.bounds.size.width.into();
                        let height: f32 = geom.bounds.size.height.into();
                        camera_input.viewport_x = origin_x;
                        camera_input.viewport_y = origin_y;
                        camera_input.viewport_width = width;
                        camera_input.viewport_height = height;
                    }
                }
            })
            // Track mouse movement and update Helio input
            .on_mouse_move({
                let input_state_clone = self.input_state.clone();
                let last_mouse_x = last_mouse_x.clone();
                let last_mouse_y = last_mouse_y.clone();
                let mouse_right_captured = mouse_right_captured.clone();
                let mouse_middle_captured = mouse_middle_captured.clone();
                let state_arc_move = state_arc.clone();

move |event: &gpui::MouseMoveEvent, _window, _cx| {
                    super::build_handlers::handle_mouse_move(
                        event,
                        input_state_clone.clone(),
                        last_mouse_x.clone(),
                        last_mouse_y.clone(),
                        mouse_right_captured.clone(),
                        mouse_middle_captured.clone(),
                        state_arc_move.clone(),
                        gpu_engine_move.clone(),
                        element_bounds_move.clone(),
                        last_mouse_pos.clone(),
                    );
                }
            })
            // Right-click for camera controls
            .on_mouse_down(gpui::MouseButton::Right, {
                let last_mouse_x = last_mouse_x.clone();
                let last_mouse_y = last_mouse_y.clone();
                let mouse_right_captured = mouse_right_captured.clone();
                let mouse_middle_captured = mouse_middle_captured.clone();
                let locked_cursor_x = locked_cursor_x.clone();
                let locked_cursor_y = locked_cursor_y.clone();
                let locked_cursor_screen_x = locked_cursor_screen_x.clone();
                let locked_cursor_screen_y = locked_cursor_screen_y.clone();
                let input_state_clone = self.input_state.clone();

move |event: &gpui::MouseDownEvent, window: &mut gpui::Window, _cx: &mut gpui::App| {
                    super::build_handlers::handle_right_mouse_down(
                        event,
                        window,
                        last_mouse_x.clone(),
                        last_mouse_y.clone(),
                        mouse_right_captured.clone(),
                        mouse_middle_captured.clone(),
                        locked_cursor_x.clone(),
                        locked_cursor_y.clone(),
                        locked_cursor_screen_x.clone(),
                        locked_cursor_screen_y.clone(),
                        input_state_clone.clone(),
                    );
                }
            })
            // Right-click release
            .on_mouse_up(gpui::MouseButton::Right, {
                let last_mouse_x = last_mouse_x.clone();
                let last_mouse_y = last_mouse_y.clone();
                let locked_cursor_x = locked_cursor_x.clone();
                let locked_cursor_y = locked_cursor_y.clone();
                let mouse_right_captured = mouse_right_captured.clone();
                let mouse_middle_captured = mouse_middle_captured.clone();
                let locked_cursor_screen_x = locked_cursor_screen_x.clone();
                let locked_cursor_screen_y = locked_cursor_screen_y.clone();

move |_event, _window, _cx| {
                    super::build_handlers::handle_right_mouse_up(
                        last_mouse_x.clone(),
                        last_mouse_y.clone(),
                        locked_cursor_x.clone(),
                        locked_cursor_y.clone(),
                        mouse_right_captured.clone(),
                        mouse_middle_captured.clone(),
                        locked_cursor_screen_x.clone(),
                        locked_cursor_screen_y.clone(),
                    );
                }
            })
            // Middle-click drag to pan the camera along the current view plane
            .on_mouse_down(gpui::MouseButton::Middle, {
                let last_mouse_x = last_mouse_x.clone();
                let last_mouse_y = last_mouse_y.clone();
                let mouse_right_captured = mouse_right_captured.clone();
                let mouse_middle_captured = mouse_middle_captured.clone();
                let locked_cursor_x = locked_cursor_x.clone();
                let locked_cursor_y = locked_cursor_y.clone();
                let locked_cursor_screen_x = locked_cursor_screen_x.clone();
                let locked_cursor_screen_y = locked_cursor_screen_y.clone();
                let input_state_clone = self.input_state.clone();

move |event: &gpui::MouseDownEvent, window: &mut gpui::Window, _cx: &mut gpui::App| {
                    super::build_handlers::handle_middle_mouse_down(
                        event,
                        window,
                        last_mouse_x.clone(),
                        last_mouse_y.clone(),
                        mouse_right_captured.clone(),
                        mouse_middle_captured.clone(),
                        locked_cursor_x.clone(),
                        locked_cursor_y.clone(),
                        locked_cursor_screen_x.clone(),
                        locked_cursor_screen_y.clone(),
                        input_state_clone.clone(),
                    );
                }
            })
            // Middle-click release
            .on_mouse_up(gpui::MouseButton::Middle, {
                let last_mouse_x = last_mouse_x.clone();
                let last_mouse_y = last_mouse_y.clone();
                let locked_cursor_x = locked_cursor_x.clone();
                let locked_cursor_y = locked_cursor_y.clone();
                let mouse_right_captured = mouse_right_captured.clone();
                let mouse_middle_captured = mouse_middle_captured.clone();
                let locked_cursor_screen_x = locked_cursor_screen_x.clone();
                let locked_cursor_screen_y = locked_cursor_screen_y.clone();

move |_event, _window, _cx| {
                    super::build_handlers::handle_middle_mouse_up(
                        last_mouse_x.clone(),
                        last_mouse_y.clone(),
                        locked_cursor_x.clone(),
                        locked_cursor_y.clone(),
                        mouse_right_captured.clone(),
                        mouse_middle_captured.clone(),
                        locked_cursor_screen_x.clone(),
                        locked_cursor_screen_y.clone(),
                    );
                }
            })
            // Scroll wheel for camera speed adjustment
            .on_scroll_wheel({
                let mouse_right_captured = mouse_right_captured.clone();
                let mouse_middle_captured = mouse_middle_captured.clone();
                let input_state_scroll = self.input_state.clone();

move |event: &gpui::ScrollWheelEvent, _phase, _cx| {
                    super::build_handlers::handle_scroll_wheel(
                        event,
                        mouse_right_captured.clone(),
                        mouse_middle_captured.clone(),
                        input_state_scroll.clone(),
                    );
                }
            })
            // Left-click for object selection
            .on_mouse_down(gpui::MouseButton::Left, {
                let pointer_events = pointer_events_for_click.clone();
                let element_bounds = element_bounds_for_click.clone();
                let mouse_right_captured = mouse_right_captured.clone();
                let mouse_middle_captured = mouse_middle_captured.clone();
                let state_arc_click = state_arc.clone();
                let gpu_engine_click = gpu_engine.clone();

move |event: &gpui::MouseDownEvent,
                      window: &mut gpui::Window,
                      _cx: &mut gpui::App| {
                    super::build_handlers::handle_left_mouse_down(
                        event,
                        window,
                        pointer_events.clone(),
                        element_bounds.clone(),
                        mouse_right_captured.clone(),
                        mouse_middle_captured.clone(),
                        state_arc_click.clone(),
                        gpu_engine_click.clone(),
                    );
                }
            })
            // Left-click release
            .on_mouse_up(gpui::MouseButton::Left, {
                let pointer_events = pointer_events_for_click.clone();
                let state_arc_up = state_arc.clone();
                let gpu_engine_up = gpu_engine.clone();

move |event: &gpui::MouseUpEvent,
                      _window: &mut gpui::Window,
                      _cx: &mut gpui::App| {
                    super::build_handlers::handle_left_mouse_up(
                        event,
                        pointer_events.clone(),
                        state_arc_up.clone(),
                        gpu_engine_up.clone(),
                    );
                }
            })
            .child(viewport_entity)
            // Overlays
            .child(self.render_overlays(
                state,
                state_arc,
                perf_snapshot,
                gpu_engine,
                cx,
            ))
    }
}
