//! SceneDB's GPU-mirror attachment seam for Helio 3.0.
//!
//! SceneDB owns all world and asset state; component values mirror their own
//! GPU rows on every write. Helio receives the cloneable GPU mirror during
//! renderer construction, and the object joins ([`scene_join`]: every
//! plugin's [`ObjectJoinRegistration`], in their order: the meshes', the
//! lights') turn the authored rows (mesh and light instances, their owner
//! links, the owner objects' transforms and visibility) into the object,
//! material and light rows its passes draw, on the GPU, whenever they
//! change. This module keeps no frame projection, render-row
//! subscription, material table or per-frame input assembly
//! (Pulsar-Native#1035, Phase 2). Every other kind's row join
//! ([`environment_join`]) does the same for its rows (Phase 4); each kind's
//! join is its feature crate's (`scene_rows`). The engine runs them and
//! knows none: a plugin's kind registers its source rows and its join
//! ([`SceneRowsRegistration`]), and the spaces it puts in the frame's table
//! ([`SpaceProducerRegistration`]).
use std::sync::Arc;

use helio_core::ENTITY_GENERATIONS_KEY;
use helio_scene_derivations::hierarchy_resolve::{HierarchyResolve, HierarchyResolveKeys};
use helio_scene_join::{DerivationGroup, RowJoinKeys, SceneDerivation};
use pulsar_scenedb::gpu::{
    BufferKey, EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};

use pulsar_scene_model::attachments::ComponentOwner;
use pulsar_scene_model::{ObjectFrame, ObjectHidden, ObjectParent, TransformRow};

/// Where this engine's object rows live: the instance owner links
/// (`ComponentOwner`), entity generations, object visibility (`Visibility`'s
/// derived `ObjectHidden`) and transforms (`Transform`'s derived
/// `TransformRow`: the f64 position as an f32 pair).
fn object_keys() -> (BufferKey, BufferKey, BufferKey, BufferKey) {
    (
        BufferKey::of("component_owners"),
        ENTITY_GENERATIONS_KEY,
        BufferKey::of("object_hidden"),
        BufferKey::of("object_transforms"),
    )
}

/// The rows an object join reads ([`ObjectJoinRegistration`]): this
/// engine's owner links, generations, visibility and transforms, and
/// whether the renderer is the editor's (which adds editor-only rows, such
/// as the lights' icons).
#[derive(Clone, Copy, Debug)]
pub struct ObjectJoinContext {
    pub owners: BufferKey,
    pub generations: BufferKey,
    pub hidden: BufferKey,
    pub transforms: BufferKey,
    pub editor: bool,
}

/// A plugin's object join: a derivation turning its instances' source rows
/// into the rows its passes read, placed by their owners, installed in the
/// engine's scene-join group in `order` (the meshes' first), ahead of the
/// render-host parts' derivations (`RenderHostPartRegistration`) and every
/// row join ([`environment_join`]), so the rows it publishes (the lights'
/// list and directional light) are there for them in the same frame. The
/// plugin's root crate submits one with `inventory::submit!`; the
/// engine names no kind.
pub struct ObjectJoinRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// For diagnostics and tests.
    pub name: &'static str,
    /// Where the join runs in the group: lower first. A join reading
    /// another's output runs after it.
    pub order: i32,
    /// The rows of each instance's local bounding sphere (`[center.xyz,
    /// radius]`) this join's kind authors, when row joins may place their
    /// rows by an owner's content ([`row_join_keys`]).
    pub owner_bounds: Option<BufferKey>,
    /// Registers the join's source-row columns (initial capacities; they
    /// grow) with the GPU mirror ([`ensure_gpu_mirror`]).
    pub register_columns: fn(&mut SceneGpuStore, &Arc<wgpu::Device>),
    pub derivation: fn(&wgpu::Device, &ObjectJoinContext) -> Box<dyn SceneDerivation>,
}

impl pulsar_plugins::Contribution for ObjectJoinRegistration {
    const CATEGORY: &'static str = "scene.object_join";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(ObjectJoinRegistration);

/// The meshes' scene join and every plugin's object join over this
/// engine's rows, for `RendererBuilder::with_scene_derivation`. `editor`
/// lets a join add editor-only rows.
pub fn scene_join(device: &wgpu::Device, editor: bool) -> Box<DerivationGroup> {
    Box::new(DerivationGroup::new(
        "SceneJoins",
        object_joins()
            .into_iter()
            .map(|registration| (registration.derivation)(device, &object_join_context(editor)))
            .collect(),
    ))
}

/// The rows an object join reads, for a renderer that is the editor's or
/// not.
pub fn object_join_context(editor: bool) -> ObjectJoinContext {
    let (owners, generations, hidden, transforms) = object_keys();
    ObjectJoinContext {
        owners,
        generations,
        hidden,
        transforms,
        editor,
    }
}

/// Every enabled plugin's object join, in order.
fn object_joins() -> Vec<&'static ObjectJoinRegistration> {
    let mut joins: Vec<_> = pulsar_plugins::registered::<ObjectJoinRegistration>().collect();
    joins.sort_by_key(|registration| (registration.order, registration.name));
    joins
}

/// Where this engine's parent-relative transforms and parent links live, for
/// Helio's hierarchy resolve. It republishes world-space transforms under the
/// transform key, so it must be installed before the scene and environment
/// joins.
pub fn hierarchy_resolve_keys() -> HierarchyResolveKeys {
    HierarchyResolveKeys {
        transforms: object_keys().3,
        parents: BufferKey::of("object_parent"),
        frames: BufferKey::of("object_frame"),
    }
}

/// Helio's hierarchy resolve, for `RendererBuilder::with_scene_derivation`,
/// ahead of [`scene_join`] and [`environment_join`].
pub fn hierarchy_resolve(device: &wgpu::Device) -> Box<HierarchyResolve> {
    Box::new(HierarchyResolve::new(device, hierarchy_resolve_keys()))
}

/// The rows every kind's row join reads: the same owner, visibility and
/// transform rows as the object joins, and the instances' bounding spheres
/// an object join's kind authors (its `owner_bounds`: the meshes'), which
/// row joins may place by. With no such kind enabled the key names rows
/// nobody writes, and a placement by an owner's content finds none. No
/// directional light: a kind whose rows take a sun names the lights' (water
/// does).
pub fn row_join_keys() -> RowJoinKeys {
    let (owners, generations, hidden, transforms) = object_keys();
    RowJoinKeys {
        owners,
        generations,
        hidden,
        transforms,
        owner_bounds: object_joins()
            .into_iter()
            .find_map(|registration| registration.owner_bounds)
            .unwrap_or(NO_OWNER_BOUNDS),
        directional_light: None,
    }
}

/// The bounds key [`row_join_keys`] names when no enabled kind authors
/// owner bounds.
pub const NO_OWNER_BOUNDS: BufferKey = BufferKey::of("owner_bounds_none");

/// A plugin's kind of scene rows: the source rows its components derive
/// (SceneDB columns, registered with the GPU mirror by
/// [`ensure_gpu_mirror`]) and the derivation that turns them into the rows
/// its passes read ([`environment_join`] runs it). The engine knows
/// neither: the plugin's root crate submits one with
/// `inventory::submit!`.
pub struct SceneRowsRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// Stable id of the registration within its plugin (`"decals.rows"`): what
    /// the plugin catalog lists it by.
    pub name: &'static str,
    /// Registers the kind's source-row columns (initial capacities; they
    /// grow).
    pub register_columns: fn(&mut SceneGpuStore, &Arc<wgpu::Device>),
    /// The kind's derivation over this engine's rows ([`row_join_keys`]),
    /// or `None` for rows another kind's derivation joins (physics' water
    /// hitbox rows, which the water plugin's join reads).
    pub derivation: Option<fn(&wgpu::Device, RowJoinKeys) -> Box<dyn SceneDerivation>>,
}

impl pulsar_plugins::Contribution for SceneRowsRegistration {
    const CATEGORY: &'static str = "scene.rows";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(SceneRowsRegistration);

/// Every other kind's row join over this engine's rows (each kind's
/// feature crate's `scene_rows`), for `RendererBuilder::with_scene_derivation`,
/// after [`scene_join`] (a kind may read what an object join published,
/// the lights' directional light):
/// every plugin's kind ([`SceneRowsRegistration`]).
pub fn environment_join(device: &wgpu::Device) -> Box<DerivationGroup> {
    let keys = row_join_keys();
    let joins: Vec<Box<dyn SceneDerivation>> =
        pulsar_plugins::registered::<SceneRowsRegistration>()
            .filter_map(|registration| registration.derivation)
            .map(|derivation| derivation(device, keys))
            .collect();
    Box::new(DerivationGroup::new("EnvironmentJoins", joins))
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
    // deduplication behavior. Every object join's source rows (the meshes',
    // the lights').
    for registration in pulsar_plugins::registered::<ObjectJoinRegistration>() {
        (registration.register_columns)(&mut gpu_store, &device);
    }
    ComponentOwner::register_gpu_columns_growable(&mut gpu_store, 4096, &device);
    ObjectHidden::register_gpu_columns_growable(&mut gpu_store, 1024, &device);
    // Each object's parent, for the hierarchy resolve (transforms are parent-relative).
    ObjectParent::register_gpu_columns_growable(&mut gpu_store, 1024, &device);
    // Which objects are rigid frames, and their slot in the coordinate-space table.
    ObjectFrame::register_gpu_columns_growable(&mut gpu_store, 64, &device);
    TransformRow::register_gpu_columns_growable(&mut gpu_store, 1024, &device);
    // Every plugin's kind's source rows.
    for registration in pulsar_plugins::registered::<SceneRowsRegistration>() {
        (registration.register_columns)(&mut gpu_store, &device);
    }

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

    // The scene's textures are the texture table every reader binds: as many
    // as each can address on this device.
    let texture_table_capacity =
        helio_core::texture_table_capacity(device.features(), &device.limits());
    let texture_store = Arc::new(std::sync::RwLock::new(
        pulsar_scenedb::gpu::TextureStore::new(texture_table_capacity as u32),
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
        editor_task_queue::spawn_named(
            editor_task_queue::ThreadSpec::new(
                "engine backend: install derived cache",
                editor_task_queue::ThreadRole::Io,
            )
            .owner("engine_backend"),
            move || match pulsar_scenedb::prune_derived_cache(budget_mb * 1024 * 1024) {
                Ok(report) if report.removed_files > 0 => tracing::info!(
                    "derived-data cache pruned: removed {} files ({} MiB), {} MiB kept",
                    report.removed_files,
                    report.removed_bytes / (1024 * 1024),
                    report.kept_bytes / (1024 * 1024)
                ),
                Ok(_) => {}
                Err(error) => tracing::debug!("derived-data cache not pruned: {error}"),
            },
        );
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_new_scene_does_not_have_a_gpu_mirror() {
        let scene_db = pulsar_scenedb::SceneDb::new();
        assert!(!scene_db.world.has_gpu_mirror());
    }
}

/// The one coordinate-space table. Spaces objects define (rigid frames, sublevel
/// placements; `pulsar_scene_model::frames`) fill slots `1..` in slot order;
/// every other producer ([`SpaceProducerRegistration`]: a portal projection)
/// asks this for slots through [`CoordinateSpaceAllocator`], so no producer
/// numbers the table itself and none can overwrite another.
pub struct SpaceTable {
    matrices: Vec<glam::Mat4>,
    /// Slots handed out by `space_for`, to reuse an equal matrix.
    allocated: std::collections::HashMap<[u32; 16], u32>,
}

impl SpaceTable {
    /// A table whose first slots are the spaces objects define (`frames[0]` is
    /// the world).
    pub fn new(frames: Vec<glam::Mat4>) -> Self {
        let mut matrices = frames;
        if matrices.is_empty() {
            matrices.push(glam::Mat4::IDENTITY);
        }
        Self {
            matrices,
            allocated: Default::default(),
        }
    }

    /// Every slot, the world first.
    pub fn matrices(&self) -> &[glam::Mat4] {
        &self.matrices
    }
}

impl helio_core::CoordinateSpaceAllocator for SpaceTable {
    fn space_for(&mut self, matrix: glam::Mat4) -> Option<u32> {
        if matrix == glam::Mat4::IDENTITY {
            return Some(0);
        }
        let key = matrix.to_cols_array().map(f32::to_bits);
        if let Some(&slot) = self.allocated.get(&key) {
            return Some(slot);
        }
        // 8 bits of an instance's flags carry the space id.
        if self.matrices.len() >= 256 {
            return None;
        }
        let slot = self.matrices.len() as u32;
        self.matrices.push(matrix);
        self.allocated.insert(key, slot);
        Some(slot)
    }
}

/// Something besides the objects that define spaces that puts spaces in the
/// frame's coordinate-space table: a portal projection (the matrices mapping
/// what an opening shows into where it shows it). Its plugin registers it
/// (`inventory::submit!`); each renderer makes its own ([`SpaceSync`]) and
/// installs its derivation.
pub struct SpaceProducerRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// For diagnostics.
    pub name: &'static str,
    pub new: fn() -> Box<dyn SpaceProducer>,
}

impl pulsar_plugins::Contribution for SpaceProducerRegistration {
    const CATEGORY: &'static str = "scene.space_producer";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(SpaceProducerRegistration);

/// One renderer's space producer ([`SpaceProducerRegistration`]).
pub trait SpaceProducer: Send + Sync {
    /// The coming frame's camera (view-projection and position, relative to
    /// the frame's origin), so it can skip what the camera cannot see.
    fn set_view(&mut self, _view_projection: glam::Mat4, _position: glam::Vec3) {}

    /// Take this frame's slots from `table` for `world`, relative to `origin`
    /// (the frame's world origin), and publish what its passes read about the
    /// frame into `payloads` (by their payload contracts' keys).
    fn apply(
        &mut self,
        world: &pulsar_scenedb::World,
        origin: glam::DVec3,
        table: &mut SpaceTable,
        payloads: &mut helio_core::ScenePayloads,
    );

    /// The scene derivation that uploads what [`Self::apply`] produced, for
    /// `RendererBuilder::with_scene_derivation`; asked once, when the
    /// renderer is built.
    fn derivation(&self, device: &wgpu::Device) -> Option<Box<dyn SceneDerivation>>;
}

/// Keeps a renderer's coordinate-space table current. Each frame it makes sure
/// every space-defining object has its slot (the allocator,
/// `pulsar_scene_model::frames::sync_spaces`, taking the write lock only when
/// something changed), lets every registered producer take its slots
/// ([`SpaceProducerRegistration`]), builds the [`SpaceTable`] and uploads it
/// only when it differs, so the many objects inside a space are never touched.
pub struct SpaceSync {
    last: Vec<glam::Mat4>,
    producers: Vec<Box<dyn SpaceProducer>>,
}

impl Default for SpaceSync {
    /// With every registered producer.
    fn default() -> Self {
        Self {
            last: Vec::new(),
            producers: pulsar_plugins::registered::<SpaceProducerRegistration>()
                .map(|registration| (registration.new)())
                .collect(),
        }
    }
}

impl SpaceSync {
    /// The producers' scene derivations, for `RendererBuilder::with_scene_derivation`.
    pub fn derivations(&self, device: &wgpu::Device) -> Vec<Box<dyn SceneDerivation>> {
        self.producers
            .iter()
            .filter_map(|producer| producer.derivation(device))
            .collect()
    }

    /// The camera of the coming frame (view-projection and position, relative to
    /// the origin given to `sync`), so producers can skip what it cannot see.
    pub fn set_view(&mut self, view_projection: glam::Mat4, position: glam::Vec3) {
        for producer in &mut self.producers {
            producer.set_view(view_projection, position);
        }
    }

    /// Bring `renderer`'s table up to date with `scene`'s spaces, relative to
    /// `origin` (the frame's world origin; zero outside camera-relative
    /// frames), and add what the producers publish about the frame to
    /// `payloads`. Returns whether the table changed.
    pub fn sync(
        &mut self,
        scene: &crate::scene::SharedScene,
        origin: glam::DVec3,
        renderer: &mut helio::Renderer,
        payloads: &mut helio_core::ScenePayloads,
    ) -> bool {
        // Levels that displays show, and the slots of everything that defines a
        // space. Both checks are cheap reads; the write lock is taken only when
        // something changed (a display was added or retargeted, a frame made).
        let needs_sync = {
            let world = &scene.read().world;
            crate::scene::sublevel_display::needs_level_instance_sync(world)
                || pulsar_scene_model::frames::needs_space_sync(world)
        };
        if needs_sync {
            let mut scene = scene.write();
            crate::scene::sublevel_display::sync_level_instances(&mut scene.world);
            pulsar_scene_model::frames::sync_spaces(&mut scene.world);
        }
        let mut table = SpaceTable::new(pulsar_scene_model::frames::frame_table(
            &scene.read().world,
            origin,
        ));
        // Every other producer takes its slots from the same table.
        {
            let world = &scene.read().world;
            for producer in &mut self.producers {
                producer.apply(world, origin, &mut table, payloads);
            }
        }
        if table.matrices == self.last {
            return false;
        }
        // Slot 0 (the world) is implicit in the renderer's table.
        renderer.set_coordinate_spaces(&table.matrices[1..]);
        self.last = table.matrices;
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
    use pulsar_scene_model::frames::{frame_above, SpaceSlot};
    let fixed = |candidate| {
        let movability = helio_component::components::object_movability(world, candidate)?;
        (!movability.can_move()).then_some((candidate, movability))
    };
    if let found @ Some(_) = fixed(entity) {
        return found;
    }
    if world.get::<SpaceSlot>(entity).is_some() {
        return None;
    }
    let space = frame_above(world, entity);
    pulsar_scene_model::hierarchy_transform::descendants_of(world, entity)
        .into_iter()
        .filter(|descendant| frame_above(world, *descendant) == space)
        .find_map(fixed)
}
