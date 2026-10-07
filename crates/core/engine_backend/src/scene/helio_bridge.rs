//! SceneDB's GPU-mirror attachment seam for Helio 3.0.
//!
//! SceneDB owns all world and asset state; component values mirror their own
//! GPU rows on every write. Helio receives the cloneable GPU mirror during
//! renderer construction, and its scene join ([`scene_join`]) turns the
//! authored rows (mesh and light instances, their owner links, the owner
//! objects' transforms and visibility) into the object, material and light
//! rows its passes draw, on the GPU, whenever they change. This module keeps
//! no frame projection, render-row subscription, material table or
//! per-frame input assembly (Pulsar-Native#1035, Phase 2).
use std::sync::Arc;

use helio_component::components::{
    LightSourceRow, StaticMeshComponent, StaticMeshDraw, LIGHT_SOURCES_BUFFER,
    MESH_BOUNDS_BUFFER, MESH_FLAGS_BUFFER, MESH_SECTIONS_BUFFER,
};
use helio_default_graphs::scene_join::{SceneJoin, SceneJoinKeys, ENTITY_GENERATIONS_KEY};
use pulsar_scenedb::gpu::{
    BufferKey, EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig,
    SceneGpuStore,
};

use pulsar_scene_model::attachments::ComponentOwner;
use pulsar_scene_model::ObjectHidden;

/// Where this engine's authored rows live, for Helio's scene join. The
/// names are the buffers the components below register: the instance owner
/// links (`ComponentOwner`), object visibility (`Visibility`'s derived
/// `ObjectHidden`) and transforms (`Transform`, packed), the mesh geometry
/// handles and derived draw rows (`StaticMeshComponent`, `StaticMeshDraw`),
/// and the light rows (`LightComponent`'s derived `LightSourceRow`).
pub fn scene_join_keys() -> SceneJoinKeys {
    // A var-len field's handle table is its pool key + `::handles`.
    SceneJoinKeys {
        owners: BufferKey::of("component_owners"),
        generations: ENTITY_GENERATIONS_KEY,
        hidden: BufferKey::of("object_hidden"),
        transforms: BufferKey::of("Transform::packed"),
        vertex_handles: BufferKey::of("builtin_mesh_vertex::handles"),
        index_handles: BufferKey::of("builtin_mesh_index::handles"),
        mesh_bounds: BufferKey::of(MESH_BOUNDS_BUFFER),
        mesh_flags: BufferKey::of(MESH_FLAGS_BUFFER),
        section_handles: BufferKey::of("static_mesh_draw_sections::handles"),
        mesh_sections: BufferKey::of(MESH_SECTIONS_BUFFER),
        light_sources: BufferKey::of(LIGHT_SOURCES_BUFFER),
    }
}

/// Helio's scene join over this engine's rows, for
/// `RendererBuilder::with_scene_derivation`. `editor` adds the light icon
/// billboards.
pub fn scene_join(device: &wgpu::Device, editor: bool) -> Box<SceneJoin> {
    Box::new(SceneJoin::new(device, scene_join_keys(), editor))
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
/// A world populated before GPU initialization needs nothing further: SceneDB
/// writes every existing GPU-bearing component into the mirror when it is
/// attached, for every schema, so this helper keeps no list of types to
/// re-insert.
pub fn ensure_gpu_mirror(
    scene_db: &mut pulsar_scenedb::SceneDb,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
) -> GpuMirrorHandle {
    if let Some(existing) = scene_db.world.gpu_mirror() {
        return existing.clone();
    }

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

    // These are SceneDB component columns. Capacities are initial capacities
    // only; growable registration remains the sole owner of GPU storage and
    // deduplication behavior.
    StaticMeshComponent::register_gpu_columns_growable(&mut gpu_store, 4096, &device);
    // The scene join's inputs (see `scene_join_keys`); the object, material
    // and light rows passes draw are the join's outputs, not SceneDB's.
    StaticMeshDraw::register_gpu_columns_growable(&mut gpu_store, 4096, &device);
    LightSourceRow::register_gpu_columns_growable(&mut gpu_store, 64, &device);
    ComponentOwner::register_gpu_columns_growable(&mut gpu_store, 4096, &device);
    ObjectHidden::register_gpu_columns_growable(&mut gpu_store, 1024, &device);
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
    crate::scene::Transform::register_gpu_columns_growable(&mut gpu_store, 1024, &device);
    // The editor viewport's post-process baseline; see `editor_postprocess`.
    helio_pass_postprocess::CameraPostProcessComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4,
        &device,
    );

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

    let material_texture_limit =
        helio_mats::MaterialBindingConfig::for_device(&device).max_textures;
    let texture_store = Arc::new(std::sync::RwLock::new(
        pulsar_scenedb::gpu::TextureStore::new(material_texture_limit as u32),
    ));
    let mirror = GpuMirrorHandle::new(Arc::new(gpu_store), queue)
        .with_texture_store(texture_store)
        .expect("register SceneDB material texture store");
    scene_db.world.attach_gpu_mirror(mirror.clone());
    crate::scene::install_scenedb_inspector(&mut scene_db.world);

    tracing::info!("SceneDB GPU mirror attached for Helio 3.0");
    mirror
}

#[cfg(test)]
mod tests {
    use super::*;
    use helio_default_graphs::scene_join as join;

    #[test]
    fn a_new_scene_does_not_have_a_gpu_mirror() {
        let scene_db = pulsar_scenedb::SceneDb::new();
        assert!(!scene_db.world.has_gpu_mirror());
    }

    /// The join reads these rows with fixed layouts; a drift here must fail
    /// a test, not draw garbage.
    #[test]
    fn the_rows_the_scene_join_reads_have_its_layouts() {
        use std::mem::size_of;
        assert_eq!(size_of::<ComponentOwner>() as u64, join::OWNER_ROW_BYTES);
        assert_eq!(size_of::<ObjectHidden>() as u64, join::HIDDEN_ROW_BYTES);
        assert_eq!(
            size_of::<crate::scene::Transform>() as u64,
            join::TRANSFORM_ROW_BYTES
        );
        assert_eq!(
            size_of::<pulsar_scenedb::gpu::VarLenHandle>() as u64,
            join::HANDLE_ROW_BYTES
        );
        assert_eq!(
            size_of::<helio_component::components::MeshSectionDraw>() as u64,
            join::MESH_SECTION_ROW_BYTES
        );
        assert_eq!(size_of::<LightSourceRow>() as u64, join::LIGHT_SOURCE_ROW_BYTES);
        assert_eq!(size_of::<[f32; 4]>() as u64, join::MESH_BOUNDS_ROW_BYTES);
        assert_eq!(size_of::<u32>() as u64, join::MESH_FLAGS_ROW_BYTES);
    }
}
