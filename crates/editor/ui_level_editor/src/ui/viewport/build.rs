//! Viewport panel UI construction: the per-frame element tree.

use super::*;
use rust_i18n::t;
use ui::menu::popup_menu::PopupMenu;

use super::build_mouse::{
    bind_left_button, bind_middle_button, bind_mouse_move, bind_right_button, bind_scroll_wheel,
    pointer_tracking_canvas, selection_box_overlay, viewport_container, ContextMenuOpener,
    MouseHandles, PointerQueue,
};

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

/// The viewport's right-click menu while objects are selected: what can be
/// done to all of them (focus, duplicate, delete, put them in a folder,
/// deselect), then the usual level menu (`level_menu`) as a submenu.
fn build_selection_menu(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    state: &Arc<parking_lot::RwLock<LevelEditorState>>,
    selection: &[String],
    action_context: &FocusHandle,
    level_menu: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
) -> PopupMenu {
    use crate::ui::hierarchy::{folder_choices, group_selection_into_folder, move_selection_to};

    let (title, folders) = {
        let state = state.read();
        let title = match selection {
            [id] => {
                let world = state.scene.world();
                crate::scene_edit::objects::get_object_name(&world, id)
                    .unwrap_or_else(|| id.clone())
            }
            many => t!("LevelEditor.Selection.Count", count = many.len()).to_string(),
        };
        // A selected folder cannot take the selection into itself.
        let folders: Vec<(String, String)> = folder_choices(&state)
            .into_iter()
            .filter(|(id, _)| !selection.contains(id))
            .collect();
        (title, folders)
    };
    let handler = |run: fn(&mut LevelEditorState)| {
        let state = state.clone();
        move |_: &mut Window, _: &mut gpui::App| run(&mut state.write())
    };

    let menu = menu
        .action_context(action_context.clone())
        .label(title)
        .separator()
        .menu(
            t!("LevelEditor.Menu.FocusSelected"),
            Box::new(FocusSelected),
        )
        .menu(t!("LevelEditor.Menu.Duplicate"), Box::new(DuplicateObject))
        .menu(t!("LevelEditor.Menu.Delete"), Box::new(DeleteObject))
        .separator()
        .menu_handler(
            t!("LevelEditor.Selection.GroupIntoFolder"),
            handler(|state| {
                group_selection_into_folder(state);
            }),
        )
        .submenu(t!("LevelEditor.Selection.MoveToFolder"), window, cx, {
            let state = state.clone();
            move |mut submenu, _, _| {
                let new_folder = state.clone();
                submenu = submenu
                    .menu_handler(t!("LevelEditor.Selection.NewFolder"), move |_, _| {
                        group_selection_into_folder(&mut new_folder.write());
                    })
                    .separator();
                let top = state.clone();
                submenu =
                    submenu.menu_handler(t!("LevelEditor.Selection.TopLevel"), move |_, _| {
                        move_selection_to(&mut top.write(), None);
                    });
                if !folders.is_empty() {
                    submenu = submenu.separator();
                }
                for (id, path) in &folders {
                    let state = state.clone();
                    let id = id.clone();
                    submenu = submenu.menu_handler(path.clone(), move |_, _| {
                        move_selection_to(&mut state.write(), Some(id.clone()));
                    });
                }
                submenu
            }
        })
        .separator()
        .submenu(t!("LevelEditor.Origin.Menu"), window, cx, {
            let state = state.clone();
            let several = selection.len() > 1;
            move |mut submenu, window, cx| {
                use crate::scene_edit::origin::{GroupOrigin, ObjectOrigin};
                let each = {
                    let state = state.clone();
                    move |origin: ObjectOrigin| {
                        let state = state.clone();
                        move |_: &mut Window, _: &mut gpui::App| {
                            crate::ui::hierarchy::set_selection_origins(&mut state.write(), origin);
                        }
                    }
                };
                let group = {
                    let state = state.clone();
                    move |origin: GroupOrigin| {
                        let state = state.clone();
                        move |_: &mut Window, _: &mut gpui::App| {
                            crate::ui::hierarchy::set_group_origin(&mut state.write(), origin);
                        }
                    }
                };
                // With several objects selected, "Each Object" and "Group" are
                // separate lists; with one, the object's own list is the menu.
                let own: [(&str, ObjectOrigin); 5] = [
                    (
                        "LevelEditor.Origin.BoundsCenter",
                        ObjectOrigin::BoundsCenter,
                    ),
                    (
                        "LevelEditor.Origin.BoundsBottom",
                        ObjectOrigin::BoundsBottom,
                    ),
                    ("LevelEditor.Origin.BoundsTop", ObjectOrigin::BoundsTop),
                    ("LevelEditor.Origin.World", ObjectOrigin::World),
                    ("LevelEditor.Origin.Transform", ObjectOrigin::Transform),
                ];
                if !several {
                    for (key, origin) in own {
                        submenu = submenu.menu_handler(t!(key), each(origin));
                    }
                    return submenu;
                }
                let own_menu =
                    move |mut menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                        for (key, origin) in own {
                            menu = menu.menu_handler(t!(key), each(origin));
                        }
                        menu
                    };
                let group_menu =
                    move |mut menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                        let entries: [(&str, GroupOrigin); 6] = [
                            ("LevelEditor.Origin.MedianCenter", GroupOrigin::MedianCenter),
                            (
                                "LevelEditor.Origin.GroupBoundsCenter",
                                GroupOrigin::BoundsCenter,
                            ),
                            (
                                "LevelEditor.Origin.GroupBoundsBottom",
                                GroupOrigin::BoundsBottom,
                            ),
                            ("LevelEditor.Origin.GroupBoundsTop", GroupOrigin::BoundsTop),
                            ("LevelEditor.Origin.Primary", GroupOrigin::Primary),
                            ("LevelEditor.Origin.World", GroupOrigin::World),
                        ];
                        for (key, origin) in entries {
                            menu = menu.menu_handler(t!(key), group(origin));
                        }
                        menu
                    };
                submenu = submenu.submenu(t!("LevelEditor.Origin.PerObject"), window, cx, own_menu);
                submenu.submenu(t!("LevelEditor.Origin.Group"), window, cx, group_menu)
            }
        })
        .separator();
    let menu = crate::ui::selection::select_submenu(menu, window, cx, state).separator();
    let level_context = action_context.clone();
    menu.submenu(
        t!("LevelEditor.Selection.Level"),
        window,
        cx,
        move |submenu, window, cx| {
            level_menu(submenu.action_context(level_context.clone()), window, cx)
        },
    )
}

type ContextMenuBuilder =
    Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>, Option<[f64; 3]>) -> PopupMenu>;
/// The entries of the viewport's "Create" submenu, as `(order, label, id)`.
fn create_menu_entries() -> Vec<(i32, String, String)> {
    crate::extend::create_entries()
        .into_iter()
        .filter_map(|entry| {
            entry
                .menu_label
                .map(|label| (entry.order, label(), entry.id.to_string()))
        })
        .collect()
}

/// The viewport's right-click menu when nothing is selected: create objects
/// at the clicked world position, tools, and actions on the selection.
fn context_menu_builder(
    create_entries: Vec<(i32, String, String)>,
    action_context: FocusHandle,
    selected_object: bool,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
) -> ContextMenuBuilder {
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
            menu = crate::ui::selection::select_submenu(menu, window, cx, &state);
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
}

/// The anchored popup that shows the open context menu, or an empty element.
fn context_menu_popup(
    context_menu: &Rc<RefCell<Option<(Point<Pixels>, Entity<PopupMenu>)>>>,
) -> AnyElement {
    match context_menu.borrow().clone() {
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
    }
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
        let viewport_entity = self.viewport.clone();

        if let Some(snapshot) = snapshot {
            self.cached_frame_snapshot = snapshot;
        }
        // Retain the mailbox across busy renderer frames. Event delivery must
        // not depend on whether this particular UI rebuild could sample stats.
        let pointer_events = self.cached_frame_snapshot.pointer_events.clone();
        let camera_input = self.cached_frame_snapshot.camera_input.clone();
        let owner_view = cx.entity().entity_id();

        let mouse = self.mouse_handles(pointer_events, &state_arc, gpu_engine, owner_view);
        let show_context_menu = self.context_menu_opener(
            context_menu_builder(
                create_menu_entries(),
                action_context.clone(),
                state.scene.selected_object().is_some(),
                state_arc.clone(),
            ),
            &state_arc,
            &action_context,
            gpu_engine,
            owner_view,
        );
        let popup_element = context_menu_popup(&self.context_menu);
        let ortho_zoom_camera = self.ortho_zoom_camera(state, &camera_input);
        // A Shift + drag selection box: drawn here, finished on release.
        let box_overlay = selection_box_overlay(
            &self.box_select,
            &self.element_bounds,
            cx.theme().foreground,
        );
        let cursor_capture =
            ViewportCursorCapture::load(&mouse.mouse_right_captured, &mouse.mouse_middle_captured);

        let container = viewport_container(
            cursor_capture,
            cx.theme().radius,
            self.element_bounds.clone(),
            camera_input,
        );
        let container = bind_mouse_move(container, &mouse);
        let container = bind_right_button(container, &mouse, &show_context_menu);
        let container = bind_middle_button(container, &mouse);
        let container = bind_scroll_wheel(container, &mouse, ortho_zoom_camera);
        bind_left_button(container, &mouse)
            .child(viewport_entity)
            .child(pointer_tracking_canvas(&mouse))
            // Overlays
            .child(box_overlay)
            .child(self.render_overlays(state, state_arc, gpu_engine, cx))
            .child(popup_element)
            .id("level-editor-viewport-context-menu")
    }

    /// Clone the shared input state the mouse handlers capture.
    fn mouse_handles(
        &self,
        pointer_events: PointerQueue,
        state_arc: &Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        owner_view: EntityId,
    ) -> MouseHandles {
        MouseHandles {
            input_state: self.input_state.clone(),
            last_mouse_x: self.last_mouse_x.clone(),
            last_mouse_y: self.last_mouse_y.clone(),
            locked_cursor_x: self.locked_cursor_x.clone(),
            locked_cursor_y: self.locked_cursor_y.clone(),
            locked_cursor_screen_x: self.locked_cursor_screen_x.clone(),
            locked_cursor_screen_y: self.locked_cursor_screen_y.clone(),
            mouse_right_captured: self.mouse_right_captured.clone(),
            mouse_middle_captured: self.mouse_middle_captured.clone(),
            right_mouse_down_position: self.right_mouse_down_position.clone(),
            right_mouse_dragged: self.right_mouse_dragged.clone(),
            element_bounds: self.element_bounds.clone(),
            box_select: self.box_select.clone(),
            pointer_events,
            state_arc: state_arc.clone(),
            gpu_engine: gpu_engine.clone(),
            owner_view,
        }
    }

    /// The camera input an orthographic view zooms with the wheel instead of
    /// flying; `None` for a perspective view.
    ///
    /// This viewport's own settings are in `view_state` (the shared state for
    /// the primary one, which the caller already holds locked as `state`).
    fn ortho_zoom_camera(
        &self,
        state: &LevelEditorState,
        camera_input: &Option<Arc<Mutex<engine_backend::subsystems::render::CameraInput>>>,
    ) -> Option<Arc<Mutex<engine_backend::subsystems::render::CameraInput>>> {
        let view_state = self.view_state.clone();
        let camera_mode = if self.own_view {
            view_state.read().editor.camera_mode
        } else {
            state.editor.camera_mode
        };
        (camera_mode != crate::state::CameraMode::Perspective)
            .then(|| camera_input.clone())
            .flatten()
    }

    /// The closure that opens the context menu at a pointer position: the
    /// selection menu when objects are selected, otherwise the create menu
    /// placed on the ground under the pointer.
    fn context_menu_opener(
        &self,
        build_context_menu: ContextMenuBuilder,
        state_arc: &Arc<parking_lot::RwLock<LevelEditorState>>,
        action_context: &FocusHandle,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        owner_view: EntityId,
    ) -> ContextMenuOpener {
        let context_menu = self.context_menu.clone();
        let placement_bounds = self.element_bounds.clone();
        let placement_gpu = gpu_engine.clone();
        let selection_state = state_arc.clone();
        let action_context = action_context.clone();
        Rc::new(move |position, window: &mut Window, cx: &mut gpui::App| {
            let world_position = placement_bounds.borrow().and_then(|bounds| {
                let gpu = placement_gpu.lock().ok()?;
                let camera = gpu.editor_camera_state()?;
                world_position_on_ground_plane(position, bounds, camera)
            });
            // With objects selected, the menu acts on them; the usual
            // level menu moves into a submenu.
            let selection = selection_state.read().scene.selected_objects();
            let menu = PopupMenu::build(window, cx, |menu, window, cx| {
                if selection.is_empty() {
                    build_context_menu(menu, window, cx, world_position)
                } else {
                    build_selection_menu(
                        menu,
                        window,
                        cx,
                        &selection_state,
                        &selection,
                        &action_context,
                        {
                            let build_context_menu = build_context_menu.clone();
                            move |menu, window, cx| {
                                build_context_menu(menu, window, cx, world_position)
                            }
                        },
                    )
                }
            });
            if menu.read(cx).is_empty() {
                return;
            }
            window
                .subscribe(&menu, cx, {
                    let context_menu = context_menu.clone();
                    move |_, _: &gpui::DismissEvent, _, cx| {
                        context_menu.borrow_mut().take();
                        cx.notify(owner_view);
                    }
                })
                .detach();
            *context_menu.borrow_mut() = Some((position, menu));
            cx.notify(owner_view);
        })
    }
}
