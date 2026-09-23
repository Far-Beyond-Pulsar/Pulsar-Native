//! Caller-facing SceneDB voxel source capability and worker ownership.

use std::sync::Arc;

use helio_component::{VoxelComponent, VoxelTerrainComponent};
use helio_pass_voxel_mesh::{
    VoxelChunkBatch, VoxelDomain, VoxelEditClose, VoxelEditJob, VoxelEditTicket, VoxelEditWorker,
    VoxelEditWorkerStatus, VoxelInboxBatch, VoxelInboxClose, VoxelInboxLimits,
    VoxelPublicationOutcome, VoxelPublicationStatus, VoxelPublicationTicket,
    VoxelPublicationWorker, VoxelSampleEdit, VoxelSceneEntry, VoxelSourceId, VoxelSourceWriter,
    VoxelTerrainId,
};
use pulsar_scenedb::{Entity, World};

use super::{
    voxel_frame::{object_entry, terrain_entry},
    SharedScene,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoxelSourceKind {
    Object,
    Terrain,
}

/// A session is bound to one entity generation and one component store. New
/// submissions fail after removal/replacement/configuration change. Accepted
/// work may safely complete into the captured orphan store; it cannot mutate
/// a replacement row, and every accepted request keeps a result ticket.
pub struct VoxelSourceSession {
    scene: SharedScene,
    entity: Entity,
    kind: VoxelSourceKind,
    source: VoxelSourceId,
    entry: VoxelSceneEntry,
    material_ids: Arc<[u32]>,
    writer: VoxelSourceWriter,
    publication: VoxelPublicationWorker,
    edits: VoxelEditWorker,
}

impl VoxelSourceSession {
    /// Open on a management/script thread, never in the renderer callback.
    /// The two CPU workers share the canonical store's revision check.
    pub fn open(
        scene: SharedScene,
        entity: Entity,
        kind: VoxelSourceKind,
        source: VoxelSourceId,
        limits: VoxelInboxLimits,
    ) -> Result<Self, String> {
        let guard = scene.try_read().ok_or("SceneDB is busy")?;
        let entry = resolve(&guard.world, entity, kind)?;
        drop(guard);
        // A newly created or deserialized cube must exist in the canonical
        // store before edits can publish their first revision.
        helio_pass_voxel_mesh::initialize_empty_cube(&entry)?;
        let writer = VoxelSourceWriter::new(
            VoxelTerrainId(u128::from(entity.bits())),
            source,
            entry.store.clone(),
        );
        let publication = VoxelPublicationWorker::start(writer.clone(), limits)
            .map_err(|error| format!("start voxel publication worker: {error:?}"))?;
        let edits = VoxelEditWorker::start(writer.clone())
            .map_err(|error| format!("start voxel edit worker: {error}"))?;
        let material_ids = Arc::from(entry.material_ids.as_slice());
        Ok(Self {
            scene,
            entity,
            kind,
            source,
            entry,
            material_ids,
            writer,
            publication,
            edits,
        })
    }

    pub fn terrain_id(&self) -> VoxelTerrainId {
        VoxelTerrainId(u128::from(self.entity.bits()))
    }

    /// Identity/lifetime check takes only a try-read on the scene. Work
    /// already accepted into a worker remains observable through its ticket.
    pub fn is_attached(&self) -> Result<bool, String> {
        let scene = self.scene.try_read().ok_or("SceneDB is busy")?;
        let Ok(current) = resolve(&scene.world, self.entity, self.kind) else {
            return Ok(false);
        };
        Ok(Arc::ptr_eq(&current.store, &self.entry.store)
            && current.domain == self.entry.domain
            && current.source_revision == self.entry.source_revision
            && current.origin == self.entry.origin
            && current.voxel_size == self.entry.voxel_size
            && current.material_ids == self.entry.material_ids
            && current.smooth_surface == self.entry.smooth_surface
            && current.initial_cube == self.entry.initial_cube)
    }

    fn require_attached(&self) -> Result<(), String> {
        if self.is_attached()? {
            Ok(())
        } else {
            Err("voxel component was removed or replaced; open a new source session".into())
        }
    }

    /// Nonblocking chunk-batch admission. The SceneDB component write lock is
    /// taken only by the CPU publication worker. The returned ticket reports
    /// success, failure with retained batch, or explicit discard.
    pub fn try_submit_chunks(
        &self,
        batch: &VoxelChunkBatch<'_>,
        payloads: &[Arc<[u8]>],
    ) -> Result<VoxelPublicationTicket, String> {
        self.require_attached()?;
        if batch.domain != self.entry.domain {
            return Err("chunk batch domain differs from its SceneDB component".into());
        }
        self.publication
            .try_submit(batch, payloads)
            .map_err(|error| format!("voxel chunk admission: {error:?}"))
    }

    /// Requeue retained work after the caller reviews its revision and opens
    /// a fresh session if the previous publisher stopped on an error.
    pub fn try_retry_chunks(
        &self,
        batch: &VoxelInboxBatch,
    ) -> Result<VoxelPublicationTicket, String> {
        self.require_attached()?;
        if batch.terrain != self.terrain_id()
            || batch.source != self.source
            || batch.domain != self.entry.domain
        {
            return Err("retained batch belongs to another voxel source configuration".into());
        }
        self.publication
            .try_retry(batch)
            .map_err(|error| format!("voxel retry admission: {error:?}"))
    }

    /// Nonblocking sample-edit admission. Edits are grouped into touched
    /// chunks and published atomically on a CPU worker. The supplied Arc can
    /// hold up to 65,536 edits per request without a frame-thread copy.
    pub fn try_submit_edits(
        &self,
        edits: Arc<[VoxelSampleEdit]>,
    ) -> Result<VoxelEditTicket, String> {
        self.require_attached()?;
        self.edits
            .try_submit(VoxelEditJob {
                edits,
                material_ids: self.material_ids.clone(),
                domain: self.entry.domain,
            })
            .map_err(|error| format!("voxel edit admission: {error:?}"))
    }

    pub fn publication_status(&self) -> Result<VoxelPublicationStatus, String> {
        self.publication
            .try_status()
            .map_err(|error| format!("voxel publication status: {error:?}"))
    }
    pub fn edit_status(&self) -> VoxelEditWorkerStatus {
        self.edits.status()
    }
    /// Caller-owned in-memory export; run off the frame thread for large maps.
    pub fn snapshot(
        &self,
    ) -> Result<helio_pass_voxel_mesh::VoxelTerrainSnapshot, helio_pass_voxel_mesh::VoxelUpdateError>
    {
        self.writer.snapshot()
    }

    /// Finish on a management thread. Drain processes queued work, while
    /// Discard marks queued tickets discarded; an active write can finish.
    pub fn finish(
        self,
        mode: VoxelInboxClose,
    ) -> (VoxelPublicationOutcome, VoxelEditWorkerStatus, bool) {
        let publication = self.publication.finish(mode);
        let (edits, panicked) = self.edits.finish(match mode {
            VoxelInboxClose::Drain => VoxelEditClose::Drain,
            VoxelInboxClose::Discard => VoxelEditClose::Discard,
        });
        (publication, edits, panicked)
    }
}

fn resolve(
    world: &World,
    entity: Entity,
    kind: VoxelSourceKind,
) -> Result<VoxelSceneEntry, String> {
    match kind {
        VoxelSourceKind::Object => {
            let component = world
                .get::<VoxelComponent>(entity)
                .ok_or("voxel object component is absent")?;
            if !component.enabled || !component.editable {
                return Err("voxel object is disabled or not editable".into());
            }
            object_entry(world, entity, &component).map_err(str::to_string)
        }
        VoxelSourceKind::Terrain => {
            let component = world
                .get::<VoxelTerrainComponent>(entity)
                .ok_or("voxel terrain component is absent")?;
            if !component.enabled || !component.editable {
                return Err("voxel terrain is disabled or not editable".into());
            }
            terrain_entry(world, entity, &component).map_err(str::to_string)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_submits_deformation_and_rejects_after_component_removal() {
        let scene: SharedScene = Arc::new(parking_lot::RwLock::new(pulsar_scenedb::SceneDb::new()));
        let entity = {
            let mut scene = scene.write();
            let entity = scene.world.spawn();
            // Deserialization intentionally omits runtime payloads.
            let serialized = serde_json::to_value(VoxelComponent::default()).unwrap();
            let restored: VoxelComponent = serde_json::from_value(serialized).unwrap();
            scene.world.insert(entity, restored);
            entity
        };
        let session = VoxelSourceSession::open(
            scene.clone(),
            entity,
            VoxelSourceKind::Object,
            VoxelSourceId(7),
            VoxelInboxLimits::default(),
        )
        .unwrap();
        let ticket = session
            .try_submit_edits(Arc::from([VoxelSampleEdit {
                xyz: [0, 0, 0],
                lod: 0,
                material_slot: 0,
            }]))
            .unwrap();
        assert!(matches!(
            ticket.wait(),
            helio_pass_voxel_mesh::VoxelEditTicketState::Published(_)
        ));
        let snapshot = session.snapshot().unwrap();
        let chunk = helio_pass_voxel_mesh::VoxelMaterialChunk::decode(
            snapshot
                .get(helio_pass_voxel_mesh::VoxelChunkKey::new(0, 0, 0, 0))
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
        let (publication, edits, panicked) = session.finish(VoxelInboxClose::Drain);
        assert!(!publication.worker_panicked && !panicked);
        assert_eq!(edits.published_jobs, 1);
    }

    #[test]
    fn accepted_batch_completes_in_old_store_after_row_replacement() {
        use helio_pass_voxel_mesh::{
            VoxelBatchRevision, VoxelChunkKey, VoxelChunkOp, VoxelChunkPayload, VoxelChunkUpdate,
            VOXEL_CHUNK_ENCODING_RAW, VOXEL_CHUNK_SCHEMA_VERSION,
        };
        let scene: SharedScene = Arc::new(parking_lot::RwLock::new(pulsar_scenedb::SceneDb::new()));
        let entity = {
            let mut scene = scene.write();
            let entity = scene.world.spawn();
            scene.world.insert(entity, VoxelComponent::default());
            entity
        };
        let session = VoxelSourceSession::open(
            scene.clone(),
            entity,
            VoxelSourceKind::Object,
            VoxelSourceId(4),
            VoxelInboxLimits::default(),
        )
        .unwrap();
        let old_store = session.entry.store.clone();
        assert_eq!(old_store.read().unwrap().0, 0);
        let held = old_store.write().unwrap();
        let payload: Arc<[u8]> = Arc::from([0u8]);
        let ops = [VoxelChunkOp::Upsert(VoxelChunkUpdate {
            key: VoxelChunkKey::new(0, 0, 0, 0),
            payload: VoxelChunkPayload {
                encoding: VOXEL_CHUNK_ENCODING_RAW,
                schema_version: VOXEL_CHUNK_SCHEMA_VERSION,
                bytes: &payload,
            },
        })];
        let ticket = session
            .try_submit_chunks(
                &VoxelChunkBatch {
                    terrain: session.terrain_id(),
                    source: VoxelSourceId(4),
                    revision: VoxelBatchRevision {
                        expected: 0,
                        publish: 1,
                    },
                    domain: session.entry.domain,
                    ops: &ops,
                },
                &[payload.clone()],
            )
            .unwrap();
        {
            let mut scene = scene.write();
            scene.world.remove::<VoxelComponent>(entity);
            scene.world.insert(entity, VoxelComponent::default());
        }
        let new_store = scene
            .read()
            .world
            .get::<VoxelComponent>(entity)
            .unwrap()
            .payload_store();
        assert!(!Arc::ptr_eq(&old_store, &new_store));
        assert!(!session.is_attached().unwrap());
        drop(held);
        let state = ticket.wait();
        assert!(
            matches!(
                state,
                helio_pass_voxel_mesh::VoxelPublicationTicketState::Published(_)
            ),
            "{state:?}"
        );
        assert_eq!(old_store.read().unwrap().0, 1);
        assert_eq!(new_store.read().unwrap().0, 0);
        session.finish(VoxelInboxClose::Drain);
    }

    #[test]
    fn failed_batch_can_be_rebased_and_retried_from_new_session() {
        use helio_pass_voxel_mesh::{
            VoxelBatchRevision, VoxelChunkKey, VoxelChunkOp, VoxelChunkPayload, VoxelChunkUpdate,
            VoxelPublicationTicketState, VOXEL_CHUNK_ENCODING_RAW, VOXEL_CHUNK_SCHEMA_VERSION,
        };
        let scene: SharedScene = Arc::new(parking_lot::RwLock::new(pulsar_scenedb::SceneDb::new()));
        let entity = {
            let mut scene = scene.write();
            let entity = scene.world.spawn();
            scene.world.insert(entity, VoxelComponent::default());
            entity
        };
        let open = || {
            VoxelSourceSession::open(
                scene.clone(),
                entity,
                VoxelSourceKind::Object,
                VoxelSourceId(4),
                VoxelInboxLimits::default(),
            )
            .unwrap()
        };
        let session = open();
        let payload: Arc<[u8]> = Arc::from([0u8]);
        let ops = [VoxelChunkOp::Upsert(VoxelChunkUpdate {
            key: VoxelChunkKey::new(0, 0, 0, 0),
            payload: VoxelChunkPayload {
                encoding: VOXEL_CHUNK_ENCODING_RAW,
                schema_version: VOXEL_CHUNK_SCHEMA_VERSION,
                bytes: &payload,
            },
        })];
        let ticket = session
            .try_submit_chunks(
                &VoxelChunkBatch {
                    terrain: session.terrain_id(),
                    source: VoxelSourceId(4),
                    revision: VoxelBatchRevision {
                        expected: 1,
                        publish: 2,
                    },
                    domain: session.entry.domain,
                    ops: &ops,
                },
                &[payload.clone()],
            )
            .unwrap();
        assert!(matches!(
            ticket.wait(),
            VoxelPublicationTicketState::Failed(_)
        ));
        let (outcome, _, panicked) = session.finish(VoxelInboxClose::Drain);
        assert!(!panicked);
        let mut retained = outcome.failure.unwrap().batch;
        retained.revision = VoxelBatchRevision {
            expected: 0,
            publish: 1,
        };
        let replacement = open();
        let retry = replacement.try_retry_chunks(&retained).unwrap();
        assert!(matches!(
            retry.wait(),
            VoxelPublicationTicketState::Published(_)
        ));
        assert_eq!(replacement.publication_status().unwrap().retried_batches, 1);
        assert_eq!(replacement.snapshot().unwrap().revision(), 1);
        replacement.finish(VoxelInboxClose::Drain);
    }
}
