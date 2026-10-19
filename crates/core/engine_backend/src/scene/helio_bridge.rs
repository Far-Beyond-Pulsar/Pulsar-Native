//! SceneDB's GPU-mirror attachment seam for Helio 3.0.
//!
//! SceneDB owns all world and asset state; component values mirror their own
//! GPU rows on every write. Helio receives the cloneable GPU mirror during
//! renderer construction, and its scene join ([`scene_join`]) turns the
//! authored rows (mesh and light instances, their owner links, the owner
//! objects' transforms and visibility) into the object, material and light
//! rows its passes draw, on the GPU, whenever they change. This module keeps
//! no frame projection, render-row subscription, material table or
//! per-frame input assembly (Pulsar-Native#1035, Phase 2). Its environment
//! join ([`environment_join`]) does the same for fog volumes, post-process
//! volumes, camera post-process settings, water volumes, foliage,
//! atmospheres, decals and particle emitters (Phase 4).
use std::sync::Arc;

use helio_component::components::{
    AtmosphereSourceRow, CameraPostProcessSourceRow, CoronaEmitterSourceRow, DecalSourceRow,
    FoliageSourceRow, GlobalFogSourceRow, GlobalWindSourceRow, LightSourceRow, LocalFogSourceRow,
    PostProcessVolumeSourceRow, SkeletalMeshComponent, SpriteSourceRow, StaticMeshComponent,
    StaticMeshDraw, WaterVolumeSourceRow, ATMOSPHERE_SOURCES_BUFFER,
    CAMERA_POST_PROCESS_SOURCES_BUFFER, CORONA_EMITTER_SOURCES_BUFFER, DECAL_SOURCES_BUFFER,
    FOLIAGE_SOURCES_BUFFER, GLOBAL_FOG_SOURCES_BUFFER, LIGHT_SOURCES_BUFFER,
    LOCAL_FOG_SOURCES_BUFFER, MESH_BOUNDS_BUFFER, MESH_FLAGS_BUFFER, MESH_SECTIONS_BUFFER,
    POST_PROCESS_VOLUME_SOURCES_BUFFER, SKELETAL_PALETTE_HANDLES_BUFFER,
    SKELETAL_SKIN_HANDLES_BUFFER, SPRITE_SOURCES_BUFFER, WATER_VOLUME_SOURCES_BUFFER,
    WIND_SOURCES_BUFFER,
};
use helio_default_graphs::environment_join::{EnvironmentJoin, EnvironmentJoinKeys};
use helio_default_graphs::hierarchy_resolve::{HierarchyResolve, HierarchyResolveKeys};
use helio_default_graphs::scene_join::{SceneJoin, SceneJoinKeys, ENTITY_GENERATIONS_KEY};
use pulsar_scenedb::gpu::{
    BufferKey, EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};

use pulsar_scene_model::attachments::ComponentOwner;
use pulsar_scene_model::{ObjectFrame, ObjectHidden, ObjectParent};

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
        skin_handles: BufferKey::of(SKELETAL_SKIN_HANDLES_BUFFER),
        palette_handles: BufferKey::of(SKELETAL_PALETTE_HANDLES_BUFFER),
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

/// Where this engine's parent-relative transforms and parent links live, for
/// Helio's hierarchy resolve. It republishes world-space transforms under the
/// transform key, so it must be installed before the scene and environment
/// joins.
pub fn hierarchy_resolve_keys() -> HierarchyResolveKeys {
    HierarchyResolveKeys {
        transforms: scene_join_keys().transforms,
        parents: BufferKey::of("object_parent"),
        frames: BufferKey::of("object_frame"),
    }
}

/// Helio's hierarchy resolve, for `RendererBuilder::with_scene_derivation`,
/// ahead of [`scene_join`] and [`environment_join`].
pub fn hierarchy_resolve(device: &wgpu::Device) -> Box<HierarchyResolve> {
    Box::new(HierarchyResolve::new(device, hierarchy_resolve_keys()))
}

/// Where this engine's environment rows live, for Helio's environment join:
/// the same owner, visibility, transform and mesh bounds rows as the scene
/// join, and the rows fog volumes, post-process volumes, camera
/// post-process, water, foliage, global wind, atmosphere, decal and particle
/// emitter components derive (Pulsar-Native#1035, Phase 4), the water
/// interaction rows physics bodies on the `WaterSim` channel derive (#1080)
/// and the 2D sprite rows (#1060).
pub fn environment_join_keys() -> EnvironmentJoinKeys {
    let scene = scene_join_keys();
    EnvironmentJoinKeys {
        owners: scene.owners,
        generations: scene.generations,
        hidden: scene.hidden,
        transforms: scene.transforms,
        global_fog: BufferKey::of(GLOBAL_FOG_SOURCES_BUFFER),
        local_fog: BufferKey::of(LOCAL_FOG_SOURCES_BUFFER),
        post_process_volumes: BufferKey::of(POST_PROCESS_VOLUME_SOURCES_BUFFER),
        camera_post_process: BufferKey::of(CAMERA_POST_PROCESS_SOURCES_BUFFER),
        camera_lens: BufferKey::of(helio_component::components::CAMERA_LENS_SOURCES_BUFFER),
        water_volumes: BufferKey::of(WATER_VOLUME_SOURCES_BUFFER),
        foliage: BufferKey::of(FOLIAGE_SOURCES_BUFFER),
        atmospheres: BufferKey::of(ATMOSPHERE_SOURCES_BUFFER),
        decals: BufferKey::of(DECAL_SOURCES_BUFFER),
        corona_emitters: BufferKey::of(CORONA_EMITTER_SOURCES_BUFFER),
        wind: BufferKey::of(WIND_SOURCES_BUFFER),
        water_hitboxes: BufferKey::of(pulsar_physics::WATER_HITBOX_SOURCES_BUFFER),
        mesh_bounds: scene.mesh_bounds,
        sprites: BufferKey::of(SPRITE_SOURCES_BUFFER),
    }
}

/// Helio's environment join over this engine's rows, for
/// `RendererBuilder::with_scene_derivation`.
pub fn environment_join(device: &wgpu::Device) -> Box<EnvironmentJoin> {
    Box::new(EnvironmentJoin::new(device, environment_join_keys()))
}

/// Ensure that the authoritative SceneDB world has one GPU mirror for Helio's
/// renderer and return the shared handle used by RendererBuilder.
///
/// The mirror is attached before renderer construction. Component writes made
/// after attachment are mirrored by SceneDB itself; this helper does not keep
/// a second world representation or perform per-frame synchronization.
///
/// When a world already has a mirror on `device`, the existing handle is
/// returned without replacing its queue, pools, or residency state. This is
/// required when multiple renderer views share one SceneDB world. A mirror on
/// another device (the previous one was lost and a renderer is being rebuilt
/// on a new one) is replaced: attaching the new mirror writes every
/// GPU-bearing component into it, and the assets the world bound are
/// uploaded again from SceneDB's CPU copies.
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
        if existing.store().device_arc().as_ref() == device.as_ref() {
            return existing.clone();
        }
        tracing::warn!("SceneDB GPU mirror is on another device; rebuilding it on the new one");
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
    SkeletalMeshComponent::register_gpu_columns_growable(&mut gpu_store, 128, &device);
    // The scene join's inputs (see `scene_join_keys`); the object, material
    // and light rows passes draw are the join's outputs, not SceneDB's.
    StaticMeshDraw::register_gpu_columns_growable(&mut gpu_store, 4096, &device);
    LightSourceRow::register_gpu_columns_growable(&mut gpu_store, 64, &device);
    ComponentOwner::register_gpu_columns_growable(&mut gpu_store, 4096, &device);
    ObjectHidden::register_gpu_columns_growable(&mut gpu_store, 1024, &device);
    // Each object's parent, for the hierarchy resolve (transforms are parent-relative).
    ObjectParent::register_gpu_columns_growable(&mut gpu_store, 1024, &device);
    // Which objects are rigid frames, and their slot in the coordinate-space table.
    ObjectFrame::register_gpu_columns_growable(&mut gpu_store, 64, &device);
    crate::scene::Transform::register_gpu_columns_growable(&mut gpu_store, 1024, &device);
    // The environment join's inputs (see `environment_join_keys`); the fog
    // media, post-process volume, camera, water volume, foliage, atmosphere,
    // decal, Corona emitter and water hitbox rows the passes read are its
    // outputs.
    GlobalFogSourceRow::register_gpu_columns_growable(&mut gpu_store, 4, &device);
    LocalFogSourceRow::register_gpu_columns_growable(&mut gpu_store, 16, &device);
    PostProcessVolumeSourceRow::register_gpu_columns_growable(&mut gpu_store, 16, &device);
    CameraPostProcessSourceRow::register_gpu_columns_growable(&mut gpu_store, 4, &device);
    // The camera lens: the authored `CameraLensComponent` derives its source
    // row (the environment join publishes it as `camera_lenses`) and the
    // lens pass's `lens_elements` rows.
    helio_component::components::register_lens_gpu_columns(&mut gpu_store, &device);
    WaterVolumeSourceRow::register_gpu_columns_growable(&mut gpu_store, 8, &device);
    FoliageSourceRow::register_gpu_columns_growable(&mut gpu_store, 8, &device);
    AtmosphereSourceRow::register_gpu_columns_growable(&mut gpu_store, 4, &device);
    DecalSourceRow::register_gpu_columns_growable(&mut gpu_store, 64, &device);
    CoronaEmitterSourceRow::register_gpu_columns_growable(&mut gpu_store, 16, &device);
    GlobalWindSourceRow::register_gpu_columns_growable(&mut gpu_store, 2, &device);
    // 2D sprites; the join places them as the sprite overlay's rows.
    SpriteSourceRow::register_gpu_columns_growable(&mut gpu_store, 64, &device);
    // Bodies that push simulated water; the join bounds them by their
    // owners' meshes into the water simulation's hitbox rows.
    pulsar_physics::WaterHitboxSourceRow::register_gpu_columns_growable(
        &mut gpu_store,
        64,
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
        // Rebuildable asset data (decoded images) is held in SceneDB's budgeted
        // residency cache, so CPU memory follows this budget, not the number of
        // assets in the project. Dropped data is decoded again on demand.
        let cpu_cache_bytes = int_of(streaming("cpu_cache_mb"))
            .unwrap_or(1024)
            .clamp(64, 65_536) as usize
            * 1024
            * 1024;
        pulsar_scenedb::set_residency_budget(cpu_cache_bytes);
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

    // Textures are cooked before they reach the GPU: a mip chain, and BC7 where
    // the device can sample it. Loaders cook in the format this device wants,
    // and SceneDB keeps the result on disk in the project's cache so a texture
    // is cooked once, not once per session.
    helio_component::texture_assets::set_preferred_cook_format(
        if device
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
        {
            helio_component::cooked_texture::CookFormat::Bc7
        } else {
            helio_component::cooked_texture::CookFormat::Rgba8
        },
    );
    install_derived_cache();

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

/// Point SceneDB's derived-data cache at the project's cache folder and keep it
/// within `streaming.derived_cache_mb` (default 4096), pruning the least
/// recently used files on a background thread. Done once per process: a second
/// scene in the same project only repeats it.
fn install_derived_cache() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(project) = engine_state::get_project_path() else {
            return;
        };
        let directory = std::path::PathBuf::from(project)
            .join(".pulsar")
            .join("cache")
            .join("derived");
        let budget_mb = match engine_state::settings::global_config().get(
            engine_state::settings::NS_PROJECT,
            "streaming",
            "derived_cache_mb",
        ) {
            Ok(engine_state::settings::ConfigValue::Int(mb)) => mb.clamp(64, 1_048_576) as u64,
            _ => 4096,
        };
        pulsar_scenedb::set_derived_cache_dir(Some(directory));
        std::thread::spawn(move || {
            match pulsar_scenedb::prune_derived_cache(budget_mb * 1024 * 1024) {
                Ok(report) if report.removed_files > 0 => tracing::info!(
                    "derived-data cache pruned: removed {} files ({} MiB), {} MiB kept",
                    report.removed_files,
                    report.removed_bytes / (1024 * 1024),
                    report.kept_bytes / (1024 * 1024)
                ),
                Ok(_) => {}
                Err(error) => tracing::debug!("derived-data cache not pruned: {error}"),
            }
        });
    });
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
        assert_eq!(
            size_of::<LightSourceRow>() as u64,
            join::LIGHT_SOURCE_ROW_BYTES
        );
        assert_eq!(size_of::<[f32; 4]>() as u64, join::MESH_BOUNDS_ROW_BYTES);
        assert_eq!(size_of::<u32>() as u64, join::MESH_FLAGS_ROW_BYTES);
    }

    #[test]
    fn the_rows_the_environment_join_reads_have_its_layouts() {
        use helio_default_graphs::environment_join as env;
        use std::mem::size_of;
        assert_eq!(
            size_of::<GlobalFogSourceRow>() as u64,
            env::GLOBAL_FOG_SOURCE_ROW_BYTES
        );
        assert_eq!(
            size_of::<LocalFogSourceRow>() as u64,
            env::LOCAL_FOG_SOURCE_ROW_BYTES
        );
        assert_eq!(
            size_of::<PostProcessVolumeSourceRow>() as u64,
            env::POST_PROCESS_VOLUME_SOURCE_ROW_BYTES
        );
        assert_eq!(
            size_of::<CameraPostProcessSourceRow>() as u64,
            env::CAMERA_POST_PROCESS_SOURCE_ROW_BYTES
        );
        assert_eq!(
            size_of::<helio_component::components::CameraLensSourceRow>() as u64,
            env::CAMERA_LENS_SOURCE_ROW_BYTES
        );
        assert_eq!(
            size_of::<WaterVolumeSourceRow>() as u64,
            env::WATER_VOLUME_SOURCE_ROW_BYTES
        );
        assert_eq!(
            size_of::<FoliageSourceRow>() as u64,
            env::FOLIAGE_SOURCE_ROW_BYTES
        );
        assert_eq!(
            size_of::<AtmosphereSourceRow>() as u64,
            env::ATMOSPHERE_SOURCE_ROW_BYTES
        );
        assert_eq!(
            size_of::<pulsar_physics::WaterHitboxSourceRow>() as u64,
            env::WATER_HITBOX_SOURCE_ROW_BYTES
        );
        assert_eq!(
            size_of::<SpriteSourceRow>() as u64,
            env::SPRITE_SOURCE_ROW_BYTES
        );
    }
}

/// Keeps a renderer's rigid-frame table (see [`pulsar_scene_model::frames`])
/// current: builds it from the world each frame and uploads it only when a
/// frame moved, so the many objects inside a frame are never touched.
#[derive(Default)]
pub struct FrameTableSync {
    last: Vec<glam::Mat4>,
}

impl FrameTableSync {
    /// Bring `renderer`'s coordinate spaces up to date with `world`'s frames,
    /// relative to `origin` (the frame's world origin; zero outside
    /// camera-relative frames). Returns whether the table changed.
    pub fn sync(
        &mut self,
        world: &pulsar_scenedb::World,
        origin: glam::DVec3,
        renderer: &mut helio::Renderer,
    ) -> bool {
        let table = pulsar_scene_model::frames::frame_table(world, origin);
        if table == self.last {
            return false;
        }
        // Slot 0 (the world) is implicit in the renderer's table.
        renderer.set_frame_spaces(&table[1..]);
        self.last = table;
        true
    }
}

/// The first object that cannot move (authored `Static` or `Stationary`) among
/// `entity` and what moving it would also move: its descendants, up to any
/// rigid frame below it. Members of a frame are not counted when `entity` is
/// the frame itself (they stay fixed in its space), and a frame nested inside
/// the group is left alone for the same reason.
pub fn first_fixed_in_group(
    world: &pulsar_scenedb::World,
    entity: pulsar_scenedb::Entity,
) -> Option<(pulsar_scenedb::Entity, helio::Movability)> {
    use pulsar_scene_model::frames::{frame_above, Frame};
    let fixed = |candidate| {
        let movability = helio_component::components::object_movability(world, candidate)?;
        (!movability.can_move()).then_some((candidate, movability))
    };
    if let found @ Some(_) = fixed(entity) {
        return found;
    }
    if world.get::<Frame>(entity).is_some() {
        return None;
    }
    let space = frame_above(world, entity);
    pulsar_scene_model::hierarchy_transform::descendants_of(world, entity)
        .into_iter()
        .filter(|descendant| frame_above(world, *descendant) == space)
        .find_map(fixed)
}
