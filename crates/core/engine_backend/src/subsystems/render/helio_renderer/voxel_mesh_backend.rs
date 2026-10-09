//! Free-standing voxel objects (`VoxelComponent`) drawn as mesh instances
//! (Pulsar-Native#1056).
//!
//! Each object's live chunks are greedy-meshed on the CPU
//! (`helio_component::voxel_mesh`) and uploaded as the object's mesh rows,
//! the rows a static mesh instance at the same entity would have. Helio's
//! scene join draws them with the owner's transform like any mesh, so the
//! volume moves, rotates and scales with its object and gets the G-buffer,
//! shadows and picking of every mesh. There is no voxel pass.
//!
//! Change-driven: the payload store's revision says when to look, and a
//! chunk is re-meshed only when its payload changed, or a face neighbour's
//! solid border against it did. Small changes (an edit) mesh on the render
//! thread; a large volume meshes on a worker thread, and a result whose
//! chunk changed again meanwhile is discarded by the chunk's revision. The
//! object keeps its last complete mesh on screen until every chunk is
//! current, then uploads the assembled mesh under its content id.
//!
//! The rows belong to the component: SceneDB clears them when it is
//! removed or despawned (a clear registration in `voxel_mesh`). This
//! backend clears them itself only for an object it stops drawing that is
//! still alive (disabled, hidden, invalid), after checking under the scene
//! lock that the row is still that component's.

use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc};

use helio_component::voxel_mesh::{
    clear_voxel_mesh_rows, greedy_mesh_chunk, write_voxel_mesh_rows, VoxelChunkMesh,
    VoxelMeshGeometry, VOXEL_MESH_RENDERER,
};
use helio_component::VoxelComponent;
use helio_default_graphs::VoxelPassFactory;
use helio_voxel_data::{
    VoxelChunkKey, VoxelMaterialChunk, VoxelPayloadStore, VOXEL_CHUNK_ENCODING_RAW,
    VOXEL_CHUNK_SAMPLES,
};
use pulsar_scenedb::Entity;

use super::voxel_backend::{VoxelRenderBackend, VoxelView};
use crate::scene::voxel_frame::{initial_cube_chunks, VoxelCubeInit, VoxelEntryId, VoxelSceneEntry};
use crate::scene::SharedScene;

/// Chunks re-meshed in one frame on the render thread; a larger batch
/// (a newly shown large volume) goes to the worker.
const INLINE_CHUNKS: usize = 64;

type Samples = Arc<[u8; VOXEL_CHUNK_SAMPLES]>;

/// Face neighbour offsets, in `greedy_mesh_chunk`'s order.
const FACES: [[i64; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

fn neighbour(key: VoxelChunkKey, face: usize) -> VoxelChunkKey {
    let [dx, dy, dz] = FACES[face];
    VoxelChunkKey::new(key.x + dx, key.y + dy, key.z + dz, key.lod)
}

/// Whether the solid samples on a chunk's `face` border differ between two
/// versions of it: the neighbour across that face only sees which of them
/// are solid.
fn border_changed(old: Option<&Samples>, new: Option<&Samples>, face: usize) -> bool {
    let axis = face / 2;
    let layer = if face % 2 == 0 { 7 } else { 0 };
    let solid = |samples: Option<&Samples>, u: usize, v: usize| {
        samples.is_some_and(|samples| {
            let mut p = [0; 3];
            p[axis] = layer;
            p[(axis + 1) % 3] = u;
            p[(axis + 2) % 3] = v;
            samples[p[2] * 64 + p[1] * 8 + p[0]] != 0
        })
    };
    (0..8).any(|u| (0..8).any(|v| solid(old, u, v) != solid(new, u, v)))
}

struct ChunkSlot {
    /// The store's payload allocation: its identity says whether the
    /// chunk changed.
    payload: Arc<[u8]>,
    samples: Samples,
    /// Bumped whenever the chunk must be re-meshed; a mesh is current when
    /// it was made for this revision.
    revision: u64,
    mesh: Option<(u64, Arc<VoxelChunkMesh>)>,
}

impl ChunkSlot {
    fn current(&self) -> bool {
        self.mesh
            .as_ref()
            .is_some_and(|(revision, _)| *revision == self.revision)
    }
}

/// What a voxel object's rows hold.
#[derive(Clone, PartialEq)]
struct Uploaded {
    geometry: u128,
    material_ids: Vec<u32>,
}

struct VoxelObject {
    store: VoxelPayloadStore,
    /// Store revision the chunks were last read at.
    seen_revision: Option<u64>,
    /// An uninitialized store draws its initial cube; these stand in for
    /// its chunks (stable allocations, so they are meshed once).
    virtual_cube: Option<(VoxelCubeInit, HashMap<VoxelChunkKey, Arc<[u8]>>)>,
    chunks: HashMap<VoxelChunkKey, ChunkSlot>,
    voxel_size: f64,
    material_ids: Vec<u32>,
    /// A chunk mesh changed since the last assembly.
    geometry_changed: bool,
    geometry: Option<Arc<VoxelMeshGeometry>>,
    uploaded: Option<Uploaded>,
    /// Among the sources of the last published frame.
    drawn: bool,
}

impl VoxelObject {
    fn new(store: VoxelPayloadStore) -> Self {
        Self {
            store,
            seen_revision: None,
            virtual_cube: None,
            chunks: HashMap::new(),
            voxel_size: 0.0,
            material_ids: Vec::new(),
            geometry_changed: true,
            geometry: None,
            uploaded: None,
            drawn: false,
        }
    }

    fn pending(&self) -> bool {
        self.chunks.values().any(|slot| !slot.current())
    }

    /// Drawn and not yet showing its store's current chunks.
    fn needs_frame(&self) -> bool {
        self.drawn
            && (self.pending()
                || self
                    .store
                    .try_read()
                    .is_ok_and(|state| self.seen_revision != Some(state.0)))
    }
}

struct MeshTask {
    object: VoxelEntryId,
    key: VoxelChunkKey,
    revision: u64,
    samples: Samples,
    neighbours: [Option<Samples>; 6],
}

struct MeshResult {
    object: VoxelEntryId,
    key: VoxelChunkKey,
    revision: u64,
    mesh: VoxelChunkMesh,
}

fn mesh_task(task: MeshTask) -> MeshResult {
    let neighbours = std::array::from_fn(|face| task.neighbours[face].as_deref());
    MeshResult {
        object: task.object,
        key: task.key,
        revision: task.revision,
        mesh: greedy_mesh_chunk(&task.samples, neighbours),
    }
}

/// The meshing thread, started with the first large batch.
struct MeshWorker {
    tasks: mpsc::Sender<Vec<MeshTask>>,
    results: mpsc::Receiver<Vec<MeshResult>>,
}

impl MeshWorker {
    fn start() -> Result<Self, String> {
        let (tasks, inbox) = mpsc::channel::<Vec<MeshTask>>();
        let (outbox, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("voxel-mesher".into())
            .spawn(move || {
                while let Ok(batch) = inbox.recv() {
                    let meshed: Vec<_> = batch.into_iter().map(mesh_task).collect();
                    if outbox.send(meshed).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| format!("could not start the voxel mesher: {error}"))?;
        Ok(Self { tasks, results })
    }
}

/// Counters for tests and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoxelMeshStats {
    /// Chunk meshes made (inline or on the worker) and applied.
    pub meshed_chunks: u64,
    /// Worker results dropped because their chunk changed meanwhile.
    pub discarded_results: u64,
    /// Mesh row uploads.
    pub uploads: u64,
}

/// See the module doc.
pub struct MeshVoxelBackend {
    /// Where the rows go: the shared scene's GPU mirror, written under its
    /// read lock. Without one (CPU tests) the backend meshes but uploads
    /// nothing.
    scene: Option<SharedScene>,
    objects: HashMap<VoxelEntryId, VoxelObject>,
    worker: Option<MeshWorker>,
    next_revision: u64,
    stats: VoxelMeshStats,
}

impl MeshVoxelBackend {
    pub fn new(scene: Option<SharedScene>) -> Self {
        Self {
            scene,
            objects: HashMap::new(),
            worker: None,
            next_revision: 1,
            stats: VoxelMeshStats::default(),
        }
    }

    pub fn stats(&self) -> VoxelMeshStats {
        self.stats
    }

    /// The last assembled mesh of an object, if it has one.
    pub fn geometry(&self, id: VoxelEntryId) -> Option<Arc<VoxelMeshGeometry>> {
        self.objects.get(&id)?.geometry.clone()
    }

    /// Geometry ids of an object's current chunk meshes.
    pub fn chunk_ids(&self, id: VoxelEntryId) -> HashMap<VoxelChunkKey, u64> {
        self.objects.get(&id).map_or_else(HashMap::new, |object| {
            object
                .chunks
                .iter()
                .filter_map(|(key, slot)| Some((*key, slot.mesh.as_ref()?.1.id)))
                .collect()
        })
    }

    /// Read `entry`'s chunks when its store changed and mark what must be
    /// re-meshed. Returns those meshing tasks and any chunks it cannot draw
    /// (which are left out).
    fn refresh(&mut self, entry: &VoxelSceneEntry) -> (Vec<MeshTask>, Vec<String>) {
        let object = self
            .objects
            .entry(entry.id)
            .or_insert_with(|| VoxelObject::new(entry.store.clone()));
        if !Arc::ptr_eq(&object.store, &entry.store) {
            // A replaced component value: start over (the rows are
            // overwritten by the next upload).
            *object = VoxelObject::new(entry.store.clone());
        }
        object.drawn = true;
        if object.voxel_size != entry.voxel_size || object.material_ids != entry.material_ids {
            object.voxel_size = entry.voxel_size;
            object.material_ids = entry.material_ids.clone();
            object.geometry_changed = true;
        }
        let state = match entry.store.try_read() {
            Ok(state) => state,
            // A writer holds it: read it next frame.
            Err(std::sync::TryLockError::WouldBlock) => return (Vec::new(), Vec::new()),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return (Vec::new(), vec!["its payload store was poisoned".into()])
            }
        };
        let revision = state.0;
        let uninitialized = revision == 0 && state.1.is_empty();
        let virtual_changed = uninitialized
            && object.virtual_cube.as_ref().map(|(init, _)| *init) != entry.initial_cube;
        if object.seen_revision == Some(revision) && !virtual_changed {
            return (Vec::new(), Vec::new());
        }
        let mut errors = Vec::new();
        let mut current: HashMap<VoxelChunkKey, Arc<[u8]>> = if uninitialized {
            drop(state);
            object.virtual_cube = entry.initial_cube.map(|init| {
                let chunks = initial_cube_chunks(init)
                    .into_iter()
                    .map(|(key, samples)| (key, Arc::from(samples.as_slice())))
                    .collect();
                (init, chunks)
            });
            object
                .virtual_cube
                .as_ref()
                .map(|(_, chunks)| chunks.clone())
                .unwrap_or_default()
        } else {
            object.virtual_cube = None;
            let chunks = state
                .1
                .iter()
                .filter_map(|(key, payload)| {
                    let key = VoxelChunkKey::new(
                        key[0] as i64,
                        key[1] as i64,
                        key[2] as i64,
                        key[3] as u8,
                    );
                    if payload.encoding != VOXEL_CHUNK_ENCODING_RAW {
                        errors.push(format!(
                            "chunk {key:?} has payload format {}, which voxel objects do not draw",
                            payload.encoding
                        ));
                        return None;
                    }
                    // Objects have one level of detail (their domain's max_lod is 0).
                    (key.lod == 0).then(|| (key, Arc::clone(&payload.bytes)))
                })
                .collect();
            drop(state);
            chunks
        };
        object.seen_revision = Some(revision);

        // What changed, and which neighbours see it.
        let mut dirty = HashSet::new();
        let mut decoded = Vec::new();
        let mut invalid = Vec::new();
        for (key, payload) in &current {
            if object
                .chunks
                .get(key)
                .is_some_and(|slot| Arc::ptr_eq(&slot.payload, payload))
            {
                continue;
            }
            let samples: Samples = match VoxelMaterialChunk::decode(payload) {
                Ok(chunk) => Arc::new(*chunk.samples()),
                Err(error) => {
                    errors.push(format!("chunk {key:?} is not a material chunk: {error:?}"));
                    invalid.push(*key);
                    continue;
                }
            };
            let old = object.chunks.get(key).map(|slot| &slot.samples);
            for face in 0..6 {
                if border_changed(old, Some(&samples), face) {
                    dirty.insert(neighbour(*key, face));
                }
            }
            dirty.insert(*key);
            decoded.push((*key, Arc::clone(payload), samples));
        }
        for key in invalid {
            current.remove(&key);
        }
        let removed: Vec<_> = object
            .chunks
            .keys()
            .filter(|key| !current.contains_key(key))
            .copied()
            .collect();
        for key in removed {
            let slot = object.chunks.remove(&key).expect("listed above");
            for face in 0..6 {
                if border_changed(Some(&slot.samples), None, face) {
                    dirty.insert(neighbour(key, face));
                }
            }
            object.geometry_changed = true;
        }
        for (key, payload, samples) in decoded {
            let slot = object.chunks.entry(key).or_insert_with(|| ChunkSlot {
                payload: Arc::clone(&payload),
                samples: Arc::clone(&samples),
                revision: 0,
                mesh: None,
            });
            slot.payload = payload;
            slot.samples = samples;
        }
        let mut tasks = Vec::new();
        for key in dirty {
            let neighbours = std::array::from_fn(|face| {
                object
                    .chunks
                    .get(&neighbour(key, face))
                    .map(|slot| Arc::clone(&slot.samples))
            });
            let Some(slot) = object.chunks.get_mut(&key) else {
                continue;
            };
            slot.revision = self.next_revision;
            self.next_revision += 1;
            tasks.push(MeshTask {
                object: entry.id,
                key,
                revision: slot.revision,
                samples: Arc::clone(&slot.samples),
                neighbours,
            });
        }
        (tasks, errors)
    }

    /// Mesh `tasks` now when there are few, else on the worker.
    fn dispatch(&mut self, tasks: Vec<MeshTask>) -> Result<(), String> {
        if tasks.is_empty() {
            return Ok(());
        }
        if tasks.len() <= INLINE_CHUNKS {
            for task in tasks {
                let result = mesh_task(task);
                self.apply(result);
            }
            return Ok(());
        }
        if self.worker.is_none() {
            self.worker = Some(MeshWorker::start()?);
        }
        let worker = self.worker.as_ref().expect("started above");
        if let Err(mpsc::SendError(tasks)) = worker.tasks.send(tasks) {
            // The worker is gone (it panicked): mesh here rather than never.
            self.worker = None;
            for task in tasks {
                let result = mesh_task(task);
                self.apply(result);
            }
        }
        Ok(())
    }

    /// Apply worker results that arrived.
    fn drain(&mut self) {
        let mut arrived = Vec::new();
        if let Some(worker) = &self.worker {
            while let Ok(batch) = worker.results.try_recv() {
                arrived.extend(batch);
            }
        }
        for result in arrived {
            self.apply(result);
        }
    }

    fn apply(&mut self, result: MeshResult) {
        let Some(object) = self.objects.get_mut(&result.object) else {
            self.stats.discarded_results += 1;
            return;
        };
        match object.chunks.get_mut(&result.key) {
            Some(slot) if slot.revision == result.revision => {
                if slot
                    .mesh
                    .as_ref()
                    .is_none_or(|(_, mesh)| mesh.id != result.mesh.id)
                {
                    object.geometry_changed = true;
                }
                slot.mesh = Some((result.revision, Arc::new(result.mesh)));
                self.stats.meshed_chunks += 1;
            }
            // The chunk changed (or went) after this task was made.
            _ => self.stats.discarded_results += 1,
        }
    }

    /// Assemble every object whose chunks are all current and changed, and
    /// upload what differs from its rows. Objects in `present` are drawn;
    /// others are released.
    fn upload(&mut self, present: &HashSet<VoxelEntryId>) {
        for (id, object) in &mut self.objects {
            if !present.contains(id) || object.pending() {
                continue;
            }
            if object.geometry_changed || object.geometry.is_none() {
                let geometry = VoxelMeshGeometry::assemble(
                    object
                        .chunks
                        .iter()
                        .filter_map(|(key, slot)| Some((*key, slot.mesh.as_ref()?.1.as_ref()))),
                    object.voxel_size as f32,
                );
                object.geometry = Some(Arc::new(geometry));
                object.geometry_changed = false;
            }
        }
        let Some(scene) = &self.scene else {
            // No GPU: only forget objects that went.
            self.objects.retain(|id, _| present.contains(id));
            return;
        };
        let scene = scene.read();
        let world = &scene.world;
        let ours = |id: VoxelEntryId, store: &VoxelPayloadStore| {
            world
                .get::<VoxelComponent>(Entity::from_bits(id.entity_bits))
                .is_some_and(|component| Arc::ptr_eq(&component.payload_store(), store))
        };
        let mirror = world.gpu_mirror();
        let mut uploads = 0;
        self.objects.retain(|id, object| {
            let row = Entity::from_bits(id.entity_bits).index();
            if !ours(*id, &object.store) {
                // Removed or replaced: SceneDB cleared its rows.
                return false;
            }
            let Some(mirror) = mirror else {
                return present.contains(id);
            };
            if !present.contains(id) {
                if object.uploaded.take().is_some() {
                    clear_voxel_mesh_rows(mirror, row);
                }
                return true;
            }
            let Some(geometry) = &object.geometry else {
                return true;
            };
            let wanted = Uploaded {
                geometry: geometry.id,
                material_ids: object.material_ids.clone(),
            };
            if object.uploaded.as_ref() != Some(&wanted) {
                write_voxel_mesh_rows(mirror, row, geometry, &object.material_ids);
                object.uploaded = Some(wanted);
                uploads += 1;
            }
            true
        });
        self.stats.uploads += uploads;
    }
}

impl VoxelRenderBackend for MeshVoxelBackend {
    fn renderer_id(&self) -> &'static str {
        VOXEL_MESH_RENDERER
    }

    /// Free-standing objects; terrains belong to generator renderers.
    fn supports(&self, source: &VoxelSceneEntry) -> bool {
        source.is_object() && source.generator.is_none()
    }

    /// Meshes are drawn by the scene join, not a pass of this backend.
    fn pass_factory(&self) -> Option<VoxelPassFactory> {
        None
    }

    fn publish_frame(
        &mut self,
        sources: &[&VoxelSceneEntry],
        _view: VoxelView,
    ) -> Result<(), String> {
        let mut errors = Vec::new();
        let mut tasks = Vec::new();
        let mut present = HashSet::new();
        for entry in sources {
            if !entry.is_object() || entry.chunk_edge_voxels != 8 {
                errors.push(format!(
                    "voxel source {:?}: the voxel mesh renderer draws free-standing voxel objects of 8³ chunks",
                    entry.id
                ));
                continue;
            }
            present.insert(entry.id);
            let (new, problems) = self.refresh(entry);
            tasks.extend(new);
            // What it can draw is drawn; the rest is reported.
            errors.extend(
                problems
                    .into_iter()
                    .map(|problem| format!("voxel object {:?}: {problem}", entry.id)),
            );
        }
        for (id, object) in &mut self.objects {
            object.drawn = present.contains(id);
        }
        // Results made before this frame's changes are checked against
        // them: a chunk edited again is discarded, not drawn stale.
        self.drain();
        if let Err(error) = self.dispatch(tasks) {
            errors.push(error);
        }
        self.upload(&present);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    /// While chunks are meshing, or a store changed since it was read.
    fn needs_frame(&self, _renderer: &helio::Renderer) -> bool {
        self.wants_frame()
    }

    fn diagnostics(&self, _renderer: &helio::Renderer) -> Option<String> {
        (!self.objects.is_empty()).then(|| {
            format!(
                "voxel-mesh objects={} pending={} meshed={} discarded={} uploads={}",
                self.objects.len(),
                self.objects.values().filter(|object| object.pending()).count(),
                self.stats.meshed_chunks,
                self.stats.discarded_results,
                self.stats.uploads,
            )
        })
    }
}

impl MeshVoxelBackend {
    /// [`VoxelRenderBackend::needs_frame`] without a renderer.
    pub fn wants_frame(&self) -> bool {
        self.objects.values().any(VoxelObject::needs_frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::voxel_frame::project_voxel_entries;
    use crate::scene::Transform;
    use helio_voxel_data::{VoxelSampleEdit, VoxelSourceId, VoxelSourceWriter, VoxelTerrainId};
    use pulsar_scenedb::World;

    fn view() -> VoxelView {
        VoxelView {
            position: [0.0, 0.0, 40.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            forward: [0.0, 0.0, -1.0],
            tan_half_fov_y: 0.41421357,
            aspect: 1.0,
            far: 1_000.0,
            size: [256, 256],
            sun: None,
        }
    }

    /// One object with `component` on an owner placed by `transform`.
    fn object(component: VoxelComponent, transform: Transform) -> (World, VoxelSceneEntry) {
        let mut world = World::new();
        let owner = world.spawn();
        world.insert(owner, transform);
        pulsar_world_registry::attach_value(&mut world, owner, component).unwrap();
        let (entries, errors) = project_voxel_entries(&world);
        assert!(errors.is_empty(), "{errors:?}");
        let entry = entries.into_iter().next().unwrap();
        (world, entry)
    }

    fn set(entry: &VoxelSceneEntry, xyz: [i64; 3], material_slot: u8) {
        VoxelSourceWriter::new(VoxelTerrainId(1), VoxelSourceId(1), entry.store.clone())
            .publish_sample_edits(
                &[VoxelSampleEdit {
                    xyz,
                    lod: 0,
                    material_slot,
                }],
                entry.domain,
                &entry.material_ids,
            )
            .unwrap();
    }

    /// Publishes until every chunk is meshed (the worker is asynchronous).
    fn settle(backend: &mut MeshVoxelBackend, entry: &VoxelSceneEntry) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            backend.publish_frame(&[entry], view()).unwrap();
            if !backend.wants_frame() {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "meshing did not finish");
            std::thread::yield_now();
        }
    }

    #[test]
    fn rotated_and_scaled_objects_select_the_mesh_renderer() {
        let transform = Transform {
            position: [1.0, 2.0, 3.0],
            rotation: [10.0, 45.0, 0.0],
            scale: [1.0, 2.0, 0.5],
        };
        let (_world, entry) = object(VoxelComponent::default(), transform);
        assert!(entry.is_object() && entry.renderer_id.is_empty());
        assert_eq!((entry.origin, entry.voxel_size), ([1.0, 2.0, 3.0], 1.0));
        let mut registry = super::super::voxel_backend::VoxelBackendRegistry::new();
        registry
            .register(Box::new(super::super::voxel_backend::PlanetVoxelBackend::new()))
            .unwrap();
        registry.register(Box::new(MeshVoxelBackend::new(None))).unwrap();
        assert!(registry.pass_factories().len() == 1, "the mesh renderer has no pass");
        assert_eq!(registry.publish_frame(&[entry], view()), Vec::<String>::new());
    }

    #[test]
    fn a_default_object_meshes_its_cube_and_edits_remesh_only_touched_chunks() {
        let (_world, entry) = object(VoxelComponent::default(), Transform::default());
        let mut backend = MeshVoxelBackend::new(None);
        settle(&mut backend, &entry);
        // 16³ in eight 8³ chunks: each shows its three outer faces.
        assert_eq!(backend.stats().meshed_chunks, 8);
        let cube = backend.geometry(entry.id).unwrap();
        assert_eq!(cube.quad_count(), 24);
        assert_eq!(cube.bounds_local, [8.0, 8.0, 8.0, 192.0f32.sqrt()]);
        let before = backend.chunk_ids(entry.id);
        assert_eq!(before.len(), 8);

        // An interior sample of chunk (0,0,0): only that chunk re-meshes.
        set(&entry, [3, 3, 3], 0);
        assert!(backend.wants_frame(), "a store edit asks for a frame");
        settle(&mut backend, &entry);
        assert_eq!(backend.stats().meshed_chunks, 9);
        let after = backend.chunk_ids(entry.id);
        let origin = VoxelChunkKey::new(0, 0, 0, 0);
        assert_ne!(after[&origin], before[&origin]);
        for (key, id) in &before {
            if *key != origin {
                assert_eq!(after[key], *id, "{key:?} was re-meshed");
            }
        }
        // The hole has six inward faces.
        assert_eq!(backend.geometry(entry.id).unwrap().quad_count(), 30);

        // A sample on chunk (0,0,0)'s +x border also changes (1,0,0)'s face.
        set(&entry, [7, 3, 3], 0);
        settle(&mut backend, &entry);
        assert_eq!(backend.stats().meshed_chunks, 11);
        let last = backend.chunk_ids(entry.id);
        let east = VoxelChunkKey::new(1, 0, 0, 0);
        assert_ne!(last[&east], after[&east]);
        for (key, id) in &after {
            if *key != origin && *key != east {
                assert_eq!(last[key], *id, "{key:?} was re-meshed");
            }
        }
        assert!(!backend.wants_frame());
    }

    #[test]
    fn an_uninitialized_object_draws_its_initial_cube_until_it_has_chunks() {
        // Deserialized components have no payloads yet.
        let restored: VoxelComponent =
            serde_json::from_value(serde_json::to_value(VoxelComponent::default()).unwrap())
                .unwrap();
        let (_world, entry) = object(restored, Transform::default());
        assert!(entry.store.read().unwrap().1.is_empty());
        let mut backend = MeshVoxelBackend::new(None);
        settle(&mut backend, &entry);
        let virtual_cube = backend.geometry(entry.id).unwrap();
        assert_eq!(virtual_cube.quad_count(), 24);
        assert!(entry.store.read().unwrap().1.is_empty(), "drawing writes nothing");
        // Initializing the store publishes the same cube: same geometry.
        crate::scene::voxel_frame::initialize_empty_cube(&entry).unwrap();
        settle(&mut backend, &entry);
        assert_eq!(backend.geometry(entry.id).unwrap().id, virtual_cube.id);
    }

    #[test]
    fn large_volumes_mesh_off_thread_and_drop_results_of_chunks_edited_meanwhile() {
        let component = VoxelComponent::filled_cube([64; 3], vec![0], 1).unwrap();
        let (_world, entry) = object(component, Transform::default());
        let mut backend = MeshVoxelBackend::new(None);
        // 512 chunks: more than the render thread meshes in a frame.
        backend.publish_frame(&[&entry], view()).unwrap();
        assert!(backend.worker.is_some());
        assert!(backend.geometry(entry.id).is_none(), "nothing until complete");
        // Edited before its result was applied: that result is stale.
        set(&entry, [3, 3, 3], 0);
        settle(&mut backend, &entry);
        let stats = backend.stats();
        assert_eq!(stats.discarded_results, 1, "{stats:?}");
        assert_eq!(stats.meshed_chunks, 512, "{stats:?}");
        // Eight chunks per side face, three faces each, plus the hole.
        let geometry = backend.geometry(entry.id).unwrap();
        assert_eq!(geometry.quad_count(), 6 * 64 + 6);
        let ids = backend.chunk_ids(entry.id);
        let origin = VoxelChunkKey::new(0, 0, 0, 0);
        let snapshot = entry.store.read().unwrap().1[&[0, 0, 0, 0]].bytes.clone();
        let samples = *VoxelMaterialChunk::decode(&snapshot).unwrap().samples();
        // The drawn chunk is the edited one.
        let neighbours = [Some(&samples), None, Some(&samples), None, Some(&samples), None];
        assert_eq!(ids[&origin], greedy_mesh_chunk(&samples, neighbours).id);
    }

    #[test]
    fn objects_that_go_are_forgotten_and_empty_objects_have_no_geometry() {
        let component = VoxelComponent::filled_cube([1, 1, 1], vec![0], 1).unwrap();
        let (_world, entry) = object(component, Transform::default());
        let mut backend = MeshVoxelBackend::new(None);
        settle(&mut backend, &entry);
        assert_eq!(backend.geometry(entry.id).unwrap().quad_count(), 6);
        set(&entry, [0, 0, 0], 0);
        settle(&mut backend, &entry);
        assert_eq!(backend.geometry(entry.id).unwrap().quad_count(), 0);
        backend.publish_frame(&[], view()).unwrap();
        assert!(backend.geometry(entry.id).is_none());
        assert!(!backend.wants_frame());
    }
}
