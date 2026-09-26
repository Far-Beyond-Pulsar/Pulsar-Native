//! SceneDB's GPU-mirror attachment seam for Helio 3.0.
///
/// SceneDB owns all active world and asset state. Helio receives the
/// cloneable GPU mirror during renderer construction and consumes the mirror's
/// component buffers directly. This module contains no renderer scene, frame
/// projection, material table, subscription cache, or per-frame input
/// assembly.
use std::{collections::HashSet, sync::Arc};

use helio_component::components::StaticMeshComponent;
use pulsar_scenedb::gpu::{
    BufferKey, EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};

use crate::scene::{Transform, Visibility};

/// Arm the change-time projection once for the currently live scene. Future
/// transform/material/visibility edits are delivered as entity-specific
/// events, so the renderer can update only the affected derived row.
pub fn arm_render_row_subscriptions(world: &mut pulsar_scenedb::World) {
    let mesh_entities: Vec<_> = world
        .query::<&StaticMeshComponent>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in mesh_entities {
        arm_render_row_subscriptions_for_entity(world, entity);
    }
    let light_entities: Vec<_> = world
        .query::<&helio_component::components::LightComponent>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in light_entities {
        arm_render_row_subscriptions_for_entity(world, entity);
    }
}

/// Arm subscriptions for one newly-created entity without revisiting the rest
/// of the scene. Structural editor commands call this immediately after spawn.
pub fn arm_render_row_subscriptions_for_entity(
    world: &mut pulsar_scenedb::World,
    entity: pulsar_scenedb::Entity,
) {
    if world.get::<StaticMeshComponent>(entity).is_some() {
        let _ = world.subscribe::<StaticMeshComponent>(entity);
        let _ = world.subscribe::<Transform>(entity);
        let _ = world.subscribe::<Visibility>(entity);
        let _ = world.subscribe::<helio_component::components::MaterialOverrideComponent>(entity);
    }
    if world
        .get::<helio_component::components::LightComponent>(entity)
        .is_some()
    {
        let _ = world.subscribe::<helio_component::components::LightComponent>(entity);
        let _ = world.subscribe::<Transform>(entity);
        let _ = world.subscribe::<Visibility>(entity);
    }
}

/// Report `entity`'s render-relevant components as changed to the render-row
/// subscriptions (#935), as a property edit through `Mut` would: its light
/// and mesh rows are re-derived at the renderer's next sync. Use after
/// components were (re)inserted before the entity's subscriptions were
/// armed (a class instance rebuilt from its class), which records no
/// change event. Also refreshes every registered class's GPU mirror.
pub fn mark_render_components_changed(world: &mut pulsar_scenedb::World, entity: pulsar_scenedb::Entity) {
    if !world.is_alive(entity) {
        return;
    }
    let classes: Vec<&'static str> = pulsar_world_registry::registered_world_component_classes().collect();
    for class_name in classes {
        if pulsar_world_registry::world_component_present_for_class(class_name, world, entity) {
            pulsar_world_registry::refresh_world_component_gpu_mirror_for_class(class_name, world, entity);
        }
    }
    if let Some(mut light) = world.get_mut::<helio_component::components::LightComponent>(entity) {
        std::ops::DerefMut::deref_mut(&mut light);
    }
    if let Some(mut mesh) = world.get_mut::<StaticMeshComponent>(entity) {
        std::ops::DerefMut::deref_mut(&mut mesh);
    }
    if let Some(mut transform) = world.get_mut::<Transform>(entity) {
        std::ops::DerefMut::deref_mut(&mut transform);
    }
}

struct EditorMeshRow;

/// Remove `entity`'s `StaticObjectComponent` row so it stops being drawn.
/// SceneDB zeroes a component's GPU row when the component is removed (or
/// its entity despawns), which the object-batch pass's `mesh_generation != 0`
/// liveness check then treats as dead.
fn retire_static_object_row(world: &mut pulsar_scenedb::World, entity: pulsar_scenedb::Entity) {
    world.remove::<helio_pass_gbuffer::StaticObjectComponent>(entity);
}

/// Retire the renderer rows derived for `entity`. `World::despawn` also
/// clears every `#[gpu]` row the entity carries, so calling this first is
/// no longer required for correctness; it remains the explicit hook for
/// dropping derived rows from an entity that stays alive.
pub fn retire_gpu_rows_for_entity(
    world: &mut pulsar_scenedb::World,
    entity: pulsar_scenedb::Entity,
) {
    retire_static_object_row(world, entity);
}

/// Author the GPU draw rows directly in SceneDB from the live mesh entities.
/// The object-batch pass reads these rows and the mesh ranges from the same
/// SceneDB mirror; no renderer object table or CPU frame cache is involved.
pub fn sync_static_mesh_rows(
    scene_db: &mut pulsar_scenedb::SceneDb,
    dirty: Option<&HashSet<pulsar_scenedb::Entity>>,
) {
    profiling::profile_scope!("HelioBridge::sync_static_mesh_rows");
    if dirty.is_none() {
        let stale: Vec<_> = scene_db
            .world
            .query::<&EditorMeshRow>()
            .filter(|(entity, _)| scene_db.world.get::<StaticMeshComponent>(*entity).is_none())
            .map(|(entity, _)| entity)
            .collect();
        for entity in stale {
            retire_static_object_row(&mut scene_db.world, entity);
            scene_db.world.remove::<EditorMeshRow>(entity);
        }
    }
    tracing::debug!(
        mesh_components = scene_db.world.query::<&StaticMeshComponent>().count(),
        object_rows = scene_db
            .world
            .query::<&helio_pass_gbuffer::StaticObjectComponent>()
            .count(),
        "[SceneDB render diagnostics] static sync entered"
    );
    let Some(mirror) = scene_db.world.gpu_mirror().cloned() else {
        return;
    };
    let entities: Vec<_> = dirty.map_or_else(
        || {
            scene_db
                .world
                .query::<&StaticMeshComponent>()
                .map(|(entity, _)| entity)
                .collect()
        },
        |dirty| dirty.iter().copied().collect(),
    );
    for entity in entities {
        if scene_db.world.get::<StaticMeshComponent>(entity).is_none() {
            retire_static_object_row(&mut scene_db.world, entity);
            scene_db.world.remove::<EditorMeshRow>(entity);
            continue;
        }
        tracing::debug!(
            entity = entity.index(),
            "[SceneDB render diagnostics] evaluating StaticMeshComponent"
        );
        let Some(transform) = scene_db.world.get::<Transform>(entity).copied() else {
            retire_static_object_row(&mut scene_db.world, entity);
            continue;
        };
        if scene_db
            .world
            .get::<Visibility>(entity)
            .is_some_and(|v| !v.visible)
        {
            retire_static_object_row(&mut scene_db.world, entity);
            continue;
        }
        // Real, geometry-derived local bounds (see `bounds_local`'s doc) --
        // computed once at hydrate time from the mesh's actual vertex
        // positions, not guessed from the transform's scale.
        let bounds_local = scene_db
            .world
            .get::<StaticMeshComponent>(entity)
            .map(|c| c.bounds_local)
            .unwrap_or([0.0, 0.0, 0.0, 0.5]);
        let Some(vertices) =
            StaticMeshComponent::vertices_gpu_handle(mirror.store(), entity.index())
                .filter(|r| r.count != 0)
        else {
            retire_static_object_row(&mut scene_db.world, entity);
            continue;
        };
        tracing::debug!(
            entity = entity.index(),
            vertex_offset = vertices.offset,
            vertex_count = vertices.count,
            "[SceneDB render diagnostics] vertex range resolved"
        );
        let Some(indices) = StaticMeshComponent::indices_gpu_handle(mirror.store(), entity.index())
            .filter(|r| r.count != 0)
        else {
            retire_static_object_row(&mut scene_db.world, entity);
            continue;
        };
        // A level-authored `MaterialOverrideComponent` defines the surface;
        // otherwise fall back to a default brown material that is only
        // inserted once. Rewrites are guarded so unchanged rows stay clean.
        let desired = match scene_db
            .world
            .get::<helio_component::components::MaterialOverrideComponent>(entity)
        {
            Some(o) => Some(helio_pass_gbuffer::MaterialComponent::from_surface(
                [o.base_color[0], o.base_color[1], o.base_color[2]],
                o.alpha,
                o.roughness,
                o.metallic,
                o.emissive_color,
                o.emissive_intensity,
            )),
            None => None,
        };
        let existing = scene_db
            .world
            .get::<helio_pass_gbuffer::MaterialComponent>(entity)
            .copied();
        match (desired, existing) {
            (Some(d), Some(e)) if d == e => {}
            (Some(d), _) => {
                scene_db.world.insert(entity, d);
            }
            (None, None) => {
                scene_db.world.insert(
                    entity,
                    helio_pass_gbuffer::MaterialComponent::new(
                        [0.22, 0.15, 0.08, 1.0],
                        0.7,
                        0.0,
                        [0.0; 3],
                        0.0,
                    ),
                );
            }
            (None, Some(_)) => {}
        }
        let model = glam::Mat4::from_scale_rotation_translation(
            glam::Vec3::from_array(transform.scale),
            glam::Quat::from_euler(
                glam::EulerRot::YXZ,
                transform.rotation[1].to_radians(),
                transform.rotation[0].to_radians(),
                transform.rotation[2].to_radians(),
            ),
            glam::Vec3::from_array(transform.position),
        );
        // World-space bounding sphere: transform the mesh's local-space
        // bounds center through `model`, and scale the local radius by the
        // largest axis scale factor -- conservative under non-uniform scale
        // (the scaled ellipsoid's farthest extent along its longest axis is
        // `radius * max_scale_component`, so a sphere of that radius fully
        // contains it, even though it isn't the tightest possible bound).
        let local_center =
            glam::Vec3::from_array([bounds_local[0], bounds_local[1], bounds_local[2]]);
        let world_center = model.transform_point3(local_center);
        let world_radius =
            bounds_local[3] * glam::Vec3::from_array(transform.scale).abs().max_element();
        let object_row = helio_pass_gbuffer::StaticObjectComponent::new(
            entity.index(),
            entity.generation().wrapping_add(1),
            entity.index(),
            entity.generation().wrapping_add(1),
            model,
            [world_center.x, world_center.y, world_center.z, world_radius],
            indices.count,
            indices.offset,
            vertices.offset as i32,
            0,
            0,
            0,
        );
        // Write only on change. Every `insert` bumps the SceneDB revision, and
        // that revision is what the status bar / hierarchy / properties panels
        // and the renderer's own idle check poll: an unconditional write here
        // made the world look edited on every render frame, dirtying those
        // panels and defeating idle detection.
        if scene_db
            .world
            .get::<helio_pass_gbuffer::StaticObjectComponent>(entity)
            != Some(&object_row)
        {
            scene_db.world.insert(entity, object_row);
        }
        if scene_db.world.get::<EditorMeshRow>(entity).is_none() {
            scene_db.world.insert(entity, EditorMeshRow);
        }
    }
}

/// Ensure that the authoritative SceneDB world has one GPU mirror for Helio's
/// renderer and return the shared handle used by RendererBuilder.
///
/// The mirror is attached before renderer construction. Component writes made
/// after attachment are mirrored by SceneDB itself; this helper does not keep
/// a second world representation or perform per-frame synchronization.
///
/// When a world already has a mirror, the existing handle is returned without
/// replacing its device, queue, pools, or residency state. This is required
/// when multiple renderer views share one SceneDB world.
///
/// A world populated before GPU initialization must be re-dispatched once
/// after attachment because SceneDB intentionally does not replay historical
/// writes into a mirror that did not exist yet. The values are copied only for
/// that one-time re-dispatch; they are never retained by the bridge.
pub fn ensure_gpu_mirror(
    scene_db: &mut pulsar_scenedb::SceneDb,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
) -> GpuMirrorHandle {
    if let Some(existing) = scene_db.world.gpu_mirror() {
        return existing.clone();
    }

    let existing_lights: Vec<_> = scene_db
        .world
        .query::<&helio_pass_forward_lit::LightComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_billboards: Vec<_> = scene_db
        .world
        .query::<&helio_pass_billboard::BillboardComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_transforms: Vec<_> = scene_db
        .world
        .query::<&Transform>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_materials: Vec<_> = scene_db
        .world
        .query::<&helio_pass_gbuffer::MaterialComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();

    let existing_static_meshes: Vec<_> = scene_db
        .world
        .query::<&StaticMeshComponent>()
        .map(|(entity, component)| (entity, component.clone()))
        .collect();
    let existing_decals: Vec<_> = scene_db
        .world
        .query::<&helio_pass_decal::DecalComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_water_volumes: Vec<_> = scene_db
        .world
        .query::<&helio_pass_water_sim::WaterVolumeComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_water_hitboxes: Vec<_> = scene_db
        .world
        .query::<&helio_pass_water_sim::WaterHitboxComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_groups: Vec<_> = scene_db
        .world
        .query::<&helio_pass_gbuffer::RenderGroupComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_sublevels: Vec<_> = scene_db
        .world
        .query::<&helio_pass_gbuffer::SublevelComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_sectioned_objects: Vec<_> = scene_db
        .world
        .query::<&helio_pass_gbuffer::SectionedObjectComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();

    let ctx = EngineGpuContext::new(device.clone(), queue.clone());
    let gpu_cfg = SceneGpuConfig {
        classes: vec![RegionClassConfig {
            capacity: 256,
            max_resident_cells: 4,
        }],
        tombstone_headroom: 8,
        max_cells_metadata: 16,
    };
    let mut gpu_store = SceneGpuStore::new(&ctx, gpu_cfg);
    // Register before the first write, as in the Cathedral/Billboard demos.
    helio_pass_forward_lit::LightComponent::register_gpu_columns_growable(
        &mut gpu_store,
        helio_pass_forward_lit::MAX_LIGHTS,
        &device,
    );
    helio_pass_billboard::BillboardComponent::register_gpu_columns_growable(
        &mut gpu_store,
        1024,
        &device,
    );

    // These are SceneDB component columns. Capacities are initial capacities
    // only; growable registration remains the sole owner of GPU storage and
    // deduplication behavior.
    StaticMeshComponent::register_gpu_columns_growable(&mut gpu_store, 4096, &device);
    helio_pass_decal::DecalComponent::register_gpu_columns_growable(&mut gpu_store, 256, &device);
    helio_pass_water_sim::WaterVolumeComponent::register_gpu_columns_growable(
        &mut gpu_store,
        64,
        &device,
    );
    helio_pass_water_sim::WaterHitboxComponent::register_gpu_columns_growable(
        &mut gpu_store,
        256,
        &device,
    );
    helio_pass_gbuffer::RenderGroupComponent::register_gpu_columns_growable(
        &mut gpu_store,
        256,
        &device,
    );
    helio_pass_gbuffer::SublevelComponent::register_gpu_columns_growable(
        &mut gpu_store,
        64,
        &device,
    );
    helio_pass_gbuffer::SectionedObjectComponent::register_gpu_columns_growable(
        &mut gpu_store,
        256,
        &device,
    );
    helio_pass_gbuffer::StaticObjectComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        &device,
    );
    helio_pass_gbuffer::MaterialComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        &device,
    );
    crate::scene::Transform::register_gpu_columns_growable(&mut gpu_store, 1024, &device);

    // SceneDB owns residency budgets and tier configuration. The bridge only
    // installs project settings while constructing the shared store.
    {
        let streaming = |key: &str| -> Option<engine_state::settings::ConfigValue> {
            engine_state::settings::global_config()
                .get(engine_state::settings::NS_PROJECT, "streaming", key)
                .ok()
        };
        let int_of = |value: Option<engine_state::settings::ConfigValue>| match value {
            Some(engine_state::settings::ConfigValue::Int(value)) => Some(value),
            _ => None,
        };
        let pool_bytes = int_of(streaming("texture_stream_pool_mb"))
            .unwrap_or(512)
            .clamp(64, 16_384) as u64
            * 1024
            * 1024;
        match gpu_store.configure_tiers(
            pulsar_scenedb::gpu::TierConfig {
                vram_budget_bytes: pool_bytes,
                ram_budget_bytes: pool_bytes.max(256 * 1024 * 1024),
            },
            &[],
        ) {
            Ok(()) => tracing::info!(
                "SceneDB tiers configured: VRAM budget {} MiB",
                pool_bytes / 1024 / 1024
            ),
            Err(error) => tracing::warn!("SceneDB tier configuration failed: {error}"),
        }
    }

    let mirror = GpuMirrorHandle::new(Arc::new(gpu_store), queue);
    scene_db.world.attach_gpu_mirror(mirror.clone());
    crate::scene::install_scenedb_inspector(&mut scene_db.world);

    for (entity, component) in existing_lights {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_billboards {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_transforms {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_materials {
        scene_db.world.insert(entity, component);
    }

    // Re-dispatch existing typed rows exactly once so the newly attached
    // mirror receives their component data. No row is retained after
    // insertion, and no Helio-side copy is created.
    for (entity, component) in existing_static_meshes {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_decals {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_water_volumes {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_water_hitboxes {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_groups {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_sublevels {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_sectioned_objects {
        scene_db.world.insert(entity, component);
    }

    tracing::info!("SceneDB GPU mirror attached for Helio 3.0");
    mirror
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_scene_does_not_have_a_gpu_mirror() {
        let scene_db = pulsar_scenedb::SceneDb::new();
        assert!(!scene_db.world.has_gpu_mirror());
    }
}
