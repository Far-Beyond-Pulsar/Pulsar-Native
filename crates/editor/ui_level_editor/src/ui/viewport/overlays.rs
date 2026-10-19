//! Floating viewport controls, performance HUDs and debug-view legends.

use super::*;

use crate::ui::toolbar::view_mode_dropdown::ViewModePickerState;
use rust_i18n::t;
use ui::button::Button;
use ui::IconName;
use ui::StyledExt as _;

impl ViewportPanel {
    /// The view-mode picker this viewport shows: its own when it was split off,
    /// else the toolbar's.
    fn view_picker(&self, cx: &App) -> Entity<ViewModePickerState> {
        match &self.own_picker {
            Some(own) => own.picker.clone(),
            None => self.toolbar.read(cx).view_mode_picker.clone(),
        }
    }

    fn view_picker_label(&self, cx: &App) -> SharedString {
        match &self.own_picker {
            Some(own) => own.label(cx),
            None => self.toolbar.read(cx).view_mode_label(cx),
        }
    }

    /// Shared state changes must invalidate the floating controls even when
    /// Helio only composites a new frame without rebuilding the UI.
    ///
    /// `state` is the editor's shared state (tool, snaps); the overlays, camera
    /// mode and debug view are this viewport's own.
    pub(crate) fn controls_signature(&self, state: &LevelEditorState, cx: &App) -> impl PartialEq {
        let editor = &state.editor;
        let view_guard = self.own_view.then(|| self.view_state.read());
        let view: &LevelEditorState = view_guard.as_deref().unwrap_or(state);
        let overlays = &view.overlays.state;
        let positions = &view.overlays.positions;
        (
            (
                editor.current_tool,
                view.editor.camera_mode,
                view.editor.viewport_debug_mode,
                editor.location_snap.to_bits(),
                editor.rotation_snap.to_bits(),
                editor.scale_snap.to_bits(),
                self.input_state.get_move_speed().to_bits(),
            ),
            (
                overlays.show_viewport_options,
                overlays.viewport_options_collapsed,
                overlays.show_camera_mode_selector,
                overlays.camera_mode_selector_collapsed,
                overlays.show_performance_overlay,
                overlays.show_gpu_pipeline_overlay,
                overlays.show_nerds_overlay,
                overlays.show_portal_debug_overlay,
                overlays.camera_sync_locked,
            ),
            (
                positions.viewport,
                positions.camera,
                positions.is_dragging_viewport,
                positions.is_dragging_camera,
            ),
            state
                .scene_hydration_progress
                .as_ref()
                .map(|progress| progress.load(std::sync::atomic::Ordering::Relaxed)),
            self.view_picker(cx).entity_id(),
        )
    }

    /// Render all viewport overlays.
    ///
    /// `state` and `state_arc` are the editor's shared state; this viewport's
    /// own settings are `self.view_state`.
    pub(super) fn render_overlays<V>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        // Pad the overlay container by the scrollbar width (12px) so overlays
        // don't sit underneath the scrollbar track.
        let scrollbar_width = px(12.0);
        let mut overlays = div()
            .absolute()
            .top_0()
            .left_0()
            .right(scrollbar_width)
            .bottom(scrollbar_width);

        let view_arc = self.view_state.clone();
        let view_guard = self.own_view.then(|| view_arc.read());
        let view: &LevelEditorState = view_guard.as_deref().unwrap_or(state);

        if let Some(progress) = &state.scene_hydration_progress {
            let progress = f32::from_bits(progress.load(std::sync::atomic::Ordering::Relaxed));
            overlays = overlays.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .w(px(280.0))
                            .rounded(px(8.0))
                            .p(px(16.0))
                            .bg(cx.theme().background)
                            .border_1()
                            .border_color(cx.theme().border)
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child("Hydrating scene…"),
                            )
                            .child(
                                div()
                                    .h(px(6.0))
                                    .w_full()
                                    .rounded_full()
                                    .bg(cx.theme().muted_foreground.opacity(0.25))
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(progress.clamp(0.0, 1.0)))
                                            .rounded_full()
                                            .bg(cx.theme().primary),
                                    ),
                            ),
                    ),
            );
        }

        // The picker owns the retained controls and their subscriptions.
        let picker = self.view_picker(cx);
        let view_mode_label = self.view_picker_label(cx);
        let snaps = self.toolbar.read(cx).snap_panels.clone();
        if view.overlays.state.show_viewport_options {
            overlays = overlays.child(
                div()
                    .absolute()
                    .top(px(view.overlays.positions.viewport.1))
                    .left(px(view.overlays.positions.viewport.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                    .child(components::viewport_options::render_viewport_options(
                        state,
                        state_arc.clone(),
                        view,
                        view_arc.clone(),
                        &snaps,
                        view.overlays.positions.is_dragging_viewport,
                        cx,
                    )),
            );
        }
        if view.overlays.state.show_camera_mode_selector {
            overlays = overlays.child(
                div()
                    .absolute()
                    .top(px(view.overlays.positions.camera.1))
                    .right(px(view.overlays.positions.camera.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                    .child(components::camera_selector::render_camera_selector(
                        view,
                        view_arc.clone(),
                        view.editor.camera_mode,
                        self.input_state.clone(),
                        &picker,
                        view_mode_label,
                        view.overlays.positions.is_dragging_camera,
                        cx,
                    )),
            );
        }

        if view.overlays.state.show_portal_debug_overlay {
            overlays = overlays.child(
                div()
                    .absolute()
                    .top(px(56.0))
                    .right_2()
                    .w(px(320.0))
                    .max_h(px(360.0))
                    .overflow_y_scroll()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                    .child(render_portal_debug_overlay(state_arc.clone(), gpu_engine, cx)),
            );
            if view.editor.camera_mode == crate::state::CameraMode::Perspective {
                overlays = overlays.child(
                    div()
                        .absolute()
                        .size_full()
                        .child(render_portal_badges(state_arc.clone(), gpu_engine)),
                );
            } else if let Some(mailbox) = gpu_engine
                .try_lock()
                .ok()
                .and_then(|engine| engine.editor_mailbox())
            {
                mailbox.set_portal_debug_texts(Vec::new());
                mailbox.set_portal_debug_geometry(Vec::new());
            }
        } else if let Some(mailbox) = gpu_engine
            .try_lock()
            .ok()
            .and_then(|engine| engine.editor_mailbox())
        {
            mailbox.set_portal_debug_texts(Vec::new());
            mailbox.set_portal_debug_geometry(Vec::new());
        }

        // What the overlay samples, on its own timer: the renderer's statistics
        // and the input latency.
        let sampler: Sampler = {
            let engine = gpu_engine.clone();
            let input = self.input_state.clone();
            std::rc::Rc::new(move |stats: &mut LiveStats| {
                if let Some(snapshot) = EngineFrameSnapshot::read_stats(&engine) {
                    snapshot.record_into(stats);
                }
                stats.record(
                    Metric::InputLatencyMs,
                    input.get_input_latency_us() as f64 / 1000.0,
                );
            })
        };

        // Bottom-left: Performance overlay. A view with its own refresh timers,
        // not rebuilt here; see `components/performance_overlay`.
        if view.overlays.state.show_performance_overlay {
            overlays = overlays.child(div().absolute().bottom_2().left_2().max_w(px(400.0)).child(
                render_performance_overlay(
                    view,
                    view_arc.clone(),
                    &self.stats,
                    sampler,
                    &self.perf_overlay,
                    cx,
                ),
            ));
        } else {
            // Closed: drop the overlay so its timers stop.
            self.perf_overlay.borrow_mut().take();
        }

        // GPU Pipeline overlay - positions next to performance overlay if both visible
        if view.overlays.state.show_gpu_pipeline_overlay {
            let overlay_div = if view.overlays.state.show_performance_overlay {
                // Position to the right of performance overlay
                div().absolute().bottom_2().left(px(300.0)) // 400px width + 10px gap
            } else {
                // Take performance overlay's position
                div().absolute().bottom_2().left_2()
            };

            overlays = overlays.child(overlay_div.max_w(px(400.0)).child(
                render_gpu_pipeline_overlay(gpu_engine, &self.gpu_overlay, cx),
            ));
        } else {
            self.gpu_overlay.borrow_mut().take();
        }

        // Bottom-right: editor and renderer diagnostics. Its timer refreshes
        // the isolated overlay in place without invalidating the viewport.
        if view.overlays.state.show_nerds_overlay {
            overlays = overlays.child(
                div()
                    .absolute()
                    .bottom_2()
                    .right_2()
                    // The cached Nerds view is width-full. Give its absolute
                    // positioning wrapper a concrete width so that width-full
                    // resolves to the panel width instead of the whole editor.
                    .w(px(230.0))
                    .child(render_nerds_overlay(
                        view_arc.clone(),
                        gpu_engine,
                        self.input_state.clone(),
                        &self.nerds_overlay,
                        cx,
                    )),
            );
        } else {
            self.nerds_overlay.borrow_mut().take();
        }

        // Debug passes can advertise a floating scale for a mode. Query only
        // while a debug mode is selected; lit view mode has no legend.
        let active_legend = (view.editor.viewport_debug_mode != 0)
            .then(|| {
                gpu_engine
                    .lock()
                    .ok()
                    .and_then(|engine| engine.editor_mailbox())
                    .and_then(|mailbox| {
                        mailbox
                            .viewport_debug_views()
                            .into_iter()
                            .find(|view_mode| {
                                view_mode.debug_mode == view.editor.viewport_debug_mode
                            })
                            .and_then(|view_mode| view_mode.legend)
                    })
            })
            .flatten();
        if let Some(legend) = active_legend {
            overlays = overlays.child(
                div()
                    .absolute()
                    .bottom_4()
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(components::debug_view_legend::render_debug_view_legend(
                        &legend,
                        cx.theme(),
                    )),
            );
        }

        // A split-off viewport can leave the camera sync: locked, its camera
        // moves with the others; unlocked, it is its own.
        if self.own_view {
            let locked = view.overlays.state.camera_sync_locked;
            let state = view_arc.clone();
            overlays = overlays.child(
                div()
                    .absolute()
                    .top_2()
                    .left(relative(0.5))
                    .ml(px(-16.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new("viewport_camera_sync")
                            .icon(if locked {
                                IconName::Link
                            } else {
                                IconName::LinkSlash
                            })
                            .small()
                            .tooltip(if locked {
                                t!("LevelEditor.Viewport.CameraLocked")
                            } else {
                                t!("LevelEditor.Viewport.CameraUnlocked")
                            })
                            .on_click(move |_, _, _| {
                                let mut state = state.write();
                                let locked = state.overlays.state.camera_sync_locked;
                                state.overlays.state.camera_sync_locked = !locked;
                            }),
                    ),
            );
        }

        overlays
    }
}

fn render_portal_debug_overlay<V>(
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    cx: &mut Context<V>,
) -> impl IntoElement
where
    V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
{
    let state = state_arc.read();
    let world = state.scene.world();
    let pairing = engine_backend::scene::portals::pair_portals(&world);
    let paired_count = pairing.pairs.len();
    let unlinked_count = engine_backend::scene::attachments::enabled_components::<
        helio_component::components::PortalComponent,
    >(&world)
    .filter(|(_, _, portal)| portal.enabled && portal.link_id == 0)
    .count();
    let disabled_count = engine_backend::scene::attachments::enabled_components::<
        helio_component::components::PortalComponent,
    >(&world)
    .filter(|(_, _, portal)| !portal.enabled)
    .count();
    let issue_count = pairing.problems.len() + unlinked_count + disabled_count;

    let mut rows = Vec::new();
    for (portal_a, portal_b) in pairing.pairs {
        let name_a = portal_name(&world, portal_a.owner);
        let name_b = portal_name(&world, portal_b.owner);
        let select_state = state_arc.clone();
        let select_engine = gpu_engine.clone();
        let select_source = portal_a;
        let select_target = portal_b;
        rows.push(
            div()
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().sidebar.opacity(0.55)))
                .child(
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(cx.theme().primary.opacity(0.22))
                        .text_color(cx.theme().primary)
                        .text_xs()
                        .child(format!("L{}", portal_a.link_id)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().foreground)
                        .child(format!("Through A → B  ·  {name_b}")),
                )
                .on_click(move |_, _, _| {
                    select_portal_debug_view(
                        &select_state,
                        &select_engine,
                        select_source,
                        select_target,
                    );
                }),
        );
        let select_state = state_arc.clone();
        let select_engine = gpu_engine.clone();
        let select_source = portal_b;
        let select_target = portal_a;
        rows.push(
            div()
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().sidebar.opacity(0.55)))
                .child(
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(cx.theme().primary.opacity(0.22))
                        .text_color(cx.theme().primary)
                        .text_xs()
                        .child(format!("L{}", portal_b.link_id)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().foreground)
                        .child(format!("Through B → A  ·  {name_a}")),
                )
                .on_click(move |_, _, _| {
                    select_portal_debug_view(
                        &select_state,
                        &select_engine,
                        select_source,
                        select_target,
                    );
                }),
        );
    }

    for problem in pairing.problems {
        let message = match problem {
            engine_backend::scene::portals::PortalProblem::Alone { link_id } => {
                format!("Link {link_id} · needs one more portal")
            }
            engine_backend::scene::portals::PortalProblem::Crowded { link_id, count } => {
                format!("Link {link_id} · {count} portals; needs exactly 2")
            }
        };
        rows.push(
            div()
                .text_xs()
                .text_color(gpui::rgb(0xfbbf24))
                .child(message),
        );
    }

    if unlinked_count > 0 {
        rows.push(
            div()
                .text_xs()
                .text_color(gpui::rgb(0xfbbf24))
                .child(format!("{unlinked_count} portal(s) use Link ID 0")),
        );
    }
    if disabled_count > 0 {
        rows.push(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("{disabled_count} portal(s) are disabled")),
        );
    }

    if rows.is_empty() {
        rows.push(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("No linked portals. Pair two portals with the same nonzero Link ID."),
        );
    }

    div()
        .rounded(px(6.0))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background.opacity(0.94))
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child("PORTAL LINKS"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(if issue_count == 0 {
                            gpui::rgb(0x4ade80)
                        } else {
                            gpui::rgb(0xfbbf24)
                        })
                        .child(format!("{paired_count} paired · {issue_count} issues")),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Each opening shows the scene from its partner."),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Select a direction to reveal its mapped geometry."),
        )
        .children(rows)
}

fn select_portal_debug_view(
    state_arc: &Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    source: engine_backend::scene::portals::Portal,
    target: engine_backend::scene::portals::Portal,
) {
    use engine_backend::scene::SceneWorldExt as _;
    let mut state = state_arc.write();
    let world = state.scene.world();
    let selected_id = world.stable_id_of(source.owner).map(str::to_owned);
    let ghost = portal_ghost_geometry(&world, source, target);
    drop(world);
    state.scene.select_object(selected_id);
    drop(state);
    if let Some(mailbox) = gpu_engine
        .try_lock()
        .ok()
        .and_then(|engine| engine.editor_mailbox())
    {
        mailbox.set_portal_debug_geometry(ghost);
    }
}

/// Snapshot nearby static triangles and map them through the selected portal
/// pair. This is a deliberate, bounded editor-only inspection payload.
fn portal_ghost_geometry(
    world: &pulsar_scenedb::World,
    source: engine_backend::scene::portals::Portal,
    target: engine_backend::scene::portals::Portal,
) -> Vec<[f64; 3]> {
    use glam::{DMat4, DVec3};
    use helio_component::components::StaticMeshComponent;

    const MAX_GHOST_VERTICES: usize = 90_000;
    const RADIUS_SQUARED: f64 = 12_000.0;
    let Some(source_world) = engine_backend::scene::hierarchy_transform::world_matrix_of(world, source.owner) else {
        return Vec::new();
    };
    let Some(target_world) = engine_backend::scene::hierarchy_transform::world_matrix_of(world, target.owner) else {
        return Vec::new();
    };
    let map = source_world * DMat4::from_rotation_y(std::f64::consts::PI) * target_world.inverse();
    let target_position = target_world.transform_point3(DVec3::ZERO);
    let mut vertices = Vec::new();
    'meshes: for (_instance, owner, component) in engine_backend::scene::attachments::enabled_components::<StaticMeshComponent>(world) {
        let Some(owner_world) = engine_backend::scene::hierarchy_transform::world_matrix_of(world, owner) else { continue; };
        let center = owner_world.transform_point3(DVec3::ZERO);
        if center.distance_squared(target_position) > RADIUS_SQUARED { continue; }
        for triangle in component.indices.chunks_exact(3) {
            if vertices.len() + 3 > MAX_GHOST_VERTICES { break 'meshes; }
            let Some(triangle_vertices) = triangle
                .iter()
                .map(|index| component.vertices.get(*index as usize))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            for vertex in triangle_vertices {
                let local = DVec3::new(
                    f64::from(vertex.position[0]),
                    f64::from(vertex.position[1]),
                    f64::from(vertex.position[2]),
                );
                let mapped = map.transform_point3(owner_world.transform_point3(local));
                vertices.push(mapped.to_array());
            }
        }
    }
    vertices
}

fn portal_name(world: &pulsar_scenedb::World, owner: pulsar_scenedb::Entity) -> String {
    use engine_backend::scene::SceneWorldExt as _;

    world
        .stable_id_of(owner)
        .and_then(|id| crate::scene_edit::objects::get_object_name(world, id))
        .unwrap_or_else(|| "Unnamed portal".to_string())
}

fn render_portal_badges(
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
) -> impl IntoElement {
    use glam::DVec3;

    let state = state_arc.read();
    let world = state.scene.world();
    let pairing = engine_backend::scene::portals::pair_portals(&world);
    let mut paired_sides = std::collections::HashMap::new();
    for (a, b) in pairing.pairs {
        paired_sides.insert(a.instance, 'A');
        paired_sides.insert(b.instance, 'B');
    }
    let mut labels = Vec::new();
    for (instance, owner, portal) in engine_backend::scene::attachments::enabled_components::<
        helio_component::components::PortalComponent,
    >(&world)
    {
        let Some(matrix) =
            engine_backend::scene::hierarchy_transform::world_matrix_of(&world, owner)
        else {
            continue;
        };
        let anchor = DVec3::new(
            -f64::from(portal.width) * 0.48,
            f64::from(portal.height) * 0.48,
            -0.02,
        );
        if world
            .get::<engine_backend::scene::Visibility>(owner)
            .is_some_and(|visibility| !visibility.visible)
        {
            continue;
        }
        let (label, color_id, issue) = if !portal.enabled {
            ("OFF".to_string(), 0, true)
        } else if portal.link_id == 0 {
            ("NO LINK".to_string(), 0, true)
        } else if let Some(side) = paired_sides.get(&instance) {
            (format!("{} {side}", portal.link_id), portal.link_id, false)
        } else {
            (format!("{} !", portal.link_id), portal.link_id, true)
        };
        let color = if issue {
            [0.96, 0.62, 0.04, 1.0]
        } else {
            [
                [0.38, 0.65, 0.98, 1.0],
                [0.20, 0.83, 0.60, 1.0],
                [0.98, 0.74, 0.13, 1.0],
                [0.96, 0.45, 0.71, 1.0],
                [0.65, 0.49, 0.98, 1.0],
                [0.13, 0.83, 0.93, 1.0],
            ][color_id.unsigned_abs() as usize % 6]
        };
        labels.push(
            engine_backend::subsystems::render::helio_renderer::PortalDebugText {
                world_position: matrix.transform_point3(anchor).to_array(),
                text: label,
                color,
            },
        );
    }
    drop(world);
    drop(state);
    if let Some(mailbox) = gpu_engine
        .try_lock()
        .ok()
        .and_then(|engine| engine.editor_mailbox())
    {
        mailbox.set_portal_debug_texts(labels);
    }

    gpui::canvas(|_, _, _| (), |_, _, _, _| ())
        .size_full()
        .into_any_element()
}
