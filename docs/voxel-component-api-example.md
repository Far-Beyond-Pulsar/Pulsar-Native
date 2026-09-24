# Voxel component API example

This shows the intended ownership boundary for externally generated chunks.
`VoxelTerrainComponent` remains in SceneDB; the data API inbox is only a
bounded transient handoff. Neither API saves data or chooses persistence
settings. A user script/tool owns any export or reload workflow.

```rust,ignore
use helio_voxel_data::{
    BoundedVoxelInbox, VoxelBatchRevision, VoxelChunkBatch, VoxelChunkKey,
    VoxelChunkOp, VoxelChunkPayload, VoxelChunkUpdate, VoxelDomain,
    VoxelInboxDrainBudget, VoxelInboxLimits, VoxelSourceWriter,
    VOXEL_CHUNK_ENCODING_RAW, VOXEL_CHUNK_SCHEMA_VERSION,
};

let terrain = VoxelTerrainComponent::default();
// Insert `terrain` into the user's SceneDB entity, then retain its payload
// handle as a capability associated with that entity.
let store = terrain.payload_store();
let writer = VoxelSourceWriter::new(terrain_id, generator_id, store);

let inbox = BoundedVoxelInbox::new(VoxelInboxLimits::default())?;
let bytes: Arc<[u8]> = generated_chunk.into();
let ops = [VoxelChunkOp::Upsert(VoxelChunkUpdate {
    key: VoxelChunkKey::new(-4, 0, 17, 0),
    payload: VoxelChunkPayload {
        encoding: VOXEL_CHUNK_ENCODING_RAW,
        schema_version: VOXEL_CHUNK_SCHEMA_VERSION,
        bytes: &bytes,
    },
})];
let batch = VoxelChunkBatch {
    terrain: terrain_id,
    source: generator_id,
    revision: VoxelBatchRevision { expected: 0, publish: 1 },
    domain: VoxelDomain::Unbounded { max_lod: 8 },
    ops: &ops,
};

// Producers use try_submit off the frame thread. It clones Arc handles only;
// Full/Busy/Invalid are back-pressure outcomes for the producer to handle.
inbox.try_submit(&batch, &[Arc::clone(&bytes)])?;

// A worker/service drains whole batches within its own operation/byte budget.
// It applies them to SceneDB component data, never to a renderer-owned store.
let pending = inbox.try_drain(VoxelInboxDrainBudget {
    max_ops: 2_048,
    max_payload_bytes: 8 * 1024 * 1024,
})?;
for batch in pending.batches {
    let receipt = batch.publish_into(&writer)?;
    observe_revision(receipt.revision);
}

// Export is an explicit caller operation. The snapshot pins immutable Arc
// payloads while the script writes them to whichever storage it owns.
let snapshot = writer.snapshot()?;
for (key, payload) in snapshot.iter() {
    user_script_storage.write_chunk(key, payload)?;
}
```

To import, the user script reads its own records, builds ordinary revisioned
`VoxelChunkBatch` values, and submits/applies them through the same path. A
failed `publish_into` returns the drained batch to the caller (it is not
silently discarded); the caller may retry, diagnose a stale revision, or
cancel it. Snapshot handles are immutable and cheap to clone, but keeping them
alive pins replaced payload allocations, so long-running exporters should
release snapshots promptly or stream bounded slices.

The current implementation is a correctness/API foundation, not the final
high-throughput storage engine: canonical payloads use a per-component
`RwLock<HashMap<...>>`, and applying each batch is synchronous. Run draining
and publication on a worker/service thread; the render thread must never drain
or apply user batches. Sharded component storage, streaming snapshots, and
measured contention reductions remain performance follow-up work. No 10 ms
radius-128 qualification is implied.
