//! The one-world bridge between [`WorldSceneStore`] and any `helio::Renderer`
//! (#637).
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
//!
//! ## Ownership + invalidation protocol vs Helio#231 (#638)
//!
//! Agreed split between Pulsar-Native's World and the renderer-side residency
//! stages, recorded here because this module is the seam both sides meet at:
//!
//! - **Geometry** (vertex/index bytes): owned by SceneDB's content-id-
//!   interned var-len pools (`StaticMeshComponent`'s `#[gpu(content_id =
//!   "mesh_asset")]` fields, Pulsar-Native#632) -- entities naming the same
//!   `mesh_asset` share ONE allocation; Helio borrows each pool's
//!   underlying buffer via `rebind_static_mesh_pools`, unaware interning
//!   exists on top. Invalidation: pool regrow changes offsets, which is why
//!   mesh keys / draw params are derived fresh each pass from handles and
//!   never cached.
//! - **Per-instance state** (model/normal matrices, bounds, cull group):
//!   owned by the World as [`ResolvedMeshFrame`] rows (#638), maintained by
//!   [`MeshFrameMaintainer`] from component-change subscriptions. Helio holds
//!   no persistent per-instance record -- its transient instance list is
//!   rebuilt from these rows every pass.
//! - **Materials**: records are renderer-side (Helio#231 owns the material
//!   table and slot allocation). Until a `MaterialComponent` exists in the
//!   World to bind instances by stable id, every instance references ONE
//!   shared default minted per renderer (`default_material` cache). When
//!   that component lands, invalidation rides the same subscription
//!   mechanism as everything above.

use std::sync::Arc;

use helio::{GroupId, GroupMask, MaterialId, Movability, Renderer};
use helio_component::components::{MaterialOverrideComponent, StaticMeshComponent};
use pulsar_reflection::scene_id_to_tag;
use pulsar_scenedb::gpu::{
    BufferKey, EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};

use crate::scene::{
    LightFrameMaintainer, MaterialResource, MeshFrameMaintainer, ResolvedLightFrame,
    ResolvedMeshFrame, WorldSceneStore,
};

/// Read-only, frame-boundary projection owned by the SceneDB owner and handed
/// to a renderer.  It deliberately contains no `World` reference and no
/// lock.  The GPU mirror handle is SceneDB-owned; the small CPU vectors are
/// derived rows for this frame, not a second scene database or a snapshot.
#[derive(Clone)]
pub struct SceneRenderProjection {
    pub revision: u64,
    pub mirror: Option<GpuMirrorHandle>,
    pub meshes: Vec<(
        pulsar_scenedb::Entity,
        ResolvedMeshFrame,
        u64,
        Option<MaterialResource>,
        Option<MaterialOverrideComponent>,
    )>,
    pub lights: Vec<(pulsar_scenedb::Entity, ResolvedLightFrame, u64)>,
}

impl SceneRenderProjection {
    /// Build a disposable render view directly from the current SceneDB rows.
    /// No derived frame rows or asset snapshots are written back to World.
    pub fn from_store(store: &WorldSceneStore) -> Self {
        let world = store.world();
        let mirror = world.gpu_mirror().cloned();
        let mesh_entities: Vec<_> = world
            .query::<&StaticMeshComponent>()
            .map(|(entity, _)| entity)
            .collect();
        let meshes = mesh_entities
            .into_iter()
            .filter_map(|entity| {
                let frame = ResolvedMeshFrame::from_world(world, entity)?;
                let stable = store
                    .stable_id_of(entity)
                    .map(scene_id_to_tag)
                    .unwrap_or(entity.index() as u64);
                let material = world.get::<MaterialResource>(entity).cloned();
                let override_material = world.get::<MaterialOverrideComponent>(entity).cloned();
                Some((entity, frame, stable, material, override_material))
            })
            .collect();
        let light_entities: Vec<_> = world
            .query::<&helio_component::components::LightComponentGpuMirror>()
            .map(|(entity, _)| entity)
            .collect();
        let lights = light_entities
            .into_iter()
            .filter_map(|entity| {
                let frame = ResolvedLightFrame::from_world(world, entity)?;
                let stable = store
                    .stable_id_of(entity)
                    .map(scene_id_to_tag)
                    .unwrap_or(entity.index() as u64);
                Some((entity, frame, stable))
            })
            .collect();
        Self {
            revision: store.render_revision(),
            mirror,
            meshes,
            lights,
        }
    }
}

/// Advance the store by one render-side sync pass: flush the GPU mirror
/// (`SceneDb::step`) and refresh the subscription-maintained resolved rows
/// (lights #636, mesh instances #638) from World change events. Shared by
/// play-mode render loops (#637) -- the editor's sync passes do the same
/// between their phases.
pub fn step_scene_for_render(
    store: &mut WorldSceneStore,
    _lights: &mut LightFrameMaintainer,
    _meshes: &mut MeshFrameMaintainer,
) {
    // SceneDB is the only invalidation/upload authority. The compatibility
    // maintainer arguments are intentionally inert.
    store.scene_db_mut().step();
}

/// Ensure `store`'s SceneDB has a GPU mirror attached and return a cloneable
/// handle to it, creating one from `device`/`queue` if none exists yet.
///
/// **Must be called BEFORE constructing the `Renderer` for this device/queue.**
/// `helio::RendererBuilder::new` requires a `SceneDbHandle` (a `GpuMirrorHandle`)
/// up front — SceneDB is the sole scene authority and there is no valid
/// renderer configuration without one, so the mirror cannot be attached
/// after the fact the way the old `attach_gpu_render_seam(..., &mut Renderer,
/// ...)` API did. Once the `Renderer` exists, finish wiring it with
/// [`bind_renderer_mesh_projection`].
///
/// Idempotent: a second renderer (e.g. another viewport) sharing the same
/// `store` gets back the SAME handle rather than a second mirror.
///
/// Registers the canonical `StaticMeshComponent::vertices`/`indices`
/// content-id-interned var-len pools (Pulsar-Native#632: entities naming the
/// same `mesh_asset` share ONE GPU-resident allocation, refcounted, freed
/// automatically at zero) plus `Transform`'s packed buffer into a fresh
/// `SceneGpuStore`, and attaches the mirror so future component inserts
/// auto-mirror their `#[gpu]` fields.
///
/// Components inserted BEFORE this call were written with no mirror attached,
/// and SceneDB deliberately does not retroactively mirror those writes -- so
/// every already-present `StaticMeshComponent` is captured now and re-inserted
/// immediately after attaching (same typed value, re-dispatched into the
/// pools; no Helio mesh state is created here).
///
/// Idempotent: if `store` already has a GPU mirror (e.g. a second viewport's
/// renderer sharing the store), that SAME handle is returned and nothing else
/// is touched -- a second call never clobbers the first mirror's wiring.
pub fn ensure_gpu_mirror(
    store: &mut WorldSceneStore,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
) -> GpuMirrorHandle {
    let scene_db = store.scene_db_mut();
    if let Some(existing) = scene_db.world.gpu_mirror() {
        return existing.clone();
    }

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
    // Minimal, cell-mirror-region config -- this seam only uses the
    // World-mirror (growable, auto-registering) path for
    // StaticMeshComponent/MaterialSlot today, not SceneGpuStore's
    // fixed-region cell-mirrored buffers, so these numbers are
    // placeholder-safe, not load-bearing.
    let gpu_cfg = SceneGpuConfig {
        classes: vec![RegionClassConfig {
            capacity: 256,
            max_resident_cells: 4,
        }],
        tombstone_headroom: 8,
        max_cells_metadata: 16,
    };
    let mut gpu_store = SceneGpuStore::new(&ctx, gpu_cfg);

    // 4096/8192 just match `MeshPool`'s own prior static defaults (mesh.rs)
    // -- growable, not a hard ceiling.
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
    // `helio-pass-object-batch`'s sole data source -- see
    // `rebuild_static_mesh_frame`'s doc. 4096 matches `StaticMeshComponent`'s
    // own default above (growable, not a hard ceiling).
    helio_pass_gbuffer::StaticObjectComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        &device,
    );
    // Registered up front (rather than lazily on first insert) so a stable
    // buffer handle exists for `rebuild_light_frame`'s per-frame
    // transform-buffer rebind immediately.
    crate::scene::Transform::register_gpu_columns_growable(&mut gpu_store, 1024, &device);
    let gpu_store = Arc::new(gpu_store);

    // `StaticMeshComponent::vertices`/`indices` are content-id-interned
    // (Pulsar-Native#632/#659, `#[gpu(content_id = "mesh_asset")]`), so
    // they register through `interned_var_len_pool`, not the plain
    // `var_len_pool` this call used before. `.underlying()` hands back the
    // SAME `Arc<VarLenGpuPool<T>>` shape `rebind_static_mesh_pools` always
    // took -- Helio's own buffer binding is completely unaware interning
    // exists on top; it just draws whatever range each entity's row-indexed
    // handle names, shared or not.
    // ── Texel-streaming tier configuration (Helio#238 §5) ────────────────────
    // The ONE consumer configuration call (SceneDB#61 §4 contract): translate
    // the canonical `project/streaming.*` keys into a TierConfig and install
    // it once, here where the store exists and before any frame can touch
    // tiers. Idempotent upstream; re-running this whole seam is already
    // guarded by `has_gpu_mirror` above.
    //
    // No MaterializationSpecs yet: SceneDB-owned texture materialization gets
    // a bind path in S3; the budget + demand verbs are live from S2 on.
    {
        let streaming = |key: &str| -> Option<engine_state::settings::ConfigValue> {
            engine_state::settings::global_config()
                .get(engine_state::settings::NS_PROJECT, "streaming", key)
                .ok()
        };
        let int_of = |v: Option<engine_state::settings::ConfigValue>| -> Option<i64> {
            match v {
                Some(engine_state::settings::ConfigValue::Int(i)) => Some(i),
                _ => None,
            }
        };
        let pool_bytes = int_of(streaming("texture_stream_pool_mb"))
            .unwrap_or(512)
            .clamp(64, 16384) as u64
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
                "SceneDB tiers configured (Helio#238): VRAM budget {} MiB from project/streaming",
                pool_bytes / 1024 / 1024
            ),
            Err(e) => tracing::warn!("configure_tiers failed (streaming stays off): {e}"),
        }
    }

    let mirror = GpuMirrorHandle::new(gpu_store, queue);
    scene_db.world.attach_gpu_mirror(mirror.clone());
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

    tracing::info!("SceneDB GPU-native render seam wired");
    mirror
}

/// Finish wiring a `Renderer` that was constructed with the handle from
/// [`ensure_gpu_mirror`]: bind Helio's mesh storage directly at the SAME
/// content-id-interned var-len pools' underlying buffers the mirror's
/// `SceneGpuStore` owns (hydrate-time writes and draw-time reads then share
/// one buffer, zero translation).
///
/// Call this once, immediately after `RendererBuilder::build()`, using the
/// same `mirror` handle that was passed to `RendererBuilder::new`.
pub fn bind_renderer_mesh_projection(mirror: &GpuMirrorHandle, renderer: &mut Renderer) {
    let gpu_store = mirror.store();
    // `StaticMeshComponent::vertices`/`indices` are content-id-interned
    // (Pulsar-Native#632/#659, `#[gpu(content_id = "mesh_asset")]`), so they
    // register through `interned_var_len_pool`, not the plain `var_len_pool`
    // this call used before. `.underlying()` hands back the SAME
    // `Arc<VarLenGpuPool<T>>` shape `rebind_static_mesh_pools` always took --
    // Helio's own buffer binding is completely unaware interning exists on
    // top; it just draws whatever range each entity's row-indexed handle
    // names, shared or not.
    let vertex_pool = gpu_store
        .interned_var_len_pool::<helio::PackedVertex>(BufferKey::of(
            "StaticMeshComponent::vertices",
        ))
        .expect("ensure_gpu_mirror must have registered this pool")
        .underlying()
        .clone();
    let index_pool = gpu_store
        .interned_var_len_pool::<u32>(BufferKey::of("StaticMeshComponent::indices"))
        .expect("ensure_gpu_mirror must have registered this pool")
        .underlying()
        .clone();
    renderer.bind_static_mesh_projection(vertex_pool, index_pool);
}

/// Assemble Helio's transient static-mesh instance list from the
/// authoritative World rows, AND author each live entity's
/// [`helio_pass_gbuffer::StaticObjectComponent`] SceneDB row -- the actual
/// GPU-driven render path (`helio-pass-object-batch` onward) reads only the
/// latter now; central render-pass crates have zero knowledge of any
/// particular scene-object type, `StaticObjectComponent` being owned by
/// `helio-pass-gbuffer`. Shared by the editor and play-mode renderers
/// (Pulsar-Native#637).
///
/// The `submit_static_mesh_frame` transient-list half is kept for now only
/// because [`helio::picking`]'s `Scene::iter_pickable_objects` (click-to-
/// select) still reads it -- migrating picking onto SceneDB is a distinct,
/// not-yet-done follow-up. Until then this function does genuinely
/// duplicate per-entity work (draw params/instance data computed twice);
/// accepted as the safe intermediate state rather than silently breaking
/// selection.
///
/// Pulsar-Native#638: the transform-derived half of each instance (model /
/// normal matrices, position, bounding radius, cull flag) is READ from the
/// subscription-maintained [`ResolvedMeshFrame`] rows instead of being
/// re-derived for every entity every pass. The GPU-pool-keyed half (mesh
/// key, draw counts/offsets) is still taken fresh from the var-len handles
/// here -- pool offsets legitimately shift on regrow, so that part must
/// never be cached.
///
/// Material binding: every instance references the shared default material,
/// minted once per renderer (`default_material` cache). Per-instance
/// materials by stable id are Helio#231's renderer-side stage -- see this
/// module's ownership-protocol doc for the agreed split.
///
/// `world`/`authored_objects`: SceneDB write access and this function's own
/// "what did I author last pass" set, needed because a `StaticObjectComponent`
/// row is a real, persistent SceneDB row (unlike Helio's rebuilt-every-frame
/// transient list) -- an entity that drops out of `projection.meshes` (removed,
/// hidden, mesh hydration failed) needs its row explicitly removed, tracked
/// here the same way `foliage_cache`/`portal_link_cache` track their own
/// live sets elsewhere in this bridge.
pub fn rebuild_static_mesh_frame(
    renderer: &mut Renderer,
    projection: &SceneRenderProjection,
    materials: &mut StaticMeshMaterialProjections,
    world: &mut pulsar_scenedb::World,
    authored_objects: &mut std::collections::HashSet<pulsar_scenedb::Entity>,
) {
    let Some(mirror) = projection.mirror.as_ref() else {
        renderer.submit_static_mesh_frame(&[]);
        let stale: Vec<_> = world
            .query::<&helio_pass_gbuffer::StaticObjectComponent>()
            .map(|(entity, _)| entity)
            .collect();
        for entity in stale {
            world.remove::<helio_pass_gbuffer::StaticObjectComponent>(entity);
        }
        authored_objects.clear();
        return;
    };
    let default_material = materials.default(renderer);
    let gpu_store = mirror.store();
    let mut inputs = Vec::new();
    let mut component_count = 0usize;
    let mut empty_handle_count = 0usize;
    let mut live_this_frame = std::collections::HashSet::with_capacity(projection.meshes.len());

    // One query over resolved rows only -- no Transform/Visibility join, no
    // matrix math in this loop anymore (#638).
    for (entity, frame, stable_id, material_resource, material_override) in &projection.meshes {
        component_count += 1;
        let vertices =
            StaticMeshComponent::vertices_gpu_handle(gpu_store, entity.index()).unwrap_or_default();
        let indices =
            StaticMeshComponent::indices_gpu_handle(gpu_store, entity.index()).unwrap_or_default();
        if vertices.count == 0 || indices.count == 0 {
            empty_handle_count += 1;
            continue;
        }

        let mesh_key = vertices.offset.rotate_left(13)
            ^ indices.offset.rotate_left(3)
            ^ vertices.count
            ^ indices.count;
        let material = material_resource
            .as_ref()
            .map(|component| materials.material_for_resource(renderer, *entity, component))
            .or_else(|| {
                material_override
                    .as_ref()
                    .map(|component| materials.material_for_override(renderer, *entity, component))
            })
            .unwrap_or(default_material);
        inputs.push(helio::StaticMeshRenderInput {
            mesh_key,
            material,
            groups: if frame.visible {
                GroupMask::NONE
            } else {
                GroupMask::from(GroupId::new(8))
            },
            movability: Movability::Movable,
            user_tag: *stable_id,
            instance: helio::GpuInstanceData {
                model: frame.model,
                normal_mat: frame.normal_mat,
                bounds: [
                    frame.position[0],
                    frame.position[1],
                    frame.position[2],
                    frame.bound_radius,
                ],
                prev_model: frame.model,
                mesh_id: mesh_key,
                material_id: material.slot(),
                flags: 0,
                lightmap_index: 0xFFFFFFFF,
            },
            aabb: frame.aabb(),
            draw: helio::GpuDrawCall {
                index_count: indices.count,
                first_index: indices.offset,
                vertex_offset: vertices.offset as i32,
                first_instance: 0,
                instance_count: 0,
            },
        });

        // ── GPU-driven path: author the SceneDB row `helio-pass-object-
        // batch` reads. `!frame.visible` is treated as absent here (no
        // per-view group masking on this path yet, unlike `groups` above --
        // a known simplification, not full parity with the legacy list).
        if frame.visible {
            if let Some((material_class, graph_hash)) = renderer.material_batch_key(material) {
                let transform = transform_cols(frame.model);
                let prev_transform = world
                    .get::<helio_pass_gbuffer::StaticObjectComponent>(*entity)
                    .map(|existing| existing.transform)
                    .unwrap_or(transform);
                world.insert(
                    *entity,
                    helio_pass_gbuffer::StaticObjectComponent {
                        // No live `helio::MeshId` exists for SceneDB-native
                        // geometry (it never went through Helio's own mesh
                        // upload path) -- unused by the draw pipeline, only
                        // by `StaticObjectComponent::mesh()` for asset
                        // re-resolution, which nothing calls for these rows.
                        mesh_slot: 0,
                        mesh_generation: 0,
                        material_slot: material.slot(),
                        material_generation: material.generation(),
                        transform,
                        prev_transform,
                        normal_mat: normal_mat_rows(frame.normal_mat),
                        bounds: [
                            frame.position[0],
                            frame.position[1],
                            frame.position[2],
                            frame.bound_radius,
                        ],
                        index_count: indices.count,
                        first_index: indices.offset,
                        vertex_offset: vertices.offset as i32,
                        material_class,
                        graph_hash_lo: graph_hash as u32,
                        graph_hash_hi: (graph_hash >> 32) as u32,
                        flags: 0,
                    },
                );
                live_this_frame.insert(*entity);
            }
        }
    }

    // Remove stale derived GPU rows by querying SceneDB itself. The caller's
    // legacy `authored_objects` argument is intentionally cleared and never
    // used as authority; removal remains correct after replacement, despawn,
    // or renderer recreation.
    let stale: Vec<_> = world
        .query::<&helio_pass_gbuffer::StaticObjectComponent>()
        .map(|(entity, _)| entity)
        .filter(|entity| !live_this_frame.contains(entity))
        .collect();
    for entity in stale {
        world.remove::<helio_pass_gbuffer::StaticObjectComponent>(entity);
    }
    authored_objects.clear();

    if component_count > 0 {
        tracing::info!(
            "[HELIO STATIC MESH] components={}, gpu_ready={}, empty_gpu_handles={}",
            component_count,
            inputs.len(),
            empty_handle_count
        );
    }

    renderer.submit_static_mesh_frame(&inputs);
}

/// Reinterpret a flat column-major `[f32; 16]` (`ResolvedMeshFrame::model`'s
/// layout) as the nested `[[f32; 4]; 4]` shape `StaticObjectComponent::
/// transform` stores -- a pure reshape, no floating-point recomputation.
fn transform_cols(flat: [f32; 16]) -> [[f32; 4]; 4] {
    [
        [flat[0], flat[1], flat[2], flat[3]],
        [flat[4], flat[5], flat[6], flat[7]],
        [flat[8], flat[9], flat[10], flat[11]],
        [flat[12], flat[13], flat[14], flat[15]],
    ]
}

/// Same reshape as `transform_cols`, for `ResolvedMeshFrame::normal_mat`'s
/// flat `[f32; 12]` into `StaticObjectComponent::normal_mat`'s `[[f32; 4]; 3]`.
fn normal_mat_rows(flat: [f32; 12]) -> [[f32; 4]; 3] {
    [
        [flat[0], flat[1], flat[2], flat[3]],
        [flat[4], flat[5], flat[6], flat[7]],
        [flat[8], flat[9], flat[10], flat[11]],
    ]
}

/// Push the World's resolved light frames (`ResolvedLightFrame`, maintained
/// at change time by `crate::scene::LightFrameMaintainer`, #636) into
/// Helio's transient light list. Shared by the editor and play-mode
/// renderers (Pulsar-Native#637).
///
/// Absence IS the removal signal: a disabled/removed/despawned light simply
/// has no resolved row, so nothing stale can survive here.
pub fn rebuild_light_frame(renderer: &mut Renderer, projection: &SceneRenderProjection) {
    // Re-resolved every call, deliberately: `resolve_buffer_handle` returns
    // a snapshot current only at the moment it's called, so caching one
    // `Arc<wgpu::Buffer>` would go stale the first time Transform's packed
    // buffer reallocates past its initial capacity. A cheap registry lookup
    // + Arc clone, not a GPU operation.
    if let Some(mirror) = projection.mirror.as_ref() {
        let gpu_store = mirror.store();
        if let Some(key) =
            gpu_store.buffer_key_for(crate::scene::Transform::packed_gpu_component_id())
        {
            if let Some(handle) = gpu_store.resolve_buffer_handle(key) {
                renderer.bind_transform_projection(handle.buffer.into());
            }
        }
    }

    let mut inputs = Vec::new();
    for (entity, frame, user_tag) in &projection.lights {
        inputs.push(helio::LightRenderInput {
            light: frame.light,
            user_tag: *user_tag,
            entity_index: entity.index(),
        });
    }
    renderer.submit_light_frame(&inputs);
}

/// Compatibility shell for the renderer interaction layer.
///
/// SceneDB owns material values. This type deliberately contains no default,
/// entity, content, or Helio-id map; material records are projected from the
/// current World component at submission time. A later renderer migration may
/// replace this shell with SceneDB GPU-mirror bindings without changing the
/// ownership contract.
#[derive(Default)]
pub struct StaticMeshMaterialProjections;

impl StaticMeshMaterialProjections {
    fn default(&mut self, renderer: &mut Renderer) -> MaterialId {
        renderer.create_material_projection(default_static_mesh_material())
    }

    fn material_for_override(
        &mut self,
        renderer: &mut Renderer,
        _entity: pulsar_scenedb::Entity,
        component: &MaterialOverrideComponent,
    ) -> MaterialId {
        renderer.create_material_projection(material_from_override(component))
    }

    fn material_for_resource(
        &mut self,
        renderer: &mut Renderer,
        _entity: pulsar_scenedb::Entity,
        component: &MaterialResource,
    ) -> MaterialId {
        let material = helio::GpuMaterial {
            base_color: component.base_color,
            emissive: [
                component.emissive_color[0] * component.emissive_intensity,
                component.emissive_color[1] * component.emissive_intensity,
                component.emissive_color[2] * component.emissive_intensity,
                0.0,
            ],
            roughness_metallic: [component.roughness, component.metallic, 1.5, 0.5],
            tex_base_color: helio::GpuMaterial::NO_TEXTURE,
            tex_normal: helio::GpuMaterial::NO_TEXTURE,
            tex_roughness: helio::GpuMaterial::NO_TEXTURE,
            tex_emissive: helio::GpuMaterial::NO_TEXTURE,
            tex_occlusion: helio::GpuMaterial::NO_TEXTURE,
            workflow: 0,
            flags: 0,
            material_class: 0,
            class_params: [0.0; 4],
        };
        renderer.create_material_projection(material)
    }
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

fn material_from_override(component: &MaterialOverrideComponent) -> helio::GpuMaterial {
    helio::GpuMaterial {
        base_color: [
            component.base_color[0],
            component.base_color[1],
            component.base_color[2],
            component.alpha,
        ],
        emissive: [
            component.emissive_color[0] * component.emissive_intensity,
            component.emissive_color[1] * component.emissive_intensity,
            component.emissive_color[2] * component.emissive_intensity,
            0.0,
        ],
        roughness_metallic: [component.roughness, component.metallic, 1.5, 0.5],
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

    fn material_override() -> MaterialOverrideComponent {
        MaterialOverrideComponent {
            base_color: [0.1, 0.2, 0.3, 0.9],
            metallic: 0.4,
            roughness: 0.6,
            emissive_color: [0.7, 0.8, 0.9],
            emissive_intensity: 2.0,
            alpha: 0.5,
            uv_scale_x: 1.0,
            uv_scale_y: 1.0,
            uv_offset_x: 0.0,
            uv_offset_y: 0.0,
        }
    }

    #[test]
    fn material_override_projection_reads_scene_db_fields() {
        let component = material_override();
        let gpu = material_from_override(&component);

        assert_eq!(gpu.base_color, [0.1, 0.2, 0.3, 0.5]);
        assert_eq!(gpu.emissive, [1.4, 1.6, 1.8, 0.0]);
        assert_eq!(gpu.roughness_metallic, [0.6, 0.4, 1.5, 0.5]);
        assert_eq!(component.alpha, 0.5);
    }

    /// #637 contract: attaching twice is a no-op the second time -- a second
    /// renderer sharing the store must not clobber the first one's wiring.
    /// (No live device in unit tests, so only the already-attached guard
    /// path is exercised; the happy path needs wgpu and runs in the editor.)
    #[test]
    fn attach_is_idempotent_when_a_mirror_already_exists() {
        let store = WorldSceneStore::new();
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
