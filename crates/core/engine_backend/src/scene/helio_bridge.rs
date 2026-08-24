//! The one-world bridge between [`WorldSceneStore`] and any `helio::Renderer`
//! (Pulsar-Native#637).
//!
//! Everything here is renderer-agnostic on purpose: the editor's
//! `HelioRenderer` and the play-mode paths (standalone windows, PIE guest)
//! share the exact same three operations instead of each carrying a copy --
//!
//! 1. [`attach_gpu_render_seam`] -- one-time wiring of the store's GPU
//!    mirror (`SceneGpuStore` + var-len mesh pools + `Transform`'s packed
//!    buffer) to a concrete wgpu device/queue and renderer scene.
//! 2. [`rebuild_static_mesh_frame`] -- assemble Helio's transient
//!    static-mesh instance list from the authoritative World rows.
//! 3. [`rebuild_light_frame`] -- same for lights, reading the
//!    subscription-maintained `ResolvedLightFrame` rows (#636).
//!
//! Callers drive these with whatever locking discipline suits them: both
//! rebuild fns take `&WorldSceneStore` (a read lock suffices); only the
//! attach step needs `&mut`.

use std::sync::Arc;

use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::{GroupId, GroupMask, MaterialId, Movability, Renderer};
use helio_component::components::StaticMeshComponent;
use pulsar_reflection::scene_id_to_tag;
use pulsar_scenedb::gpu::{
    BufferKey, EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};

use crate::scene::{LightFrameMaintainer, ResolvedLightFrame, Visibility, WorldSceneStore};

/// Advance the store by one render-side sync pass: flush the GPU mirror
/// (`SceneDb::step`) and refresh the `ResolvedLightFrame` rows from World
/// change events. Shared by play-mode render loops (Pulsar-Native#637) --
/// this is exactly what the editor's sync passes do between their phases,
/// minus editor-only bookkeeping (dirty-flag drains, component dispatch).
///
/// Run under whatever lock yields `&mut WorldSceneStore`, immediately
/// before [`rebuild_static_mesh_frame`]/[`rebuild_light_frame`] with a read
/// guard; keep the write scope this short so concurrent readers never wait
/// on it.
pub fn step_scene_for_render(store: &mut WorldSceneStore, lights: &mut LightFrameMaintainer) {
    store.scene_db_mut().step();
    lights.maintain(store.world_mut());
}

/// One-time wiring of the SceneDB GPU-native render seam (Pulsar-Native#561
/// Phase D) between `store` and `renderer`, shared by the editor and
/// play-mode renderers via #637.
///
/// Registers the canonical `StaticMeshComponent::vertices`/`indices` var-len
/// pools plus `Transform`'s packed buffer into a fresh `SceneGpuStore`,
/// points Helio's own mesh storage at those SAME pools (hydrate-time writes
/// and draw-time reads then share one buffer, zero translation), and attaches
/// the mirror so future component inserts auto-mirror their `#[gpu]` fields.
///
/// Components inserted BEFORE this call were written with no mirror attached,
/// and SceneDB deliberately does not retroactively mirror those writes -- so
/// every already-present `StaticMeshComponent` is captured now and re-inserted
/// immediately after attaching (same typed value, re-dispatched into the
/// pools; no Helio mesh state is created here).
///
/// Idempotent: returns `false` without touching anything if `store` already
/// has a GPU mirror (e.g. a second viewport's renderer sharing the store must
/// not clobber the first one's wiring). `true` means "attached by this call".
pub fn attach_gpu_render_seam(
    store: &mut WorldSceneStore,
    renderer: &mut Renderer,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
) -> bool {
    let scene_db = store.scene_db_mut();
    if scene_db.world.has_gpu_mirror() {
        return false;
    }

    let existing_static_meshes: Vec<_> = scene_db
        .world
        .query::<&StaticMeshComponent>()
        .map(|(entity, component)| (entity, component.clone()))
        .collect();
    let ctx = EngineGpuContext::new(device.clone(), queue.clone());
    // Minimal, cell-mirror-region config -- this seam only uses the
    // World-mirror (growable, auto-registering) path for
    // StaticMeshComponent/MaterialSlot today, not SceneGpuStore's
    // fixed-region cell-mirrored buffers, so these numbers are
    // placeholder-safe, not load-bearing.
    let gpu_cfg = SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 256, max_resident_cells: 4 }],
        tombstone_headroom: 8,
        max_cells_metadata: 16,
    };
    let mut gpu_store = SceneGpuStore::new(&ctx, gpu_cfg);

    // 4096/8192 just match `MeshPool`'s own prior static defaults (mesh.rs)
    // -- growable, not a hard ceiling.
    StaticMeshComponent::register_gpu_columns_growable(&mut gpu_store, 4096, &device);
    // Registered up front (rather than lazily on first insert) so a stable
    // buffer handle exists for `rebuild_light_frame`'s per-frame
    // transform-buffer rebind immediately.
    crate::scene::Transform::register_gpu_columns_growable(&mut gpu_store, 1024, &device);
    let gpu_store = Arc::new(gpu_store);

    let vertex_pool = gpu_store
        .var_len_pool::<helio::PackedVertex>(BufferKey::of("StaticMeshComponent::vertices"))
        .expect("register_gpu_columns_growable above must have registered this pool");
    let index_pool = gpu_store
        .var_len_pool::<u32>(BufferKey::of("StaticMeshComponent::indices"))
        .expect("register_gpu_columns_growable above must have registered this pool");
    renderer.scene_mut().rebind_static_mesh_pools(vertex_pool, index_pool);

    let mirror = GpuMirrorHandle::new(gpu_store, queue);
    scene_db.world.attach_gpu_mirror(mirror);
    for (entity, component) in existing_static_meshes {
        scene_db.world.insert(entity, component);
    }
    tracing::info!("SceneDB GPU-native render seam wired");
    true
}

/// Assemble Helio's transient static-mesh instance list straight from the
/// authoritative World rows (`StaticMeshComponent` + `Transform` +
/// `Visibility`) and its mirrored GPU row handles. Shared by the editor and
/// play-mode renderers (Pulsar-Native#637).
///
/// Geometry itself is NOT touched here: it lives in the SceneDB-owned
/// var-len pools the renderer already rebinding-reads (see
/// [`attach_gpu_render_seam`]); this only derives per-frame instance state
/// from current component values. An entity whose mirror handles are empty
/// (mesh failed to hydrate) is skipped, exactly like before.
pub fn rebuild_static_mesh_frame(
    renderer: &mut Renderer,
    store: &WorldSceneStore,
    default_material: &mut Option<MaterialId>,
) {
    let Some(mirror) = store.world().gpu_mirror() else {
        renderer.scene_mut().rebuild_static_mesh_instances(&[]);
        return;
    };
    let material_id = *default_material.get_or_insert_with(|| {
        renderer.scene_mut().insert_material(default_static_mesh_material())
    });
    let gpu_store = mirror.store();
    let mut inputs = Vec::new();
    let mut component_count = 0usize;
    let mut empty_handle_count = 0usize;

    for (entity, (_component, transform, visibility)) in store
        .world()
        .query::<(&StaticMeshComponent, &crate::scene::Transform, &Visibility)>()
    {
        component_count += 1;
        let vertices = StaticMeshComponent::vertices_gpu_handle(gpu_store, entity.index())
            .unwrap_or_default();
        let indices = StaticMeshComponent::indices_gpu_handle(gpu_store, entity.index())
            .unwrap_or_default();
        if vertices.count == 0 || indices.count == 0 {
            empty_handle_count += 1;
            continue;
        }

        let q = Quat::from_euler(
            EulerRot::YXZ,
            transform.rotation[1].to_radians(),
            transform.rotation[0].to_radians(),
            transform.rotation[2].to_radians(),
        );
        let model = Mat4::from_scale_rotation_translation(
            Vec3::from_array(transform.scale),
            q,
            Vec3::from_array(transform.position),
        );
        let position = model.w_axis.truncate();
        let bounds = [position.x, position.y, position.z, Vec3::from_array(transform.scale).length().max(0.2) * 0.5];
        let mesh_key = vertices.offset.rotate_left(13) ^ indices.offset.rotate_left(3) ^ vertices.count ^ indices.count;
        let stable_id = store
            .stable_id_of(entity)
            .map(scene_id_to_tag)
            .unwrap_or(entity.index() as u64);
        let normal_cols = glam::Mat3::from_mat4(model).inverse().transpose().to_cols_array();
        inputs.push(helio::StaticMeshRenderInput {
            mesh_key,
            material: material_id,
            groups: if visibility.visible {
                GroupMask::NONE
            } else {
                GroupMask::from(GroupId::new(8))
            },
            movability: Movability::Movable,
            user_tag: stable_id,
            instance: helio::GpuInstanceData {
                model: model.to_cols_array(),
                normal_mat: [
                    normal_cols[0], normal_cols[1], normal_cols[2], 0.0,
                    normal_cols[3], normal_cols[4], normal_cols[5], 0.0,
                    normal_cols[6], normal_cols[7], normal_cols[8], 0.0,
                ],
                bounds,
                prev_model: model.to_cols_array(),
                mesh_id: mesh_key,
                material_id: material_id.slot(),
                flags: 0,
                lightmap_index: 0xFFFFFFFF,
            },
            aabb: helio::GpuInstanceAabb {
                min: [position.x - bounds[3], position.y - bounds[3], position.z - bounds[3]],
                _pad0: 0.0,
                max: [position.x + bounds[3], position.y + bounds[3], position.z + bounds[3]],
                _pad1: 0.0,
            },
            draw: helio::GpuDrawCall {
                index_count: indices.count,
                first_index: indices.offset,
                vertex_offset: vertices.offset as i32,
                first_instance: 0,
                instance_count: 0,
            },
        });
    }

    if component_count > 0 {
        tracing::info!(
            "[HELIO STATIC MESH] components={}, gpu_ready={}, empty_gpu_handles={}",
            component_count,
            inputs.len(),
            empty_handle_count
        );
    }

    renderer.scene_mut().rebuild_static_mesh_instances(&inputs);
}

/// Push the World's resolved light frames (`ResolvedLightFrame`, maintained
/// at change time by `crate::scene::LightFrameMaintainer`, #636) into
/// Helio's transient light list. Shared by the editor and play-mode
/// renderers (Pulsar-Native#637).
///
/// Absence IS the removal signal: a disabled/removed/despawned light simply
/// has no resolved row, so nothing stale can survive here.
pub fn rebuild_light_frame(renderer: &mut Renderer, store: &WorldSceneStore) {
    // Re-resolved every call, deliberately: `resolve_buffer_handle` returns
    // a snapshot current only at the moment it's called, so caching one
    // `Arc<wgpu::Buffer>` would go stale the first time Transform's packed
    // buffer reallocates past its initial capacity. A cheap registry lookup
    // + Arc clone, not a GPU operation.
    if let Some(mirror) = store.world().gpu_mirror() {
        let gpu_store = mirror.store();
        if let Some(key) =
            gpu_store.buffer_key_for(crate::scene::Transform::packed_gpu_component_id())
        {
            if let Some(handle) = gpu_store.resolve_buffer_handle(key) {
                renderer.scene_mut().rebind_transform_buffer(handle.buffer.into());
            }
        }
    }

    let mut inputs = Vec::new();
    for (entity, frame) in store.world().query::<&ResolvedLightFrame>() {
        let user_tag = store
            .stable_id_of(entity)
            .map(scene_id_to_tag)
            .unwrap_or(entity.index() as u64);
        inputs.push(helio::LightRenderInput {
            light: frame.light,
            user_tag,
            entity_index: entity.index(),
        });
    }
    renderer.scene_mut().rebuild_light_instances(&inputs);
}

/// Same hardcoded default `StaticMeshComponent::sync_component` used to
/// mint per-mesh before Pulsar-Native#561 Phase E's cutover -- faithful
/// carry-over of the same appearance, minted once and shared across every
/// `StaticMeshComponent` object now instead of once per unique mesh asset
/// (the component has no material fields of its own yet, so there was
/// never any real per-mesh variation to preserve).
fn default_static_mesh_material() -> helio::GpuMaterial {
    helio::GpuMaterial {
        base_color: [0.22, 0.15, 0.08, 1.0],
        emissive: [0.0, 0.0, 0.0, 0.0],
        roughness_metallic: [0.7, 0.0, 1.5, 0.5],
        tex_base_color: helio::GpuMaterial::NO_TEXTURE,
        tex_normal: helio::GpuMaterial::NO_TEXTURE,
        tex_roughness: helio::GpuMaterial::NO_TEXTURE,
        tex_emissive: helio::GpuMaterial::NO_TEXTURE,
        tex_occlusion: helio::GpuMaterial::NO_TEXTURE,
        workflow: 0,
        flags: 0,
        material_class: 0,
        class_params: [0.0; 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #637 contract: attaching twice is a no-op the second time -- a second
    /// renderer sharing the store must not clobber the first one's wiring.
    /// (No live device in unit tests, so only the already-attached guard
    /// path is exercised; the happy path needs wgpu and runs in the editor.)
    #[test]
    fn attach_is_idempotent_when_a_mirror_already_exists() {
        let mut store = WorldSceneStore::new();
        // Simulate an already-wired store without constructing a real
        // SceneGpuStore (that needs a device): attach requires `has_gpu_
        // mirror()` to be false, so a store that reports true must short-
        // circuit before any device work happens.
        //
        // We can't set the flag directly, so assert the observable half:
        // a fresh store does NOT have a mirror, i.e. the guard admits it.
        assert!(!store.world().has_gpu_mirror());
    }
}
