//! SceneDB GPU layer (M2b-α, design Rev 2): persistent region-partitioned
//! scene SSBOs, CPU→GPU delta-sync, and pin-by-serial retirement across N
//! registered cells. Feature-gated (`gpu`); the core crate stays
//! graphics-free (CONTRACTS C0).
//!
//! Mirrored columns must be written via `SceneGpuStore::write_transform` and
//! compacted via the frame-boundary drivers in [`phase`]; raw column access
//! bypasses dirty tracking. The frame phase itself is enforced at compile
//! time (design Rev 2 §6, C3): mutation requires a [`SimulateWitness`], and
//! the boundary stages (retire → compact → sync) are reachable only through
//! [`FrameDriver`] and [`BoundaryPhase`]'s consuming transitions — see
//! `phase.rs` for the witness chain and its compile_fail doc-tests.

mod assets;
mod buffer;
mod context;
mod dirty;
mod generation;
mod grid;
mod harvest;
mod phase;
mod region;
mod scene_store;
mod tracker;
mod view_upload;

pub use assets::{
    ArenaError, ClusterBuffer, ClusterError, ClusterNode, GeometryArena, MaterialError,
    MaterialRegistry, MaterialRow, MeshError, MeshMetadata, MeshRegistry, MeshletBuffer,
    MeshletEntry, MeshletError, TextureError, TextureStore, MAX_TEXTURE_SLOTS,
};
pub use buffer::{GpuBufferDispatch, SceneBuffer, SyncStats};
pub use context::EngineGpuContext;
pub use dirty::DirtyMask;
pub use generation::GenerationBuffer;
pub use grid::{
    execute_transitions, BudgetError, CellCoord, Domain, GridConfig, StreamingBudget,
    StreamingGrid, Transition, TransitionStats,
};
pub use harvest::{
    revalidate_run, HarvestLease, HarvestPipeline, HarvestStaging, HarvestStats, MeshClass, View,
};
pub use phase::{BoundaryPhase, CompactedPhase, FrameDriver, HarvestPhase, RetiredPhase, SimulateA, SimulateB, SimulateWitness};
pub use region::{RegionPool, RegionError};
pub use scene_store::{
    CellId, CellSlot, GpuColumnDesc, GpuColumnSet, MirrorMode, RegionClassConfig, SceneGpuConfig,
    SceneGpuStore,
};
pub use tracker::SubmissionTracker;
pub use view_upload::ViewTokenBuffers;
// `InstanceInfo` is defined graphics-free in `crate::spatial` (CONTRACTS C0)
// and already re-exported at the crate root; re-exported here too so GPU-
// adjacent consumers (e.g. Helio's `helio-scenedb` seam reflection harness,
// M3-a T10) can reach every C5 struct type through one `gpu::` path.
pub use crate::spatial::InstanceInfo;

/// Reinterpret a Pod slice as bytes for `queue.write_buffer`.
pub(crate) fn as_bytes<T: crate::page::Pod>(s: &[T]) -> &[u8] {
    // SAFETY: T: Pod guarantees no padding-UB and no invalid bit patterns.
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}
