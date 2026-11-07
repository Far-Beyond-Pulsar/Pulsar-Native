//! Viewport panel UI construction: the per-frame element tree.

use super::*;
use rust_i18n::t;
use ui::menu::popup_menu::PopupMenu;

use crate::ui::actions::{
    AddObjectOfType, DeleteObject, DuplicateObject, FocusSelected, OpenEnvironmentConfig,
    OpenHighResScreenshot,
};

/// Map the context-menu pointer to the editor's ground plane. When the ray
/// misses that plane (for example, the camera is looking above the horizon),
/// keep placement in front of the camera at a useful, fixed distance.
fn world_position_on_ground_plane(
    screen: gpui::Point<gpui::Pixels>,
    bounds: gpui::Bounds<gpui::Pixels>,
    camera: engine_backend::subsystems::render::EditorCameraState,
) -> Option<[f64; 3]> {
    let width: f32 = bounds.size.width.into();
    let height: f32 = bounds.size.height.into();
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let sx: f32 = screen.x.into();
    let sy: f32 = screen.y.into();
    let ox: f32 = bounds.origin.x.into();
    let oy: f32 = bounds.origin.y.into();
    let ndc_x = ((sx - ox) / width) * 2.0 - 1.0;
    let ndc_y = 1.0 - ((sy - oy) / height) * 2.0;

    let (sin_yaw, cos_yaw) = camera.yaw.sin_cos();
    let (sin_pitch, cos_pitch) = camera.pitch.sin_cos();
    let forward = [sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch];
    let right = [cos_yaw, 0.0, sin_yaw];
    let up = [-sin_yaw * sin_pitch, cos_pitch, cos_yaw * sin_pitch];
    // Helio's editor camera uses a 45 degree vertical perspective FOV.
    let tan_half_fov = (45.0_f32.to_radians() * 0.5).tan();
    let aspect = width / height;
    let ray = [
        forward[0] + right[0] * ndc_x * tan_half_fov * aspect + up[0] * ndc_y * tan_half_fov,
        forward[1] + right[1] * ndc_x * tan_half_fov * aspect + up[1] * ndc_y * tan_half_fov,
        forward[2] + right[2] * ndc_x * tan_half_fov * aspect + up[2] * ndc_y * tan_half_fov,
    ];
    let origin = camera.position;
    let plane_distance = if ray[1] < -1e-5 {
        -origin[1] as f32 / ray[1]
    } else {
        f32::NAN
    };
    // If the camera is on the ground plane, the mathematical intersection is
    // the camera itself (distance zero), which made every menu create at 0,0,0.
    let distance = if plane_distance.is_finite() && plane_distance > 0.1 {
        plane_distance
    } else {
        10.0
    };
    Some([
        origin[0] + f64::from(ray[0] * distance),
        origin[1] + f64::from(ray[1] * distance),
        origin[2] + f64::from(ray[2] * distance),
    ])
}

impl ViewportPanel {
    /// Build the complete viewport UI.
    pub(super) fn build_viewport_ui<V>(
        &mut self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        snapshot: Option<EngineFrameSnapshot>,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        action_context: FocusHandle,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let _viewport_hovered = self.viewport_hovered.clone();
        let _element_bounds = self.element_bounds.clone();
        let viewport_entity = self.viewport.clone();

        if let Some(snapshot) = snapshot {
            self.cached_frame_snapshot = snapshot;
        }
        let snap = &self.cached_frame_snapshot;

        // Clone for event handlers
        let _input_state_scroll = Arc::clone(&self.input_state);
        let mouse_right_captured = self.mouse_right_captured.clone();
        let mouse_middle_captured = self.mouse_middle_captured.clone();
        // Retain the mailbox across busy renderer frames. Event delivery must
        // not depend on whether this particular UI rebuild could sample stats.
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
        let selected_object = state.scene.selected_object().is_some();
        let create_entries: Vec<(i32, String, String)> = crate::extend::create_entries()
            .into_iter()
            .filter_map(|entry| {
                entry
                    .menu_label
                    .map(|label| (entry.order, label(), entry.id.to_string()))
            })
            .collect();
        let build_context_menu = {
            let create_entries = create_entries.clone();
            let action_context = action_context.clone();
            Rc::new(
                move |mut menu: PopupMenu,
                      window: &mut Window,
                      cx: &mut Context<PopupMenu>,
                      world_position: Option<[f64; 3]>| {
                    menu = menu.action_context(action_context.clone());
                    menu = menu.submenu(t!("LevelEditor.Menu.Create"), window, cx, {
                        let create_entries = create_entries.clone();
                        let submenu_action_context = action_context.clone();
                        move |mut submenu, _, _| {
                            submenu = submenu.action_context(submenu_action_context.clone());
                            let mut last_group = None;
                            for (order, label, object_type) in &create_entries {
                                let group = order.div_euclid(100);
                                if last_group.is_some_and(|last| last != group) {
                                    submenu = submenu.separator();
                                }
                                last_group = Some(group);
                                submenu = submenu.menu(
                                    label.clone(),
                                    Box::new(AddObjectOfType {
                                        object_type: object_type.clone(),
                                        world_position: world_position,
                                    }),
                                );
                            }
                            submenu
                        }
                    });
                    menu = menu.submenu(t!("LevelEditor.Menu.Tools"), window, cx, {
                        let submenu_action_context = action_context.clone();
                        move |menu, _, _| {
                            menu.action_context(submenu_action_context.clone())
                                .menu(
                                    t!("LevelEditor.Menu.EnvironmentConfig"),
                                    Box::new(OpenEnvironmentConfig),
                                )
                                .menu(
                                    t!("LevelEditor.Menu.HighResScreenshot"),
                                    Box::new(OpenHighResScreenshot),
                                )
                        }
                    });
                    menu = menu.separator();
                    menu = menu.submenu(t!("LevelEditor.Menu.Go"), window, cx, {
                        let submenu_action_context = action_context.clone();
                        move |menu, _, _| {
                            menu.action_context(submenu_action_context.clone())
                                .menu_with_disabled(
                                    t!("LevelEditor.Menu.FocusSelected"),
                                    Box::new(FocusSelected),
                                    !selected_object,
                                )
                        }
                    });
                    menu.menu_with_disabled(
                        t!("LevelEditor.Menu.Duplicate"),
                        Box::new(DuplicateObject),
                        !selected_object,
                    )
                    .menu_with_disabled(
                        t!("LevelEditor.Menu.Delete"),
                        Box::new(DeleteObject),
                        !selected_object,
                    )
                },
            )
        };
        let context_menu = self.context_menu.clone();
        let placement_bounds = self.element_bounds.clone();
        let placement_gpu = gpu_engine.clone();
        let right_mouse_down_position = self.right_mouse_down_position.clone();
        let right_mouse_dragged = self.right_mouse_dragged.clone();
        let owner_view = cx.entity().entity_id();
        let show_context_menu = {
            let context_menu = context_menu.clone();
            let build_context_menu = build_context_menu.clone();
            Rc::new(move |position, window: &mut Window, cx: &mut gpui::App| {
                let world_position = placement_bounds.borrow().and_then(|bounds| {
                    let gpu = placement_gpu.lock().ok()?;
                    let camera = gpu.editor_camera_state()?;
                    world_position_on_ground_plane(position, bounds, camera)
                });
                let menu = PopupMenu::build(window, cx, |menu, window, cx| {
                    build_context_menu(menu, window, cx, world_position)
                });
                if menu.read(cx).is_empty() {
                    return;
                }
                window
                    .subscribe(&menu, cx, {
                        let context_menu = context_menu.clone();
                        let owner_view = owner_view.clone();
                        move |_, _: &gpui::DismissEvent, _, cx| {
                            context_menu.borrow_mut().take();
                            cx.notify(owner_view.clone());
                        }
                    })
                    .detach();
                *context_menu.borrow_mut() = Some((position, menu));
                cx.notify(owner_view.clone());
            })
        };
        let popup_element = match context_menu.borrow().clone() {
            Some((position, menu)) => deferred(
                anchored()
                    .position(position)
                    .snap_to_window_with_margin(px(8.0))
                    .anchor(gpui::Corner::TopLeft)
                    .child(div().occlude().child(menu)),
            )
            .with_priority(1)
            .into_any_element(),
            None => div().into_any_element(),
        };

        // This viewport's own settings (the shared state for the primary one,
        // which the caller already holds locked).
        let view_state = self.view_state.clone();
        let camera_mode = if self.own_view {
            view_state.read().editor.camera_mode
        } else {
            state.editor.camera_mode
        };
        // An orthographic view zooms with the wheel instead of flying.
        let ortho_zoom_camera = (camera_mode != crate::state::CameraMode::Perspective)
            .then(|| snap.camera_input.clone())
            .flatten();

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
                let right_mouse_down_position = right_mouse_down_position.clone();
                let right_mouse_dragged = right_mouse_dragged.clone();
                let state_arc_move = state_arc.clone();
                let view_state_move = view_state.clone();
                let pointer_events_move = snap.pointer_events.clone();

                move |event: &gpui::MouseMoveEvent, _window, _cx| {
                    if event.pressed_button == Some(gpui::MouseButton::Right) {
                        if let Some(start) = *right_mouse_down_position.borrow() {
                            let dx: f32 = (event.position.x - start.x).into();
                            let dy: f32 = (event.position.y - start.y).into();
                            if dx * dx + dy * dy > 9.0 {
                                right_mouse_dragged.store(true, Ordering::Release);
                            }
                        }
                    }
                    super::build_handlers::handle_mouse_move(
                        event,
                        input_state_clone.clone(),
                        last_mouse_x.clone(),
                        last_mouse_y.clone(),
                        mouse_right_captured.clone(),
                        mouse_middle_captured.clone(),
                        state_arc_move.clone(),
                        view_state_move.clone(),
                        gpu_engine_move.clone(),
                        pointer_events_move.clone(),
                        element_bounds_move.clone(),
                        last_mouse_pos.clone(),
                    );
                }
            })
            // Right drag remains the viewport's orbit gesture. A stationary
            // right click opens the context menu on release.
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
                let right_mouse_down_position = right_mouse_down_position.clone();
                let right_mouse_dragged = right_mouse_dragged.clone();
                move |event: &gpui::MouseDownEvent,
                      window: &mut gpui::Window,
                      _cx: &mut gpui::App| {
                    *right_mouse_down_position.borrow_mut() = Some(event.position);
                    right_mouse_dragged.store(false, Ordering::Release);
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
            .on_mouse_up(gpui::MouseButton::Right, {
                let last_mouse_x = last_mouse_x.clone();
                let last_mouse_y = last_mouse_y.clone();
                let locked_cursor_x = locked_cursor_x.clone();
                let locked_cursor_y = locked_cursor_y.clone();
                let mouse_right_captured = mouse_right_captured.clone();
                let mouse_middle_captured = mouse_middle_captured.clone();
                let locked_cursor_screen_x = locked_cursor_screen_x.clone();
                let locked_cursor_screen_y = locked_cursor_screen_y.clone();
                let right_mouse_down_position = right_mouse_down_position.clone();
                let right_mouse_dragged = right_mouse_dragged.clone();
                let show_context_menu = show_context_menu.clone();
                move |_event, window, cx| {
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
                    let down_position = right_mouse_down_position.borrow_mut().take();
                    if !right_mouse_dragged.swap(false, Ordering::AcqRel) {
                        if let Some(position) = down_position {
                            show_context_menu(position, window, cx);
                        }
                    }
                }
            })
            .on_mouse_up_out(gpui::MouseButton::Right, {
                let last_mouse_x = last_mouse_x.clone();
                let last_mouse_y = last_mouse_y.clone();
                let locked_cursor_x = locked_cursor_x.clone();
                let locked_cursor_y = locked_cursor_y.clone();
                let mouse_right_captured = mouse_right_captured.clone();
                let mouse_middle_captured = mouse_middle_captured.clone();
                let locked_cursor_screen_x = locked_cursor_screen_x.clone();
                let locked_cursor_screen_y = locked_cursor_screen_y.clone();
                let right_mouse_down_position = right_mouse_down_position.clone();
                let right_mouse_dragged = right_mouse_dragged.clone();
                let show_context_menu = show_context_menu.clone();
                move |_event, window, cx| {
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
                    let down_position = right_mouse_down_position.borrow_mut().take();
                    if !right_mouse_dragged.swap(false, Ordering::AcqRel) {
                        if let Some(position) = down_position {
                            show_context_menu(position, window, cx);
                        }
                    }
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

                move |event: &gpui::MouseDownEvent,
                      window: &mut gpui::Window,
                      _cx: &mut gpui::App| {
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
                let zoom_camera = ortho_zoom_camera.clone();

                move |event: &gpui::ScrollWheelEvent, _phase, _cx| {
                    super::build_handlers::handle_scroll_wheel(
                        event,
                        mouse_right_captured.clone(),
                        mouse_middle_captured.clone(),
                        input_state_scroll.clone(),
                        zoom_camera.clone(),
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
                let view_state_up = view_state.clone();
                let gpu_engine_up = gpu_engine.clone();

                move |event: &gpui::MouseUpEvent,
                      _window: &mut gpui::Window,
                      _cx: &mut gpui::App| {
                    super::build_handlers::handle_left_mouse_up(
                        event,
                        pointer_events.clone(),
                        state_arc_up.clone(),
                        view_state_up.clone(),
                        gpu_engine_up.clone(),
                    );
                }
            })
            .on_mouse_up_out(gpui::MouseButton::Left, {
                let pointer_events = pointer_events_for_click.clone();
                let state = state_arc.clone();
                let view_state = view_state.clone();
                let engine = gpu_engine.clone();
                move |event, _window, _cx| {
                    super::build_handlers::handle_left_mouse_up(
                        event,
                        pointer_events.clone(),
                        state.clone(),
                        view_state.clone(),
                        engine.clone(),
                    );
                }
            })
            .child(viewport_entity)
            .child({
                let queue = pointer_events_for_click.clone();
                let right = mouse_right_captured.clone();
                let middle = mouse_middle_captured.clone();
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _cx| {
                        window.on_mouse_event(
                            move |event: &gpui::MouseMoveEvent, phase, _window, _cx| {
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
                            },
                        );
                    },
                )
                .absolute()
                .size_full()
            })
            // Overlays
            .child(self.render_overlays(state, state_arc, gpu_engine, cx))
            .child(popup_element)
            .id("level-editor-viewport-context-menu")
    }
}
