//! Frontend-owned acceleration projection of SceneDB's static shadow casters.
//! No renderer-owned scene table or GPU readback is involved.
//!
//! A host adapter: it reads a CPU `World`'s host components through change
//! cursors, so it lives with them (moved from helio-default-passes in the
//! layered rewrite's phase 1). Passes consume only what the renderer publishes
//! of it ([`SceneDbRayTracing::publish`]): the TLAS, transmission rows
//! (`helio_pass_hlfs`, Pulsar-Native#1066) and each instance's surface
//! (`RayInstance`: the GI probes' mesh tracer shades its hits from them).
//! With [`SceneDbRayTracing::with_virtual_geometry`], virtual-geometry
//! instances are traced through their assets' proxies
//! ([`crate::ray_proxies`]).
//!
//! Cut-outs (alpha-tested materials: leaves, fences) go in non-opaque, their
//! rows flagged `RAY_INSTANCE_MASKED` with their material's row: a ray query
//! that reads the rows tests each candidate's alpha
//! (`helio_mats::vg_alpha`, the raster's test) and commits only where the
//! surface is. A partial projection holds them; a full one
//! ([`SceneDbRayTracing::new`], for consumers that take every candidate as
//! glass or as opaque) still refuses them.
use crate::ray_proxies::{Lookup, VgRayProxies, VgRayStats};
use crate::{MeshComponent, RenderGroupComponent, StaticObjectComponent};
use helio_core::{BlasGeometry, BlasManager, Movability, TlasInstanceInput, TlasManager};
use helio_mats::host_rows::MaterialComponent;
use helio_mesh_api::{
    RayInstance, RAY_INSTANCE_MASKED, RAY_INSTANCE_PROXY, RAY_INSTANCE_TWO_SIDED,
};
use pulsar_scenedb::{
    ChangeCursor, ChangeRead, ComponentChange, ComponentChangeKind, Entity, World,
};
use std::{
    collections::{HashMap, HashSet},
    hash::Hasher,
    sync::Arc,
};

/// Explicit linear-RGB transmission for each crossed triangle of a thin sheet.
/// Attach to the material entity. This is independent of display alpha and does
/// not model refraction, thickness, or caustics. Closed meshes attenuate at both
/// entrance and exit surfaces; use a single surface for a single pane.
#[derive(Clone, Copy, Debug)]
pub struct RayTransmission(pub [f32; 3]);

/// Change cursors for every component the projection reads. Objects are
/// applied incrementally; a change to any of the others forces a rebuild.
struct Cursors {
    objects: ChangeCursor,
    others: [ChangeCursor; 5],
}

impl Cursors {
    fn open(world: &World) -> Self {
        Self {
            objects: world.open_change_cursor::<StaticObjectComponent>(),
            others: [
                world.open_change_cursor::<MeshComponent>(),
                world.open_change_cursor::<MaterialComponent>(),
                world.open_change_cursor::<RayTransmission>(),
                world.open_change_cursor::<RenderGroupComponent>(),
                world.open_change_cursor::<Movability>(),
            ],
        }
    }
}

/// Every `StaticObjectComponent` field the projection depends on except the
/// transform. A change that leaves this equal only moves the TLAS instance.
#[derive(Clone, Copy, PartialEq, Eq)]
struct CasterKey {
    mesh: (u32, u32),
    material: (u32, u32),
    draw: (u32, u32, i32),
    graph_hash: u64,
    flags: u32,
}

impl CasterKey {
    fn of(object: &StaticObjectComponent) -> Self {
        Self {
            mesh: (object.mesh_slot, object.mesh_generation),
            material: (object.material_slot, object.material_generation),
            draw: (object.index_count, object.first_index, object.vertex_offset),
            graph_hash: object.row().graph_hash(),
            flags: object.flags,
        }
    }
}

/// A caster material, resolved once a build.
#[derive(Clone, Copy)]
struct MaterialRow {
    generation: u32,
    rgb: [f32; 3],
    transmits: bool,
    /// Whether the projection can represent it (opaque, explicitly
    /// transmitting, or masked in a partial projection; no custom shader).
    supported: bool,
    /// `RAY_INSTANCE_MASKED` and `RAY_INSTANCE_TWO_SIDED` bits.
    flags: u32,
    base_color: [f32; 4],
    emissive: [f32; 4],
}

fn casts_shadow(object: &StaticObjectComponent) -> bool {
    object.flags & helio_mesh_api::INSTANCE_FLAG_CASTS_SHADOW != 0
}

/// Row-major 3x4 TLAS transform of an object's column-major world matrix.
fn tlas_transform(object: &StaticObjectComponent) -> [f32; 12] {
    let m = object.row().transform().to_cols_array();
    [
        m[0], m[4], m[8], m[12], m[1], m[5], m[9], m[13], m[2], m[6], m[10], m[14],
    ]
}

pub struct SceneDbRayTracing {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    blas: BlasManager,
    tlas: TlasManager,
    transmission: Option<wgpu::Buffer>,
    has_transmission: bool,
    transmission_rows: Vec<[f32; 4]>,
    /// Each instance's surface (`RayInstance`), in TLAS order.
    instances_buffer: Option<wgpu::Buffer>,
    instance_rows: Vec<RayInstance>,
    /// Whether objects the projection cannot represent are left out
    /// ([`Self::partial`]) instead of failing the build.
    partial: bool,
    /// Objects left out, by the key they were left out with: their moves
    /// change nothing.
    skipped: HashMap<Entity, CasterKey>,
    geometry_ids: HashMap<(u64, bool), u64>,
    /// BLAS identities of virtual-geometry proxies, by proxy key and
    /// opacity.
    proxy_ids: HashMap<(u64, bool), u64>,
    next_geometry_id: u64,
    /// Virtual-geometry instances' proxies ([`Self::with_virtual_geometry`]).
    vg: Option<VgRayProxies>,
    /// `None` until the first full build, and after any error or journal
    /// overflow: the next `prepare` then rebuilds from a full scan.
    cursors: Option<Cursors>,
    /// TLAS instances and the caster that owns each, in the same order.
    instances: Vec<TlasInstanceInput>,
    shadow_masks: Vec<u8>,
    casters: Vec<(Entity, CasterKey)>,
    caster_slots: HashMap<Entity, usize>,
    changes: Vec<ComponentChange>,
    /// Base colour textures' means: a hit's colour is its material's factor
    /// times its texture's mean.
    texture_means: helio_scene_derivations::texture_means::TextureMeans,
}
impl SceneDbRayTracing {
    /// The scene's shadow casters, all of them: an object the projection
    /// cannot represent (masked or custom-shader materials, group or
    /// coordinate-space visibility, non-triangle geometry families) fails
    /// the build, so ray-traced shadows never silently miss a caster nor
    /// take a cut-out for a solid card.
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Self::with_partial(device, queue, false)
    }

    /// Every object the projection can represent, shadow caster
    /// (`INSTANCE_FLAG_CASTS_SHADOW`) or not, cut-outs included (module
    /// docs); the others (and geometry families it has no projection for:
    /// foliage, voxel volumes, sectioned objects; virtual geometry without
    /// [`Self::with_virtual_geometry`]) are left out. For consumers that
    /// tolerate a missing object and alpha-test cut-outs, such as the GI
    /// probes, whose rays then see past it.
    pub fn partial(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Self::with_partial(device, queue, true)
    }

    fn with_partial(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>, partial: bool) -> Self {
        Self {
            texture_means: helio_scene_derivations::texture_means::TextureMeans::new(&device),
            blas: BlasManager::new(device.clone()),
            tlas: TlasManager::new(device.clone(), 1),
            device,
            queue,
            transmission: None,
            has_transmission: false,
            transmission_rows: Vec::new(),
            instances_buffer: None,
            instance_rows: Vec::new(),
            partial,
            skipped: HashMap::new(),
            geometry_ids: HashMap::new(),
            proxy_ids: HashMap::new(),
            next_geometry_id: 0,
            vg: None,
            cursors: None,
            instances: Vec::new(),
            shadow_masks: Vec::new(),
            casters: Vec::new(),
            caster_slots: HashMap::new(),
            changes: Vec::new(),
        }
    }

    /// Traces virtual-geometry instances (static objects whose mesh is an
    /// asset of `runtime`) through their assets' proxies instead of their
    /// classic meshes ([`crate::ray_proxies`]). An instance whose proxy is
    /// still building is left out (rays pass it); in a full projection
    /// ([`Self::new`]) that fails the build until it is ready.
    pub fn with_virtual_geometry(mut self, runtime: helio_vg_stream::VgRuntime) -> Self {
        self.vg = Some(VgRayProxies::new(runtime));
        self.cursors = None;
        self
    }

    /// The virtual-geometry proxies' counters (zero without them).
    pub fn vg_stats(&self) -> VgRayStats {
        self.vg
            .as_ref()
            .map_or_else(VgRayStats::default, VgRayProxies::stats)
    }

    /// Call after flushing the World GPU mirror and before rendering. The World
    /// remains authoritative.
    ///
    /// The first call scans every object row and builds each referenced
    /// mesh/opacity variant's BLAS. Later calls read SceneDB change journals
    /// (their own cursors; no other client's events are consumed): a frame
    /// with no changes does no per-object work, and a transform-only change
    /// to existing casters rewrites just those TLAS instances. Anything else
    /// (a caster added or removed, a mesh, material, transmission, render
    /// group or `Movability` change) rescans, as does a journal overflow.
    ///
    /// A rescan hashes each referenced mesh to detect in-place vertex edits.
    /// Insert a non-deforming [`Movability`] (anything but `Dynamic`) on a
    /// mesh entity to skip that hash: its BLAS is then built once and reused
    /// until SceneDB reallocates or replaces the mesh's geometry range.
    pub fn prepare(&mut self, world: &World) -> helio_core::Result<&wgpu::Tlas> {
        // Texture means that arrived change the instances' colours: rebuild.
        if self.texture_means.collect() {
            self.cursors = None;
        }
        // Proxies that arrived or went change the instances: rebuild.
        if self
            .vg
            .as_mut()
            .is_some_and(|vg| vg.poll(&self.device, &self.queue))
        {
            self.cursors = None;
        }
        let result = match self.update(world) {
            Ok(true) => Ok(()),
            Ok(false) => self.build(world),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            self.blas.clear();
            self.geometry_ids.clear();
            self.proxy_ids.clear();
            self.tlas.invalidate();
            self.cursors = None;
            return Err(error);
        }
        self.texture_means.dispatch(&self.device, &self.queue);
        Ok(self.tlas.tlas().expect("successful TLAS build"))
    }

    /// Valid only after successful preparation; rows follow TLAS instance order.
    pub fn transmission(&self) -> Option<&wgpu::Buffer> {
        self.has_transmission
            .then_some(self.transmission.as_ref())
            .flatten()
    }

    pub fn tlas(&self) -> Option<&wgpu::Tlas> {
        self.tlas.tlas()
    }

    /// The frame's ray-traced scene (`helio_frame_api::rays::SCENE_RAYS`):
    /// the TLAS and the buffers that follow it, after a successful
    /// preparation.
    pub fn scene_rays(&self) -> Option<helio_frame_api::rays::SceneRays> {
        let mut rays = helio_frame_api::rays::SceneRays::new(self.tlas()?.clone());
        if let Some(transmission) = self.transmission() {
            rays = rays.with(
                helio_frame_api::rays::RAY_TRANSMISSION,
                transmission.clone(),
            );
        }
        if let Some(instances) = &self.instances_buffer {
            rays = rays.with(helio_mesh_api::RAY_INSTANCES, instances.clone());
        }
        if let Some((vertices, indices)) = self.vg.as_ref().and_then(VgRayProxies::buffers) {
            rays = rays
                .with(helio_mesh_api::RAY_PROXY_VERTICES, vertices.clone())
                .with(helio_mesh_api::RAY_PROXY_INDICES, indices.clone());
        }
        Some(rays)
    }

    /// Publishes [`Self::scene_rays`] for `renderer`'s next frame (nothing
    /// when the preparation failed: that frame has no TLAS).
    pub fn publish(&self, renderer: &mut helio::Renderer) {
        if let Some(rays) = self.scene_rays() {
            renderer.set_scene_payload(
                &helio_frame_api::rays::SCENE_RAYS,
                std::sync::Arc::new(rays),
            );
        }
    }

    /// Whether `object` is projected: a shadow caster, or any object in a
    /// partial projection.
    fn projects(&self, object: &StaticObjectComponent) -> bool {
        self.partial || casts_shadow(object)
    }

    /// Applies journaled changes to the previous build. `Ok(false)` means
    /// the changes need a full rebuild (or there is nothing to update yet).
    fn update(&mut self, world: &World) -> helio_core::Result<bool> {
        let Some(cursors) = self.cursors.as_mut() else {
            return Ok(false);
        };
        if self.tlas.tlas().is_none() {
            return Ok(false);
        }
        if !self.partial {
            reject_unsupported_geometry(world, self.vg.is_some())?;
        }
        self.changes.clear();
        for cursor in &mut cursors.others {
            if world.read_changes(cursor, &mut self.changes) != ChangeRead::Complete
                || !self.changes.is_empty()
            {
                return Ok(false);
            }
        }
        if world.read_changes(&mut cursors.objects, &mut self.changes) != ChangeRead::Complete {
            return Ok(false);
        }
        let mut moved = false;
        for change in &self.changes {
            let slot = self.caster_slots.get(&change.entity).copied();
            let object = match change.kind {
                ComponentChangeKind::Removed => None,
                ComponentChangeKind::Inserted | ComponentChangeKind::Mutated => {
                    world.get::<StaticObjectComponent>(change.entity)
                }
            };
            match (slot, object) {
                // A non-caster changed and still is not one.
                (None, None) => {
                    self.skipped.remove(&change.entity);
                }
                (None, Some(object)) if !self.projects(object) => {
                    self.skipped.remove(&change.entity);
                }
                // One left out moved, or went.
                (None, object)
                    if self
                        .skipped
                        .get(&change.entity)
                        .is_some_and(|key| object.is_none_or(|o| CasterKey::of(o) == *key)) => {}
                (Some(slot), Some(object))
                    if self.projects(object) && CasterKey::of(object) == self.casters[slot].1 =>
                {
                    let transform = tlas_transform(object);
                    if self.instances[slot].transform != transform {
                        self.instances[slot].transform = transform;
                        moved = true;
                    }
                }
                // Caster added, removed or re-keyed.
                _ => return Ok(false),
            }
        }
        if moved {
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SceneDB RT acceleration update"),
                });
            self.tlas
                .build_masked(
                    &mut encoder,
                    &self.instances,
                    &self.blas,
                    &self.shadow_masks,
                )
                .map_err(|e| rt_error(&e.to_string()))?;
            self.queue.submit([encoder.finish()]);
        }
        Ok(true)
    }

    fn build(&mut self, world: &World) -> helio_core::Result<()> {
        self.has_transmission = false;
        // Open before scanning: a change racing the scan is re-applied from
        // the journal next frame, never lost.
        self.cursors = Some(Cursors::open(world));
        self.casters.clear();
        self.caster_slots.clear();
        self.skipped.clear();
        let error = rt_error;
        if !self.partial {
            reject_unsupported_geometry(world, self.vg.is_some())?;
        }
        let store = world
            .gpu_mirror()
            .ok_or_else(|| error("GPU mirror is required"))?
            .store();
        let meshes: HashMap<_, _> = world
            .query::<(&MeshComponent,)>()
            .map(|(entity, (mesh,))| (entity.index(), (entity, mesh)))
            .collect();
        let materials: HashMap<_, _> = world
            .query::<(&MaterialComponent,)>()
            .map(|(entity, (material,))| (entity.index(), (entity, material)))
            .collect();
        let vertices =
            store.resolve_buffer_handle(helio_core::BufferKey::of("builtin_mesh_vertex"));
        let indices = store.resolve_buffer_handle(helio_core::BufferKey::of("builtin_mesh_index"));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SceneDB RT acceleration"),
            });
        let mut live = HashSet::new();
        let mut proxies_used = HashSet::new();
        let mut mesh_ranges = HashMap::new();
        let mut material_rows = HashMap::new();
        let mut instances = Vec::new();
        let mut transmission_rows = Vec::<[f32; 4]>::new();
        let mut instance_rows = Vec::<RayInstance>::new();
        let mut has_transmission = false;
        for (object_entity, (object,)) in world.query::<(&StaticObjectComponent,)>() {
            if !self.projects(object) {
                continue;
            }
            // An object the projection cannot represent: left out of a
            // partial projection, else the build fails.
            macro_rules! unsupported {
                ($message:expr) => {{
                    if self.partial {
                        self.skipped.insert(object_entity, CasterKey::of(object));
                        continue;
                    }
                    return Err(error($message));
                }};
            }
            if world
                .get::<RenderGroupComponent>(object_entity)
                .is_some_and(|group| group.group_mask != 0)
            {
                unsupported!("group visibility needs an explicit RT projection");
            }
            if helio_mesh_api::coordinate_space(object.flags) != 0 {
                unsupported!("non-world coordinate-space casters are unsupported");
            }
            // Resolve each material once per frame; many objects share one.
            let row = match material_rows.get(&object.material_slot) {
                Some(row) => *row,
                None => {
                    let (material_entity, material) = materials
                        .get(&object.material_slot)
                        .ok_or_else(|| error("missing caster material"))?;
                    let transmission = world.get::<RayTransmission>(*material_entity);
                    let masked = material.flags & helio_mats::FLAG_ALPHA_TEST != 0;
                    let supported = material.flags & helio_mats::FLAG_HAS_CUSTOM_SHADER == 0
                        && if masked {
                            self.partial && transmission.is_none()
                        } else {
                            material.flags & helio_mats::FLAG_ALPHA_BLEND == 0
                                || transmission.is_some()
                        };
                    let rgb = transmission.map_or([0.0; 3], |value| value.0);
                    if !rgb.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
                        return Err(error("transmission must be finite linear RGB in [0, 1]"));
                    }
                    // The factor times the base colour texture's mean
                    // (white until measured).
                    let texture = (material.tex_base_color != helio_mats::GpuMaterial::NO_TEXTURE)
                        .then(|| world.gpu_mirror().and_then(|m| m.texture_store()))
                        .flatten()
                        .and_then(|store| {
                            store.read().ok()?.texture(material.tex_base_color).cloned()
                        });
                    let mean = texture
                        .and_then(|t| self.texture_means.mean(&t))
                        .unwrap_or([1.0; 3]);
                    let [r, g, b, _] = material.base_color;
                    let [r, g, b] = [r * mean[0], g * mean[1], b * mean[2]];
                    let [er, eg, eb, strength] = material.emissive;
                    let two_sided = material.flags & helio_mats::FLAG_DOUBLE_SIDED != 0;
                    let row = MaterialRow {
                        generation: material_entity.generation().wrapping_add(1),
                        rgb,
                        transmits: transmission.is_some(),
                        supported,
                        flags: if masked { RAY_INSTANCE_MASKED } else { 0 }
                            | if two_sided { RAY_INSTANCE_TWO_SIDED } else { 0 },
                        base_color: [r, g, b, 1.0],
                        emissive: [er * strength, eg * strength, eb * strength, 0.0],
                    };
                    material_rows.insert(object.material_slot, row);
                    row
                }
            };
            if !row.supported || object.row().graph_hash() != 0 {
                unsupported!("custom-shader casters, transparent casters without explicit RayTransmission, and masked casters (in a full projection, or transmitting) are unsupported");
            }
            if row.generation != object.material_generation {
                return Err(error("stale caster material identity"));
            }
            let (rgb, transmits) = (row.rgb, row.transmits);
            // Glass and cut-outs are candidates; everything else is opaque.
            let opaque = rgb.iter().all(|v| *v == 0.0) && row.flags & RAY_INSTANCE_MASKED == 0;
            let (entity, mesh) = meshes
                .get(&object.mesh_slot)
                .ok_or_else(|| error("missing caster mesh"))?;
            if entity.generation().wrapping_add(1) != object.mesh_generation {
                return Err(error("stale caster mesh identity"));
            }
            let mesh_identity = ((entity.generation() as u64) << 32) | entity.index() as u64;
            // Resolve each mesh once per frame; many objects share one.
            let (vertex_range, index_range) = match mesh_ranges.get(&mesh_identity) {
                Some(ranges) => *ranges,
                None => {
                    let ranges = (
                        MeshComponent::vertices_gpu_handle(store, entity.index())
                            .ok_or_else(|| error("missing mirrored vertices"))?,
                        MeshComponent::indices_gpu_handle(store, entity.index())
                            .ok_or_else(|| error("missing mirrored indices"))?,
                    );
                    mesh_ranges.insert(mesh_identity, ranges);
                    ranges
                }
            };
            if object.index_count != index_range.count
                || object.first_index != index_range.offset
                || object.vertex_offset != vertex_range.offset as i32
            {
                return Err(error("caster draw range differs from current mesh; refresh the StaticObjectComponent"));
            }
            // A virtual-geometry asset: traced through its proxy, whatever
            // its classic mesh is (a fallback or a placeholder).
            let lookup = match self.vg.as_mut() {
                Some(vg) => vg.lookup((vertex_range.offset, vertex_range.count)),
                None => Lookup::Classic,
            };
            let (id, first_index, base_vertex, flags) = match lookup {
                Lookup::Pending => unsupported!("virtual geometry whose proxy is not built yet"),
                Lookup::Proxy(..) if transmits => unsupported!("transmitting virtual geometry"),
                Lookup::Proxy(key, range) => {
                    let id = match self.proxy_ids.get(&(key, opaque)) {
                        Some(id) => *id,
                        None => {
                            let id = self.next_geometry_id;
                            self.next_geometry_id = id
                                .checked_add(1)
                                .ok_or_else(|| error("BLAS identity exhausted"))?;
                            self.proxy_ids.insert((key, opaque), id);
                            id
                        }
                    };
                    proxies_used.insert(key);
                    if live.insert(id) {
                        let (vertices, proxy_indices) = self
                            .vg
                            .as_ref()
                            .and_then(VgRayProxies::buffers)
                            .ok_or_else(|| error("missing proxy pool"))?;
                        self.blas
                            .build_from_buffers_with_opacity(
                                id,
                                &mut encoder,
                                BlasGeometry {
                                    revision: 0,
                                    vertices,
                                    first_vertex: range.first_vertex,
                                    vertex_count: range.vertex_count,
                                    vertex_stride: helio_mesh_api::RAY_PROXY_VERTEX_BYTES,
                                    indices: Some(proxy_indices),
                                    first_index: range.first_index,
                                    index_count: range.index_count,
                                },
                                opaque,
                            )
                            .map_err(|e| error(&e.to_string()))?;
                    }
                    (
                        id,
                        range.first_index,
                        range.first_vertex,
                        RAY_INSTANCE_PROXY,
                    )
                }
                Lookup::Classic => {
                    // A mesh can be instanced with both opaque and non-opaque materials.
                    // Allocate separate BLAS identities without truncating entity generations.
                    let id = if let Some(id) = self.geometry_ids.get(&(mesh_identity, opaque)) {
                        *id
                    } else {
                        let id = self.next_geometry_id;
                        self.next_geometry_id = id
                            .checked_add(1)
                            .ok_or_else(|| error("BLAS identity exhausted"))?;
                        self.geometry_ids.insert((mesh_identity, opaque), id);
                        id
                    };
                    if live.insert(id) {
                        if vertex_range.count as usize != mesh.vertices.len()
                            || index_range.count as usize != mesh.indices.len()
                        {
                            return Err(error(
                                "mesh mirror is not current; flush SceneDB before RT preparation",
                            ));
                        }
                        // A mesh without `Movability` keeps content-based invalidation
                        // for in-place edits. A mesh that promises not to deform is
                        // never re-read: its BLAS key (buffer, ranges, opacity) still
                        // rebuilds it when SceneDB moves or replaces the allocation.
                        let revision = if world
                            .get::<Movability>(*entity)
                            .is_none_or(|movability| movability.can_deform())
                        {
                            // Streaming hash's vectorized bulk path for large meshes.
                            let mut hash = twox_hash::XxHash3_64::default();
                            hash.write(bytemuck::cast_slice(&mesh.vertices));
                            hash.write(bytemuck::cast_slice(&mesh.indices));
                            hash.finish()
                        } else {
                            0
                        };
                        self.blas
                            .build_from_buffers_with_opacity(
                                id,
                                &mut encoder,
                                BlasGeometry {
                                    revision,
                                    vertices: &vertices
                                        .as_ref()
                                        .ok_or_else(|| error("missing vertex pool"))?
                                        .buffer,
                                    first_vertex: vertex_range.offset,
                                    vertex_count: vertex_range.count,
                                    vertex_stride: std::mem::size_of::<helio_mesh_api::PackedVertex>()
                                        as u64,
                                    indices: Some(
                                        &indices
                                            .as_ref()
                                            .ok_or_else(|| error("missing index pool"))?
                                            .buffer,
                                    ),
                                    first_index: index_range.offset,
                                    index_count: index_range.count,
                                },
                                opaque,
                            )
                            .map_err(|e| error(&e.to_string()))?;
                    }
                    (id, index_range.offset, vertex_range.offset, 0)
                }
            };
            has_transmission |= transmits;
            transmission_rows.push([rgb[0], rgb[1], rgb[2], 0.0]);
            instance_rows.push(RayInstance {
                first_index,
                base_vertex,
                flags: flags | row.flags,
                material: object.material_slot,
                base_color: row.base_color,
                emissive: row.emissive,
                transmission: [rgb[0], rgb[1], rgb[2], 0.0],
            });
            self.caster_slots.insert(object_entity, self.casters.len());
            self.casters.push((object_entity, CasterKey::of(object)));
            instances.push(TlasInstanceInput {
                mesh_id: id,
                transform: tlas_transform(object),
            });
        }
        if let Some(vg) = self.vg.as_mut() {
            vg.retain(&proxies_used);
        }
        self.proxy_ids.retain(|_, id| live.contains(id));
        self.blas.retain(|id| live.contains(&id));
        self.geometry_ids.retain(|_, id| live.contains(id));
        self.shadow_masks = self
            .casters
            .iter()
            .map(|(_, key)| {
                if key.flags & helio_mesh_api::INSTANCE_FLAG_MOVABLE != 0 {
                    2
                } else {
                    1
                }
            })
            .collect();
        self.tlas
            .build_masked(&mut encoder, &instances, &self.blas, &self.shadow_masks)
            .map_err(|e| error(&e.to_string()))?;
        if has_transmission {
            let size = ((transmission_rows.len() + 1) * 16).max(32) as u64;
            let mut buffer_needs_upload = false;
            if self
                .transmission
                .as_ref()
                .is_none_or(|buffer| buffer.size() < size)
            {
                self.transmission = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("SceneDB RT thin-sheet transmission"),
                    size: size.next_power_of_two(),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
                buffer_needs_upload = true;
            }
            let buffer = self.transmission.as_ref().unwrap();
            if buffer_needs_upload || self.transmission_rows != transmission_rows {
                let header = [1u32, transmission_rows.len() as u32, 0, 0];
                self.queue
                    .write_buffer(buffer, 0, bytemuck::cast_slice(&header));
                self.queue
                    .write_buffer(buffer, 16, bytemuck::cast_slice(&transmission_rows));
                self.transmission_rows = transmission_rows;
            }
        } else {
            self.transmission_rows.clear();
        }
        self.has_transmission = has_transmission;
        // The instances' surfaces, a row each (at least one: a binding).
        let size = (instance_rows.len().max(1) * std::mem::size_of::<RayInstance>()) as u64;
        if self
            .instances_buffer
            .as_ref()
            .is_none_or(|buffer| buffer.size() < size)
        {
            self.instances_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("SceneDB RT instance surfaces"),
                size: size.next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.instance_rows.clear();
        }
        if self.instance_rows != instance_rows && !instance_rows.is_empty() {
            self.queue.write_buffer(
                self.instances_buffer.as_ref().unwrap(),
                0,
                bytemuck::cast_slice(&instance_rows),
            );
        }
        self.instance_rows = instance_rows;
        self.instances = instances;
        self.queue.submit([encoder.finish()]);
        Ok(())
    }
}

fn rt_error(message: &str) -> helio_core::Error {
    helio_core::Error::InvalidPassConfig(format!("SceneDB RT: {message}"))
}

/// These geometry families need their own explicit projection. Never
/// silently omit known non-triangle or multi-material scene content.
/// `virtual_geometry`: VG instances are projected through proxies.
fn reject_unsupported_geometry(world: &World, virtual_geometry: bool) -> helio_core::Result<()> {
    let store = world
        .gpu_mirror()
        .ok_or_else(|| rt_error("GPU mirror is required"))?
        .store();
    for name in [
        "sectioned_objects",
        "foliage_layers",
        "voxel_volumes",
        "vg_instances",
    ] {
        if virtual_geometry && name == "vg_instances" {
            continue;
        }
        if store
            .resolve_buffer_handle(helio_core::BufferKey::of(name))
            .is_some()
        {
            return Err(rt_error(&format!("unsupported geometry buffer {name}")));
        }
    }
    Ok(())
}
