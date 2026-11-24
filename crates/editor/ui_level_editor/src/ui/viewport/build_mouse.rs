//! The viewport's mouse input: the handlers bound onto its container and the
//! shared state they capture.

use super::*;

pub(super) type PointerQueue =
    Option<Arc<Mutex<Vec<engine_backend::subsystems::render::PendingPointerEvent>>>>;

pub(super) type ContextMenuOpener = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut gpui::App)>;

/// The shared input state the viewport's mouse handlers capture.
#[derive(Clone)]
pub(super) struct MouseHandles {
    pub(super) input_state: Arc<InputState>,
    pub(super) last_mouse_x: Arc<AtomicI32>,
    pub(super) last_mouse_y: Arc<AtomicI32>,
    pub(super) locked_cursor_x: Arc<AtomicI32>,
    pub(super) locked_cursor_y: Arc<AtomicI32>,
    pub(super) locked_cursor_screen_x: Arc<AtomicI32>,
    pub(super) locked_cursor_screen_y: Arc<AtomicI32>,
    pub(super) mouse_right_captured: Arc<AtomicBool>,
    pub(super) mouse_middle_captured: Arc<AtomicBool>,
    pub(super) right_mouse_down_position: Rc<RefCell<Option<Point<Pixels>>>>,
    pub(super) right_mouse_dragged: Arc<AtomicBool>,
    pub(super) element_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    pub(super) box_select: Rc<RefCell<Option<(Point<Pixels>, Point<Pixels>)>>>,
    pub(super) pointer_events: PointerQueue,
    pub(super) state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    pub(super) gpu_engine: Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    pub(super) owner_view: EntityId,
}

/// The Shift + drag selection box, drawn over the viewport while it is held.
pub(super) fn selection_box_overlay(
    box_select: &Rc<RefCell<Option<(Point<Pixels>, Point<Pixels>)>>>,
    element_bounds: &Rc<RefCell<Option<Bounds<Pixels>>>>,
    foreground: Hsla,
) -> AnyElement {
    match (*box_select.borrow(), *element_bounds.borrow()) {
        (Some((start, end)), Some(bounds)) => {
            let min = point(start.x.min(end.x), start.y.min(end.y)) - bounds.origin;
            let max = point(start.x.max(end.x), start.y.max(end.y)) - bounds.origin;
            div()
                .absolute()
                .left(min.x)
                .top(min.y)
                .w(max.x - min.x)
                .h(max.y - min.y)
                .bg(foreground.opacity(0.08))
                .border_1()
                .border_color(foreground.opacity(0.7))
                .into_any_element()
        }
        _ => div().into_any_element(),
    }
}

/// The viewport's container: transparent (Helio draws behind it), hides the
/// cursor while the camera is captured, and publishes its bounds.
pub(super) fn viewport_container(
    cursor_capture: ViewportCursorCapture,
    radius: Pixels,
    element_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    camera_input: Option<Arc<Mutex<engine_backend::subsystems::render::CameraInput>>>,
) -> gpui::Div {
    div()
        .size_full()
        .relative()
        // Cursor requests are paint state in GPUI. Event handlers update the
        // capture atomics and refresh the window; paint owns presentation.
        .when(cursor_capture.is_active(), |viewport| {
            viewport.cursor(CursorStyle::None)
        })
        // TRANSPARENT - no background for direct Helio rendering
        .rounded(radius)
        // CRITICAL: Capture element bounds and update Helio camera viewport.
        // Runs every frame, cached or not — geometry stashing belongs here,
        // not in on_children_prepainted (which is skipped on cache hits).
        .on_frame(move |geom, _window, _cx| {
            *element_bounds.borrow_mut() = Some(geom.bounds);

            // Update Helio camera viewport to match GPUI viewport bounds
            if let Some(cam) = &camera_input {
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
}

/// Track the pointer: drag the selection box and feed the camera input.
pub(super) fn bind_mouse_move(el: gpui::Div, m: &MouseHandles) -> gpui::Div {
    let last_mouse_pos = Rc::new(RefCell::new(None::<(f32, f32)>));
    el.on_mouse_move({
        let m = m.clone();
        move |event: &gpui::MouseMoveEvent, _window, cx| {
            let mut drawing_box = m.box_select.borrow_mut();
            if let Some((_, end)) = drawing_box.as_mut() {
                if event.pressed_button == Some(gpui::MouseButton::Left) {
                    *end = event.position;
                } else {
                    // The release was missed (focus loss): drop it.
                    *drawing_box = None;
                }
                cx.notify(m.owner_view);
                return;
            }
            drop(drawing_box);
            if event.pressed_button == Some(gpui::MouseButton::Right) {
                if let Some(start) = *m.right_mouse_down_position.borrow() {
                    let dx: f32 = (event.position.x - start.x).into();
                    let dy: f32 = (event.position.y - start.y).into();
                    if dx * dx + dy * dy > 9.0 {
                        m.right_mouse_dragged.store(true, Ordering::Release);
                    }
                }
            }
            super::build_handlers::handle_mouse_move(
                event,
                m.input_state.clone(),
                m.last_mouse_x.clone(),
                m.last_mouse_y.clone(),
                m.mouse_right_captured.clone(),
                m.mouse_middle_captured.clone(),
                m.state_arc.clone(),
                m.gpu_engine.clone(),
                m.pointer_events.clone(),
                m.element_bounds.clone(),
                last_mouse_pos.clone(),
            );
        }
    })
}

/// A right release: end the camera capture, then open the context menu when
/// the click did not drag.
pub(super) fn right_release_handler(
    m: MouseHandles,
    show_context_menu: ContextMenuOpener,
) -> impl Fn(&gpui::MouseUpEvent, &mut gpui::Window, &mut gpui::App) {
    move |_event, window, cx| {
        super::build_handlers::handle_right_mouse_up(
            m.last_mouse_x.clone(),
            m.last_mouse_y.clone(),
            m.locked_cursor_x.clone(),
            m.locked_cursor_y.clone(),
            m.mouse_right_captured.clone(),
            m.mouse_middle_captured.clone(),
            m.locked_cursor_screen_x.clone(),
            m.locked_cursor_screen_y.clone(),
        );
        let down_position = m.right_mouse_down_position.borrow_mut().take();
        if !m.right_mouse_dragged.swap(false, Ordering::AcqRel) {
            if let Some(position) = down_position {
                show_context_menu(position, window, cx);
            }
        }
    }
}

/// Right drag remains the viewport's orbit gesture. A stationary right click
/// opens the context menu on release.
pub(super) fn bind_right_button(
    el: gpui::Div,
    m: &MouseHandles,
    show_context_menu: &ContextMenuOpener,
) -> gpui::Div {
    el.on_mouse_down(gpui::MouseButton::Right, {
        let m = m.clone();
        move |event: &gpui::MouseDownEvent, window: &mut gpui::Window, _cx: &mut gpui::App| {
            *m.right_mouse_down_position.borrow_mut() = Some(event.position);
            m.right_mouse_dragged.store(false, Ordering::Release);
            super::build_handlers::handle_right_mouse_down(
                event,
                window,
                m.last_mouse_x.clone(),
                m.last_mouse_y.clone(),
                m.mouse_right_captured.clone(),
                m.mouse_middle_captured.clone(),
                m.locked_cursor_x.clone(),
                m.locked_cursor_y.clone(),
                m.locked_cursor_screen_x.clone(),
                m.locked_cursor_screen_y.clone(),
                m.input_state.clone(),
            );
        }
    })
    .on_mouse_up(
        gpui::MouseButton::Right,
        right_release_handler(m.clone(), show_context_menu.clone()),
    )
    .on_mouse_up_out(
        gpui::MouseButton::Right,
        right_release_handler(m.clone(), show_context_menu.clone()),
    )
}

/// Middle-click drag pans the camera along the current view plane.
pub(super) fn bind_middle_button(el: gpui::Div, m: &MouseHandles) -> gpui::Div {
    el.on_mouse_down(gpui::MouseButton::Middle, {
        let m = m.clone();
        move |event: &gpui::MouseDownEvent, window: &mut gpui::Window, _cx: &mut gpui::App| {
            super::build_handlers::handle_middle_mouse_down(
                event,
                window,
                m.last_mouse_x.clone(),
                m.last_mouse_y.clone(),
                m.mouse_right_captured.clone(),
                m.mouse_middle_captured.clone(),
                m.locked_cursor_x.clone(),
                m.locked_cursor_y.clone(),
                m.locked_cursor_screen_x.clone(),
                m.locked_cursor_screen_y.clone(),
                m.input_state.clone(),
            );
        }
    })
    // Middle-click release
    .on_mouse_up(gpui::MouseButton::Middle, {
        let m = m.clone();
        move |_event, _window, _cx| {
            super::build_handlers::handle_middle_mouse_up(
                m.last_mouse_x.clone(),
                m.last_mouse_y.clone(),
                m.locked_cursor_x.clone(),
                m.locked_cursor_y.clone(),
                m.mouse_right_captured.clone(),
                m.mouse_middle_captured.clone(),
                m.locked_cursor_screen_x.clone(),
                m.locked_cursor_screen_y.clone(),
            );
        }
    })
}

/// The scroll wheel adjusts the camera speed, or zooms an orthographic view.
pub(super) fn bind_scroll_wheel(
    el: gpui::Div,
    m: &MouseHandles,
    zoom_camera: Option<Arc<Mutex<engine_backend::subsystems::render::CameraInput>>>,
) -> gpui::Div {
    el.on_scroll_wheel({
        let m = m.clone();
        move |event: &gpui::ScrollWheelEvent, _phase, _cx| {
            super::build_handlers::handle_scroll_wheel(
                event,
                m.mouse_right_captured.clone(),
                m.mouse_middle_captured.clone(),
                m.input_state.clone(),
                zoom_camera.clone(),
            );
        }
    })
}

/// A left release: finish a selection box, then the pointer release.
pub(super) fn left_release_handler(
    m: MouseHandles,
) -> impl Fn(&gpui::MouseUpEvent, &mut gpui::Window, &mut gpui::App) {
    move |event, _window, cx| {
        if super::build_handlers::finish_box_select(
            &m.box_select,
            &m.element_bounds,
            &m.pointer_events,
            event.position,
        ) {
            cx.notify(m.owner_view);
        }
        super::build_handlers::handle_left_mouse_up(
            event,
            m.pointer_events.clone(),
            m.state_arc.clone(),
            m.gpu_engine.clone(),
        );
    }
}

/// Left click selects objects; its release finishes a selection box.
pub(super) fn bind_left_button(el: gpui::Div, m: &MouseHandles) -> gpui::Div {
    el.on_mouse_down(gpui::MouseButton::Left, {
        let m = m.clone();
        move |event: &gpui::MouseDownEvent, window: &mut gpui::Window, _cx: &mut gpui::App| {
            super::build_handlers::handle_left_mouse_down(
                event,
                window,
                m.pointer_events.clone(),
                m.element_bounds.clone(),
                m.mouse_right_captured.clone(),
                m.mouse_middle_captured.clone(),
                m.state_arc.clone(),
                m.gpu_engine.clone(),
                m.box_select.clone(),
            );
        }
    })
    .on_mouse_up(gpui::MouseButton::Left, left_release_handler(m.clone()))
    .on_mouse_up_out(gpui::MouseButton::Left, left_release_handler(m.clone()))
}

/// An invisible canvas over the viewport that keeps tracking the pointer
/// outside its edge: it drags a selection box (clamped to the viewport) and
/// queues pointer moves for the renderer.
pub(super) fn pointer_tracking_canvas(m: &MouseHandles) -> impl IntoElement {
    let queue = m.pointer_events.clone();
    let right = m.mouse_right_captured.clone();
    let middle = m.mouse_middle_captured.clone();
    let box_select = m.box_select.clone();
    let owner_view = m.owner_view;
    gpui::canvas(
        |_, _, _| (),
        move |bounds, (), window, _cx| {
            window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _window, cx| {
                // A selection box keeps following the pointer
                // past the viewport's edge, clamped to it.
                if phase == gpui::DispatchPhase::Capture && !bounds.contains(&event.position) {
                    if let Some((_, end)) = box_select.borrow_mut().as_mut() {
                        let max = bounds.bottom_right();
                        *end = point(
                            event.position.x.clamp(bounds.origin.x, max.x),
                            event.position.y.clamp(bounds.origin.y, max.y),
                        );
                        cx.notify(owner_view);
                        return;
                    }
                }
                if phase != gpui::DispatchPhase::Capture
                    || bounds.contains(&event.position)
                    || right.load(Ordering::Relaxed)
                    || middle.load(Ordering::Relaxed)
                {
                    return;
                }
                let x: f32 = (event.position.x - bounds.origin.x).into();
                let y: f32 = (event.position.y - bounds.origin.y).into();
                let w: f32 = bounds.size.width.into();
                let h: f32 = bounds.size.height.into();
                if let Some(queue) = &queue {
                    if let Ok(mut events) = queue.lock() {
                        use engine_backend::subsystems::render::PendingPointerEvent;
                        PendingPointerEvent::queue(
                            &mut events,
                            PendingPointerEvent::MouseMove {
                                norm_x: x / w.max(1.0),
                                norm_y: y / h.max(1.0),
                            },
                        );
                    }
                }
            });
        },
    )
    .absolute()
    .size_full()
}
