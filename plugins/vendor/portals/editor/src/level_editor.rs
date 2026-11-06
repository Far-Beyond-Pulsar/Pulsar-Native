//! The portals' debug overlay in the level editor's viewport: the link
//! list (paired portals, problems), badges over each portal and the ghost
//! geometry of a selected direction, registered through the level editor's
//! extension points (`ui_level_editor::extend::viewport`). Shown from the
//! viewport's Overlays menu or by choosing the portal debug view
//! ([`portals_component::PORTAL_DEBUG_VIEW_MODE`]).

use std::sync::{Arc, Mutex};

use gpui::prelude::*;
use gpui::*;
use ui::ActiveTheme as _;
use ui::StyledExt as _;
use ui_level_editor::extend::SharedState;
use ui_level_editor::extend::viewport::{DebugOverlay, OverlayContext};

inventory::submit! {
    DebugOverlay {
        origin: pulsar_plugins::origin!(),
        id: "portal_links",
        menu_label: "Portal links",
        debug_mode: Some(portals_component::PORTAL_DEBUG_VIEW_MODE),
        panel: |context, cx| {
            render_portal_debug_overlay(context.state.clone(), context.gpu_engine, cx)
                .into_any_element()
        },
        perspective: |context| render_portal_badges(context.state.clone(), context.gpu_engine),
        hidden: |mailbox| {
            mailbox.set_view_payload(portals_component::PORTAL_DEBUG_TEXTS.key(), None);
            mailbox.set_view_payload(portals_component::PORTAL_DEBUG_GEOMETRY.key(), None);
        },
    }
}

fn render_portal_debug_overlay(
    state_arc: SharedState,
    gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    cx: &mut App,
) -> impl IntoElement {
    let state = state_arc.read();
    let world = state.scene.world();
    let pairing = portals_component::pair_portals(&world);
    let paired_count = pairing.pairs.len();
    let unlinked_count = engine_backend::scene::attachments::enabled_components::<
        portals_component::PortalComponent,
    >(&world)
    .filter(|(_, _, portal)| portal.enabled && portal.link_id == 0)
    .count();
    let disabled_count = engine_backend::scene::attachments::enabled_components::<
        portals_component::PortalComponent,
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
            portals_component::PortalProblem::Alone { link_id } => {
                format!("Link {link_id} · needs one more portal")
            }
            portals_component::PortalProblem::Crowded { link_id, count } => {
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
    state_arc: &SharedState,
    gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    source: portals_component::Portal,
    target: portals_component::Portal,
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
        mailbox.set_view_payload(
            portals_component::PORTAL_DEBUG_GEOMETRY.key(),
            Some(std::sync::Arc::new(ghost) as engine_backend::subsystems::render::ViewPayload),
        );
    }
}

/// Snapshot nearby content triangles (every kind's content geometry: the
/// meshes') and map them through the selected portal pair. This is a
/// deliberate, bounded editor-only inspection payload.
fn portal_ghost_geometry(
    world: &pulsar_scenedb::World,
    source: portals_component::Portal,
    target: portals_component::Portal,
) -> Vec<[f64; 3]> {
    use glam::{DMat4, DVec3};

    const MAX_GHOST_VERTICES: usize = 90_000;
    const RADIUS_SQUARED: f64 = 12_000.0;
    let Some(source_world) =
        engine_backend::scene::hierarchy_transform::world_matrix_of(world, source.owner)
    else {
        return Vec::new();
    };
    let Some(target_world) =
        engine_backend::scene::hierarchy_transform::world_matrix_of(world, target.owner)
    else {
        return Vec::new();
    };
    let map = source_world * DMat4::from_rotation_y(std::f64::consts::PI) * target_world.inverse();
    let target_position = target_world.transform_point3(DVec3::ZERO);
    let mut vertices = Vec::new();
    // Each object's placement, once: whether it is near, and where.
    let mut placements: std::collections::HashMap<pulsar_scenedb::Entity, Option<DMat4>> =
        Default::default();
    engine_backend::subsystems::render::helio_renderer::content_triangles(
        world,
        &mut |object, triangle| {
            let placement = *placements.entry(object).or_insert_with(|| {
                engine_backend::scene::hierarchy_transform::world_matrix_of(world, object)
                    .filter(|owner_world| {
                        owner_world
                            .transform_point3(DVec3::ZERO)
                            .distance_squared(target_position)
                            <= RADIUS_SQUARED
                    })
            });
            let Some(owner_world) = placement else {
                return true;
            };
            if vertices.len() + 3 > MAX_GHOST_VERTICES {
                return false;
            }
            for corner in triangle {
                let mapped = map.transform_point3(owner_world.transform_point3(corner.as_dvec3()));
                vertices.push(mapped.to_array());
            }
            true
        },
    );
    vertices
}

fn portal_name(world: &pulsar_scenedb::World, owner: pulsar_scenedb::Entity) -> String {
    use engine_backend::scene::SceneWorldExt as _;

    world
        .stable_id_of(owner)
        .and_then(|id| ui_level_editor::scene_edit::objects::get_object_name(world, id))
        .unwrap_or_else(|| "Unnamed portal".to_string())
}

fn render_portal_badges(
    state_arc: SharedState,
    gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
) {
    use glam::DVec3;

    let state = state_arc.read();
    let world = state.scene.world();
    let pairing = portals_component::pair_portals(&world);
    let mut paired_sides = std::collections::HashMap::new();
    for (a, b) in pairing.pairs {
        paired_sides.insert(a.instance, 'A');
        paired_sides.insert(b.instance, 'B');
    }
    let mut labels = Vec::new();
    for (instance, owner, portal) in engine_backend::scene::attachments::enabled_components::<
        portals_component::PortalComponent,
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
        labels.push(portals_component::PortalDebugText {
            world_position: matrix.transform_point3(anchor).to_array(),
            text: label,
            color,
        });
    }
    drop(world);
    drop(state);
    if let Some(mailbox) = gpu_engine
        .try_lock()
        .ok()
        .and_then(|engine| engine.editor_mailbox())
    {
        mailbox.set_view_payload(
            portals_component::PORTAL_DEBUG_TEXTS.key(),
            Some(std::sync::Arc::new(labels) as engine_backend::subsystems::render::ViewPayload),
        );
    }
}
