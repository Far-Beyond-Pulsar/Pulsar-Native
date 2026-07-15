//! SceneDB GPU layer (M2a, design Rev 3): persistent scene SSBOs, CPU→GPU
//! delta-sync, and pin-by-serial retirement. Feature-gated (`gpu`); the core
//! crate stays graphics-free (CONTRACTS C0).
//!
//! Mirrored columns must be written via `GpuStore::write_transform` and
//! compacted via `GpuStore::compact`; raw column access bypasses dirty
//! tracking — hard enforcement arrives with the M2b phase machine.

mod buffer;
mod context;
mod generation;
mod region;
mod store;
mod tracker;

pub use buffer::{SceneBuffer, SyncStats};
pub use context::EngineGpuContext;
pub use generation::GenerationBuffer;
pub use region::{RegionPool, RegionError};
pub use store::{GpuStore, GpuStoreConfig};
pub use tracker::SubmissionTracker;

/// Reinterpret a Pod slice as bytes for `queue.write_buffer`.
pub(crate) fn as_bytes<T: crate::page::Pod>(s: &[T]) -> &[u8] {
    // SAFETY: T: Pod guarantees no padding-UB and no invalid bit patterns.
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}
