use super::*;

pub(super) fn handle_mouse_move(
    event: &gpui::MouseMoveEvent,
    input_state_clone: Arc<InputState>,
    last_mouse_x: Arc<AtomicI32>,
    last_mouse_y: Arc<AtomicI32>,
    mouse_right_captured: Arc<AtomicBool>,
    mouse_middle_captured: Arc<AtomicBool>,
    state_arc_move: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine_move: Arc<Mutex<GpuRenderer>>,
    element_bounds_move: Rc<RefCell<Option<Bounds<Pixels>>>>,
    last_mouse_pos: Rc<RefCell<Option<(f32, f32)>>>,
) {
                    let capture =
                        ViewportCursorCapture::load(&mouse_right_captured, &mouse_middle_captured);
                    let is_rotating = capture == ViewportCursorCapture::Rotate;
                    let is_panning = capture == ViewportCursorCapture::Pan;

                    if is_rotating || is_panning {
                        #[cfg(target_os = "windows")]
                        {
                            // Cursor stays hidden on Windows
                        }

                        #[cfg(not(target_os = "windows"))]
                        {
                            let pos_x: f32 = event.position.x.into();
                            let pos_y: f32 = event.position.y.into();
                            let x = (pos_x * 1000.0) as i32;
                            let y = (pos_y * 1000.0) as i32;
                            input_state_clone.mouse_x.store(x, Ordering::Relaxed);
                            input_state_clone.mouse_y.store(y, Ordering::Relaxed);
                        }
                    } else {
                        let pos_x: f32 = event.position.x.into();
                        let pos_y: f32 = event.position.y.into();
                        let x = (pos_x * 1000.0) as i32;
                        let y = (pos_y * 1000.0) as i32;
                        last_mouse_x.store(x, Ordering::Relaxed);
                        last_mouse_y.store(y, Ordering::Relaxed);
                        input_state_clone.mouse_x.store(x, Ordering::Relaxed);
                        input_state_clone.mouse_y.store(y, Ordering::Relaxed);
                    }

                    // Handle overlay dragging
                    let mut state = state_arc_move.write();
                    if state.overlays.positions.is_dragging_camera {
                        if let Some((start_x, start_y)) = state.overlays.positions.camera_drag_start
                        {
                            let current_x: f32 = event.position.x.into();
                            let current_y: f32 = event.position.y.into();
                            let delta_x = current_x - start_x;
                            let delta_y = current_y - start_y;

                            // Camera overlay positioned from right edge with .right(px(value))
                            // When dragging right, delta_x is positive, but right value should decrease
                            state.overlays.positions.camera.0 =
                                (state.overlays.positions.camera.0 - delta_x).max(0.0);
                            state.overlays.positions.camera.1 =
                                (state.overlays.positions.camera.1 + delta_y).max(0.0);
                            state.overlays.positions.camera_drag_start =
                                Some((current_x, current_y));
                        }
                        return;
                    }

                    if state.overlays.positions.is_dragging_viewport {
                        if let Some((start_x, start_y)) =
                            state.overlays.positions.viewport_drag_start
                        {
                            let current_x: f32 = event.position.x.into();
                            let current_y: f32 = event.position.y.into();
                            let delta_x = current_x - start_x;
                            let delta_y = current_y - start_y;

                            // Viewport overlay is positioned from left edge, normal drag
                            state.overlays.positions.viewport.0 =
                                (state.overlays.positions.viewport.0 + delta_x).max(0.0);
                            state.overlays.positions.viewport.1 =
                                (state.overlays.positions.viewport.1 + delta_y).max(0.0);
                            state.overlays.positions.viewport_drag_start =
                                Some((current_x, current_y));
                        }
                        return;
                    }
                    drop(state);

                    // Update Helio mouse input
                    let bounds_opt = element_bounds_move.borrow();
                    let (norm_x, norm_y, viewport_width, viewport_height) =
                        if let Some(ref bounds) = *bounds_opt {
                            let origin_x: f32 = bounds.origin.x.into();
                            let origin_y: f32 = bounds.origin.y.into();
                            let width: f32 = bounds.size.width.into();
                            let height: f32 = bounds.size.height.into();
                            let pos_x: f32 = event.position.x.into();
                            let pos_y: f32 = event.position.y.into();
                            // Subtract viewport origin; event.position is window-relative.
                            let local_x = pos_x - origin_x;
                            let local_y = pos_y - origin_y;
                            (
                                (local_x / width).clamp(0.0, 1.0),
                                (local_y / height).clamp(0.0, 1.0),
                                width,
                                height,
                            )
                        } else {
                            return;
                        };
                    drop(bounds_opt);

                    let mut last_pos = last_mouse_pos.borrow_mut();
                    *last_pos = Some((norm_x, norm_y));
                    drop(last_pos);

                    // Read the live camera in the same non-blocking pass that
                    // forwards the move to Helio, so the tool-mode ray below
                    // is built from this frame's camera and not a stale one
                    // captured when the element tree was assembled.
                    let mut camera_state = None;
                    if let Ok(mut engine) = gpu_engine_move.try_lock() {
                        camera_state = engine.editor_camera_state();
                        if !is_rotating && !is_panning {
                            engine.handle_mouse_move(norm_x, norm_y);
                        }
                    }

                    // Tool-mode dispatch for continuous input. `Drag` is what
                    // makes a sculpt stroke continuous; `Hover` only refreshes
                    // the brush ring. Skipped entirely while the camera has
                    // the cursor captured -- that is a camera gesture, not an
                    // authoring one.
                    if is_rotating || is_panning {
                        return;
                    }
                    let kind = if event.pressed_button == Some(gpui::MouseButton::Left) {
                        crate::level_editor::tool_modes::PointerKind::Drag
                    } else {
                        crate::level_editor::tool_modes::PointerKind::Hover
                    };
                    dispatch_tool_pointer(
                        &state_arc_move,
                        &gpu_engine_move,
                        tool_camera_frame(camera_state),
                        (viewport_width, viewport_height),
                        kind,
                        event.pressed_button,
                        norm_x,
                        norm_y,
                        event.modifiers,
                    );
}

pub(super) fn handle_right_mouse_down(
    event: &gpui::MouseDownEvent,
    window: &mut gpui::Window,
    last_mouse_x: Arc<AtomicI32>,
    last_mouse_y: Arc<AtomicI32>,
    mouse_right_captured: Arc<AtomicBool>,
    mouse_middle_captured: Arc<AtomicBool>,
    locked_cursor_x: Arc<AtomicI32>,
    locked_cursor_y: Arc<AtomicI32>,
    locked_cursor_screen_x: Arc<AtomicI32>,
    locked_cursor_screen_y: Arc<AtomicI32>,
    input_state_clone: Arc<InputState>,
) {
                    if !crate::level_editor::ui::viewport::cursor::prepare_relative_mouse_mode() {
                        ViewportCursorCapture::Released
                            .store(&mouse_right_captured, &mouse_middle_captured);
                        crate::level_editor::ui::viewport::cursor::end_relative_mouse_mode();
                        crate::level_editor::ui::viewport::cursor::unlock_cursor();
                        return;
                    }

                    let shift_pressed = event.modifiers.shift;
                    let window_x: f32 = event.position.x.into();
                    let window_y: f32 = event.position.y.into();
                    let x = (window_x * 1000.0) as i32;
                    let y = (window_y * 1000.0) as i32;

                    if let Some((screen_x, screen_y)) =
                        crate::level_editor::ui::viewport::cursor::window_to_screen_position(
                            window, window_x, window_y,
                        )
                    {
                        locked_cursor_x.store(x, Ordering::Relaxed);
                        locked_cursor_y.store(y, Ordering::Relaxed);
                        locked_cursor_screen_x.store(screen_x, Ordering::Relaxed);
                        locked_cursor_screen_y.store(screen_y, Ordering::Relaxed);
                        crate::level_editor::ui::viewport::cursor::lock_cursor_to_window(window);
                    } else {
                        locked_cursor_x.store(x, Ordering::Relaxed);
                        locked_cursor_y.store(y, Ordering::Relaxed);
                        if let Some((sx, sy)) =
                            crate::level_editor::ui::viewport::cursor::get_cursor_position()
                        {
                            locked_cursor_screen_x.store(sx, Ordering::Relaxed);
                            locked_cursor_screen_y.store(sy, Ordering::Relaxed);
                        }
                        crate::level_editor::ui::viewport::cursor::lock_cursor_to_window(window);
                    }

                    last_mouse_x.store(x, Ordering::Relaxed);
                    last_mouse_y.store(y, Ordering::Relaxed);
                    input_state_clone.mouse_x.store(x, Ordering::Relaxed);
                    input_state_clone.mouse_y.store(y, Ordering::Relaxed);

                    let capture = if shift_pressed {
                        ViewportCursorCapture::Pan
                    } else {
                        ViewportCursorCapture::Rotate
                    };
                    capture.store(&mouse_right_captured, &mouse_middle_captured);

                    crate::level_editor::ui::viewport::cursor::begin_relative_mouse_mode();
}

pub(super) fn handle_right_mouse_up(
    last_mouse_x: Arc<AtomicI32>,
    last_mouse_y: Arc<AtomicI32>,
    locked_cursor_x: Arc<AtomicI32>,
    locked_cursor_y: Arc<AtomicI32>,
    mouse_right_captured: Arc<AtomicBool>,
    mouse_middle_captured: Arc<AtomicBool>,
    locked_cursor_screen_x: Arc<AtomicI32>,
    locked_cursor_screen_y: Arc<AtomicI32>,
) {
                    let restore_x = locked_cursor_screen_x.load(Ordering::Relaxed);
                    let restore_y = locked_cursor_screen_y.load(Ordering::Relaxed);
                    last_mouse_x.store(0, Ordering::Relaxed);
                    last_mouse_y.store(0, Ordering::Relaxed);
                    locked_cursor_x.store(0, Ordering::Relaxed);
                    locked_cursor_y.store(0, Ordering::Relaxed);
                    ViewportCursorCapture::Released
                        .store(&mouse_right_captured, &mouse_middle_captured);
                    locked_cursor_screen_x.store(0, Ordering::Relaxed);
                    locked_cursor_screen_y.store(0, Ordering::Relaxed);

                    crate::level_editor::ui::viewport::cursor::end_relative_mouse_mode();
                    crate::level_editor::ui::viewport::cursor::unlock_cursor();
                    if restore_x > 0 && restore_y > 0 {
                        crate::level_editor::ui::viewport::cursor::set_cursor_position(
                            restore_x, restore_y,
                        );
                    }
}

pub(super) fn handle_middle_mouse_down(
    event: &gpui::MouseDownEvent,
    window: &mut gpui::Window,
    last_mouse_x: Arc<AtomicI32>,
    last_mouse_y: Arc<AtomicI32>,
    mouse_right_captured: Arc<AtomicBool>,
    mouse_middle_captured: Arc<AtomicBool>,
    locked_cursor_x: Arc<AtomicI32>,
    locked_cursor_y: Arc<AtomicI32>,
    locked_cursor_screen_x: Arc<AtomicI32>,
    locked_cursor_screen_y: Arc<AtomicI32>,
    input_state_clone: Arc<InputState>,
) {
                    if !crate::level_editor::ui::viewport::cursor::prepare_relative_mouse_mode() {
                        ViewportCursorCapture::Released
                            .store(&mouse_right_captured, &mouse_middle_captured);
                        crate::level_editor::ui::viewport::cursor::end_relative_mouse_mode();
                        crate::level_editor::ui::viewport::cursor::unlock_cursor();
                        return;
                    }

                    let window_x: f32 = event.position.x.into();
                    let window_y: f32 = event.position.y.into();
                    let x = (window_x * 1000.0) as i32;
                    let y = (window_y * 1000.0) as i32;

                    if let Some((screen_x, screen_y)) =
                        crate::level_editor::ui::viewport::cursor::window_to_screen_position(
                            window, window_x, window_y,
                        )
                    {
                        locked_cursor_x.store(x, Ordering::Relaxed);
                        locked_cursor_y.store(y, Ordering::Relaxed);
                        locked_cursor_screen_x.store(screen_x, Ordering::Relaxed);
                        locked_cursor_screen_y.store(screen_y, Ordering::Relaxed);
                        crate::level_editor::ui::viewport::cursor::lock_cursor_to_window(window);
                    } else {
                        locked_cursor_x.store(x, Ordering::Relaxed);
                        locked_cursor_y.store(y, Ordering::Relaxed);
                        if let Some((sx, sy)) =
                            crate::level_editor::ui::viewport::cursor::get_cursor_position()
                        {
                            locked_cursor_screen_x.store(sx, Ordering::Relaxed);
                            locked_cursor_screen_y.store(sy, Ordering::Relaxed);
                        }
                        crate::level_editor::ui::viewport::cursor::lock_cursor_to_window(window);
                    }

                    last_mouse_x.store(x, Ordering::Relaxed);
                    last_mouse_y.store(y, Ordering::Relaxed);
                    input_state_clone.mouse_x.store(x, Ordering::Relaxed);
                    input_state_clone.mouse_y.store(y, Ordering::Relaxed);

                    // Middle mouse always pans along the current view plane.
                    ViewportCursorCapture::Pan
                        .store(&mouse_right_captured, &mouse_middle_captured);

                    crate::level_editor::ui::viewport::cursor::begin_relative_mouse_mode();
}

pub(super) fn handle_middle_mouse_up(
    last_mouse_x: Arc<AtomicI32>,
    last_mouse_y: Arc<AtomicI32>,
    locked_cursor_x: Arc<AtomicI32>,
    locked_cursor_y: Arc<AtomicI32>,
    mouse_right_captured: Arc<AtomicBool>,
    mouse_middle_captured: Arc<AtomicBool>,
    locked_cursor_screen_x: Arc<AtomicI32>,
    locked_cursor_screen_y: Arc<AtomicI32>,
) {
                    let restore_x = locked_cursor_screen_x.load(Ordering::Relaxed);
                    let restore_y = locked_cursor_screen_y.load(Ordering::Relaxed);
                    last_mouse_x.store(0, Ordering::Relaxed);
                    last_mouse_y.store(0, Ordering::Relaxed);
                    locked_cursor_x.store(0, Ordering::Relaxed);
                    locked_cursor_y.store(0, Ordering::Relaxed);
                    ViewportCursorCapture::Released
                        .store(&mouse_right_captured, &mouse_middle_captured);
                    locked_cursor_screen_x.store(0, Ordering::Relaxed);
                    locked_cursor_screen_y.store(0, Ordering::Relaxed);

                    crate::level_editor::ui::viewport::cursor::end_relative_mouse_mode();
                    crate::level_editor::ui::viewport::cursor::unlock_cursor();
                    if restore_x > 0 && restore_y > 0 {
                        crate::level_editor::ui::viewport::cursor::set_cursor_position(
                            restore_x, restore_y,
                        );
                    }
}

pub(super) fn handle_scroll_wheel(
    event: &gpui::ScrollWheelEvent,
    mouse_right_captured: Arc<AtomicBool>,
    mouse_middle_captured: Arc<AtomicBool>,
    input_state_scroll: Arc<InputState>,
) {
                    let scroll_delta: f32 = event.delta.pixel_delta(px(1.0)).y.into();

                    // Check if right-click is held (camera rotation mode)
                    let is_rotating =
                        ViewportCursorCapture::load(&mouse_right_captured, &mouse_middle_captured)
                            == ViewportCursorCapture::Rotate;

                    if is_rotating {
                        // Right-click held: adjust camera move speed
                        let speed_delta = scroll_delta * 0.5; // Scale for reasonable adjustment
                        input_state_scroll.adjust_move_speed(speed_delta);
                        tracing::info!("[VIEWPORT] 🎚️ Camera speed adjusted by {:.2}", speed_delta);
                    }
}

pub(super) fn handle_left_mouse_down(
    event: &gpui::MouseDownEvent,
    window: &mut gpui::Window,
    pointer_events: Option<Arc<Mutex<Vec<engine_backend::subsystems::render::PendingPointerEvent>>>>,
    element_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    mouse_right_captured: Arc<AtomicBool>,
    mouse_middle_captured: Arc<AtomicBool>,
    state_arc_click: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine_click: Arc<Mutex<GpuRenderer>>,
) {
                    if ViewportCursorCapture::load(&mouse_right_captured, &mouse_middle_captured)
                        .is_active()
                    {
                        return;
                    }

                    let bounds_opt = element_bounds.borrow();
                    // Normalize cursor to [0,1] relative to the viewport element.
                    // event.position is window-relative; subtract the viewport origin.
                    let (norm_x, norm_y, viewport_width, viewport_height) = if let Some(ref bounds) = *bounds_opt {
                        let origin_x: f32 = bounds.origin.x.into();
                        let origin_y: f32 = bounds.origin.y.into();
                        let width: f32 = bounds.size.width.into();
                        let height: f32 = bounds.size.height.into();
                        let pos_x: f32 = event.position.x.into();
                        let pos_y: f32 = event.position.y.into();
                        let local_x = pos_x - origin_x;
                        let local_y = pos_y - origin_y;
                        (
                            (local_x / width).clamp(0.0, 1.0),
                            (local_y / height).clamp(0.0, 1.0),
                            width,
                            height,
                        )
                    } else {
                        let window_size = window.viewport_size();
                        let pos_x: f32 = event.position.x.into();
                        let pos_y: f32 = event.position.y.into();
                        let width: f32 = window_size.width.into();
                        let height: f32 = window_size.height.into();
                        (
                            (pos_x / width).clamp(0.0, 1.0),
                            (pos_y / height).clamp(0.0, 1.0),
                            width,
                            height,
                        )
                    };

                    // Tool-mode dispatch: give the active mode first refusal on the
                    // click (design doc §4.5). LevelEdit always returns `PassThrough`,
                    // so this is byte-for-byte the prior behavior for today's default mode.
                    let camera = tool_camera_frame(
                        gpu_engine_click
                            .lock()
                            .ok()
                            .and_then(|e| e.editor_camera_state()),
                    );
                    let dispatch_result = dispatch_tool_pointer(
                        &state_arc_click,
                        &gpu_engine_click,
                        camera,
                        (viewport_width, viewport_height),
                        crate::level_editor::tool_modes::PointerKind::Down,
                        Some(gpui::MouseButton::Left),
                        norm_x,
                        norm_y,
                        event.modifiers,
                    );
                    if dispatch_result
                        == crate::level_editor::tool_modes::ToolPointerResult::Consumed
                    {
                        return;
                    }

                    if let Some(events) = &pointer_events {
                        if let Ok(mut events) = events.lock() {
                            tracing::info!(
                                "[VIEWPORT] Left click: screen=({:.1},{:.1}) norm=({:.4},{:.4})",
                                event.position.x,
                                event.position.y,
                                norm_x,
                                norm_y
                            );
                            events.push(engine_backend::subsystems::render::PendingPointerEvent::LeftClick {
                                norm_x,
                                norm_y,
                            });
                        }
                    }
}

pub(super) fn handle_left_mouse_up(
    event: &gpui::MouseUpEvent,
    pointer_events: Option<Arc<Mutex<Vec<engine_backend::subsystems::render::PendingPointerEvent>>>>,
    state_arc_up: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine_up: Arc<Mutex<GpuRenderer>>,
) {
                    let mut state = state_arc_up.write();
                    state.overlays.positions.is_dragging_camera = false;
                    state.overlays.positions.is_dragging_viewport = false;
                    state.overlays.positions.camera_drag_start = None;
                    state.overlays.positions.viewport_drag_start = None;
                    drop(state);

                    // Close the active tool-mode gesture before the release
                    // reaches the renderer. No ray is cast for `Up`.
                    let consumed = dispatch_tool_pointer(
                        &state_arc_up,
                        &gpu_engine_up,
                        crate::level_editor::tool_modes::CameraFrame::default(),
                        (0.0, 0.0),
                        crate::level_editor::tool_modes::PointerKind::Up,
                        Some(gpui::MouseButton::Left),
                        0.0,
                        0.0,
                        event.modifiers,
                    ) == crate::level_editor::tool_modes::ToolPointerResult::Consumed;
                    if consumed {
                        return;
                    }

                    // This push is the actual fix for the drag-release
                    // freeze: previously this was `gpu_engine_up.try_lock()`
                    // then `engine.handle_left_release()` directly -- a
                    // non-blocking lock with no retry, racing the render
                    // thread's own unconditional per-frame `gpu_engine.lock()`.
                    // A lost race silently dropped the release, which
                    // permanently wedged `EditorState::is_dragging()` (only
                    // `handle_left_release` ever calls `end_drag()`), gating
                    // out all future scene sync. This queue push can't lose
                    // that race -- there's no lock left to lose it on.
                    if let Some(events) = &pointer_events {
                        if let Ok(mut events) = events.lock() {
                            events.push(engine_backend::subsystems::render::PendingPointerEvent::LeftRelease);
                        }
                    }
}
