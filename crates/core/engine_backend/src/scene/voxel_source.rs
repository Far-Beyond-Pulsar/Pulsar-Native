//! Sample edits on a voxel object's live chunks (`VoxelComponent`), on a CPU
//! worker. Voxel worlds are edited through their brush journal
//! (`VoxelTerrainComponent` methods).

use std::sync::Arc;

use helio_component::VoxelComponent;
use helio_voxel_data::{
    VoxelEditClose, VoxelEditJob, VoxelEditTicket, VoxelEditWorker, VoxelEditWorkerStatus,
    VoxelSampleEdit, VoxelSourceId, VoxelSourceWriter, VoxelTerrainId,
};
use pulsar_scene_model::attachments;
use pulsar_scenedb::{Entity, World};

use super::{
    voxel_frame::{initialize_empty_cube, object_entry, VoxelObjectChunks, VoxelSceneEntry},
    SharedScene,
};

/// A session is bound to one entity generation and one component store. New
/// submissions fail after removal/replacement/configuration change. Accepted
/// work may safely complete into the captured orphan store; it cannot mutate
/// a replacement row, and every accepted request keeps a result ticket.
pub struct VoxelSourceSession {
    scene: SharedScene,
    entity: Entity,
    origin: [f64; 3],
    voxel_size: f64,
    chunks: VoxelObjectChunks,
    material_ids: Arc<[u32]>,
    edits: VoxelEditWorker,
}

impl VoxelSourceSession {
    /// Open on a management/script thread, never in the renderer callback.
    pub fn open(scene: SharedScene, entity: Entity, source: VoxelSourceId) -> Result<Self, String> {
        let guard = scene.try_read().ok_or("SceneDB is busy")?;
        let (entry, chunks) = resolve(&guard.world, entity)?;
        drop(guard);
        // A newly created or deserialized cube must exist in the canonical
        // store before edits can publish their first revision.
        initialize_empty_cube(entity.bits(), &chunks)?;
        let writer = VoxelSourceWriter::new(
            VoxelTerrainId(u128::from(entity.bits())),
            source,
            chunks.store.clone(),
        );
        let edits = VoxelEditWorker::start(writer)
            .map_err(|error| format!("start voxel edit worker: {error}"))?;
        let material_ids = Arc::from(chunks.material_ids.as_slice());
        Ok(Self {
            scene,
            entity,
            origin: entry.origin,
            voxel_size: entry.voxel_size,
            chunks,
            material_ids,
            edits,
        })
    }

    /// Identity/lifetime check takes only a try-read on the scene. Work
    /// already accepted into the worker remains observable through its ticket.
    pub fn is_attached(&self) -> Result<bool, String> {
        let scene = self.scene.try_read().ok_or("SceneDB is busy")?;
        let Ok((current, chunks)) = resolve(&scene.world, self.entity) else {
            return Ok(false);
        };
        Ok(Arc::ptr_eq(&chunks.store, &self.chunks.store)
            && chunks.domain == self.chunks.domain
            && current.origin == self.origin
            && current.voxel_size == self.voxel_size
            && chunks.material_ids == self.chunks.material_ids
            && chunks.initial_cube == self.chunks.initial_cube)
    }

    /// Nonblocking sample-edit admission. Edits are grouped into touched
    /// chunks and published atomically on a CPU worker. The supplied Arc can
    /// hold up to 65,536 edits per request without a frame-thread copy.
    pub fn try_submit_edits(
        &self,
        edits: Arc<[VoxelSampleEdit]>,
    ) -> Result<VoxelEditTicket, String> {
        if !self.is_attached()? {
            return Err(
                "voxel component was removed or replaced; open a new source session".into(),
            );
        }
        self.edits
            .try_submit(VoxelEditJob {
                edits,
                material_ids: self.material_ids.clone(),
                domain: self.chunks.domain,
            })
            .map_err(|error| format!("voxel edit admission: {error:?}"))
    }

    /// Finish on a management thread. Drain processes queued edits, while
    /// Discard marks queued tickets discarded; an active write can finish.
    pub fn finish(self, mode: VoxelEditClose) -> (VoxelEditWorkerStatus, bool) {
        self.edits.finish(mode)
    }
}

/// The object's projection and its chunks.
fn resolve(world: &World, entity: Entity) -> Result<(VoxelSceneEntry, VoxelObjectChunks), String> {
    let component = world
        .get::<VoxelComponent>(entity)
        .ok_or("voxel object component is absent")?;
    if !component.enabled || !component.editable || !attachments::is_enabled(world, entity) {
        return Err("voxel object is disabled or not editable".into());
    }
    let entry = object_entry(world, entity, &component).map_err(str::to_string)?;
    let chunks = entry
        .chunks
        .clone()
        .expect("a voxel object projects its chunks");
    Ok((entry, chunks))
}

#[cfg(test)]
mod tests {
    use super::super::voxel_frame::spawn_test_instance;
    use super::*;

    #[test]
    fn session_submits_deformation_and_rejects_after_component_removal() {
        let scene: SharedScene = Arc::new(parking_lot::RwLock::new(pulsar_scenedb::SceneDb::new()));
        let entity = {
            let mut scene = scene.write();
            let entity = spawn_test_instance(&mut scene.world, "VoxelComponent");
            // Deserialization intentionally omits runtime payloads.
            let serialized = serde_json::to_value(VoxelComponent::default()).unwrap();
            let restored: VoxelComponent = serde_json::from_value(serialized).unwrap();
            scene.world.insert(entity, restored);
            entity
        };
        let session = VoxelSourceSession::open(scene.clone(), entity, VoxelSourceId(7)).unwrap();
        let ticket = session
            .try_submit_edits(Arc::from([VoxelSampleEdit {
                xyz: [0, 0, 0],
                lod: 0,
                material_slot: 0,
            }]))
            .unwrap();
        assert!(matches!(
            ticket.wait(),
            helio_voxel_data::VoxelEditTicketState::Published(_)
        ));
        let snapshot = VoxelSourceWriter::new(
            VoxelTerrainId(1),
            VoxelSourceId(7),
            session.chunks.store.clone(),
        )
        .snapshot()
        .unwrap();
        let chunk = helio_voxel_data::VoxelMaterialChunk::decode(
            snapshot
                .get_raw_material(helio_voxel_data::VoxelChunkKey::new(0, 0, 0, 0))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(chunk.sample(0, 0, 0), 0);
        assert_eq!(chunk.sample(1, 1, 1), 1);
        scene.write().world.remove::<VoxelComponent>(entity);
        assert!(!session.is_attached().unwrap());
        assert!(session
            .try_submit_edits(Arc::from([VoxelSampleEdit {
                xyz: [1, 0, 0],
                lod: 0,
                material_slot: 1,
            }]))
            .is_err());
        let (edits, panicked) = session.finish(VoxelEditClose::Drain);
        assert!(!panicked);
        assert_eq!(edits.published_jobs, 1);
    }
}
