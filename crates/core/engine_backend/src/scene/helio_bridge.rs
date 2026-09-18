//! SceneDB's GPU-mirror attachment seam for Helio 3.0.
///
/// SceneDB owns all active world and asset state. Helio receives the
/// cloneable GPU mirror during renderer construction and consumes the mirror's
/// component buffers directly. This module contains no renderer scene, frame
/// projection, material table, subscription cache, or per-frame input
/// assembly.
use std::sync::Arc;

use helio_component::components::StaticMeshComponent;
use pulsar_scenedb::gpu::{
    BufferKey, EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};

use crate::scene::{Transform, Visibility, WorldSceneStore};

struct EditorMeshRow;

/// Author the GPU draw rows directly in SceneDB from the live mesh entities.
/// The object-batch pass reads these rows and the mesh ranges from the same
/// SceneDB mirror; no renderer object table or CPU frame cache is involved.
pub fn sync_static_mesh_rows(store: &mut WorldSceneStore) {
    let scene_db = store.scene_db_mut();
    let stale: Vec<_> = scene_db
        .world
        .query::<&EditorMeshRow>()
        .filter(|(entity, _)| scene_db.world.get::<StaticMeshComponent>(*entity).is_none())
        .map(|(entity, _)| entity)
        .collect();
    for entity in stale {
        scene_db
            .world
            .remove::<helio_pass_gbuffer::StaticObjectComponent>(entity);
        scene_db.world.remove::<EditorMeshRow>(entity);
    }
    tracing::info!(
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
    let entities: Vec<_> = scene_db
        .world
        .query::<&StaticMeshComponent>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in entities {
        tracing::debug!(
            entity = entity.index(),
            "[SceneDB render diagnostics] evaluating StaticMeshComponent"
        );
        let Some(transform) = scene_db.world.get::<Transform>(entity).copied() else {
            scene_db
                .world
                .remove::<helio_pass_gbuffer::StaticObjectComponent>(entity);
            continue;
        };
        if scene_db
            .world
            .get::<Visibility>(entity)
            .is_some_and(|v| !v.visible)
        {
            scene_db
                .world
                .remove::<helio_pass_gbuffer::StaticObjectComponent>(entity);
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
            scene_db
                .world
                .remove::<helio_pass_gbuffer::StaticObjectComponent>(entity);
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
            scene_db
                .world
                .remove::<helio_pass_gbuffer::StaticObjectComponent>(entity);
            continue;
        };
        if scene_db
            .world
            .get::<helio_pass_gbuffer::MaterialComponent>(entity)
            .is_none()
        {
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
        scene_db.world.insert(
            entity,
            helio_pass_gbuffer::StaticObjectComponent::new(
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
            ),
        );
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
    store: &mut WorldSceneStore,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
) -> GpuMirrorHandle {
    let scene_db = store.scene_db_mut();
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
    fn a_new_store_does_not_have_a_gpu_mirror() {
        let store = WorldSceneStore::new();
        assert!(!store.world().has_gpu_mirror());
    }
}
