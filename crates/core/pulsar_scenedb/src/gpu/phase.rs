//! Compile-time frame-phase machine (design Rev 2 §6, C3): zero-size witness
//! types that make the frame's phase a type, not a runtime value. Holding a
//! phase value IS the permission to call the APIs gated on it.
//!
//! Honest coverage map — what the types close vs. what they do not:
//!
//! - CLOSED by the types: witness forgery (all witnesses are ZSTs with
//!   private fields, `SimulateWitness` is sealed — no external construction
//!   or impl), boundary-stage reordering, skipping, and double-running
//!   (each transition consumes `self`; `retire_all`/`compact_all`/`sync_all`
//!   are `pub(crate)`, reachable only through this chain).
//! - STILL on the runtime `Phase` debug-asserts (debug builds only): a
//!   STALE or duplicated Simulate witness. `FrameDriver::begin` does not
//!   lifetime-tie the witness to the frame, and `write_transform`/
//!   `free_deferred` take `&impl SimulateWitness` without consuming it — a
//!   caller who hoards a `SimulateA` across a boundary can mutate during
//!   what is dynamically the wrong window; only the enum catches that, and
//!   only in debug.
//! - Enforced by NOTHING: boundary liveness — no type obliges a caller to
//!   ever end the frame and run the boundary at all.
//!
//! A lifetime-carrying witness (`SimulateA<'frame>` borrowed from the
//! driver/store) is the candidate hardening for the stale-witness hole —
//! M2b-β/M4 scope.
//!
//! One frame: `FrameDriver::begin` → `SimulateA` → `SimulateB` → `HarvestPhase`
//! → `BoundaryPhase` → (retire → compact → sync) → back to the next
//! `FrameDriver::begin`. `SimulateA`/`SimulateB` are the two mutation
//! sub-phases (C3: A = gameplay, B = physics writeback — the distinction
//! gains teeth once physics lands in M4; both are accepted anywhere a
//! `SimulateWitness` is required today).

use super::{CellSlot, SceneGpuStore, SyncStats};

/// Owns one frame's progression through the phase machine. `begin` is the
/// only entry point into a fresh Simulate phase; everything downstream is a
/// chain of consuming transitions on the witness values themselves.
pub struct FrameDriver(());

impl FrameDriver {
    pub fn new() -> Self {
        Self(())
    }

    /// Open a new frame: gameplay mutation is now permitted.
    pub fn begin(&mut self) -> SimulateA {
        SimulateA(())
    }
}

impl Default for FrameDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Gameplay simulate sub-phase (C3 A). Mutation-permitting.
pub struct SimulateA(());

impl SimulateA {
    /// Gameplay simulation is done for this frame; hand off to physics
    /// writeback.
    pub fn end(self) -> SimulateB {
        SimulateB(())
    }
}

/// Physics-writeback simulate sub-phase (C3 B). Mutation-permitting.
pub struct SimulateB(());

impl SimulateB {
    /// Physics writeback is done; no further mutation this frame.
    pub fn end(self) -> HarvestPhase {
        HarvestPhase(())
    }
}

/// Harvest phase: read-only. Holding this witness grants no mutation
/// capability — `write_transform`/`free_deferred` require a
/// [`SimulateWitness`], and `HarvestPhase` deliberately does not implement
/// it (see the compile_fail doc-test below).
pub struct HarvestPhase(());

impl HarvestPhase {
    /// Harvest is done; open the frame boundary.
    pub fn end(self) -> BoundaryPhase {
        BoundaryPhase(())
    }
}

/// Frame-boundary phase: retire → (transitions: β slots in here — cell
/// promotion/eviction reacts to this frame's occupancy before compaction
/// runs) → compact → sync. `run` is the all-in-one composition; `retire`/
/// `compact`/`sync` are the same three stages exposed as individually
/// consuming transitions, for callers (e.g. tests) that need to observe
/// store/cell state BETWEEN stages.
///
/// Boundary stages cannot be reordered — `retire_all` is `pub(crate)`:
/// ```compile_fail
/// use pulsar_scenedb::gpu::*;
/// fn f(store: &mut SceneGpuStore, cells: &mut [CellSlot<'_>]) {
///     store.retire_all(cells); // private outside the crate
/// }
/// ```
pub struct BoundaryPhase(());

impl BoundaryPhase {
    /// Run the full boundary in one call: retire → compact → sync.
    pub fn run(self, store: &mut SceneGpuStore, cells: &mut [CellSlot<'_>]) -> SyncStats {
        let (retired, _drained) = self.retire(store, cells);
        retired.compact(store, cells).sync(store, cells)
    }

    /// §5 flow step 3: drain every cell's deferred-retire queue against the
    /// completed-serial watermark. Returns the total number of slots retired
    /// across every cell — the gate must not lose direct observability of
    /// what it gates.
    pub fn retire(self, store: &mut SceneGpuStore, cells: &mut [CellSlot<'_>]) -> (RetiredPhase, u32) {
        let drained = store.retire_all(cells);
        (RetiredPhase(()), drained)
    }
}

/// After `retire_all`, before `compact_all`. Exists solely so integration
/// tests outside this crate — which cannot call the now-`pub(crate)`
/// `retire_all`/`compact_all`/`sync_all` directly — can still observe store
/// and cell state between boundary stages (test6's between-stage asserts).
pub struct RetiredPhase(());

impl RetiredPhase {
    pub fn compact(self, store: &mut SceneGpuStore, cells: &mut [CellSlot<'_>]) -> CompactedPhase {
        store.compact_all(cells);
        CompactedPhase(())
    }
}

/// After `compact_all`, before `sync_all`.
pub struct CompactedPhase(());

impl CompactedPhase {
    pub fn sync(self, store: &mut SceneGpuStore, cells: &mut [CellSlot<'_>]) -> SyncStats {
        store.sync_all(cells)
    }
}

/// Sealed: mutation APIs (`write_transform`, `free_deferred`) accept either
/// simulate sub-phase (C3 A = gameplay, B = physics writeback — the
/// distinction gains teeth when physics lands, M4) and nothing else. Sealed
/// so downstream crates cannot manufacture a witness for a phase that was
/// never granted mutation permission.
///
/// Mutation requires a Simulate witness — a Harvest witness does not compile:
/// ```compile_fail
/// use pulsar_scenedb::gpu::*;
/// fn f(store: &SceneGpuStore, id: CellId, cell: &mut pulsar_scenedb::CellStorage,
///      h: pulsar_scenedb::Handle, harvest: &HarvestPhase) {
///     store.write_transform(id, cell, h, &[0.0; 16], harvest); // not a SimulateWitness
/// }
/// ```
///
/// The positive counterpart — the same gated call COMPILES with a valid
/// Simulate witness:
/// ```
/// use pulsar_scenedb::gpu::*;
/// fn f(store: &SceneGpuStore, id: CellId, cell: &mut pulsar_scenedb::CellStorage,
///      h: pulsar_scenedb::Handle, sim: &SimulateA) {
///     store.write_transform(id, cell, h, &[0.0; 16], sim); // SimulateA is a SimulateWitness
/// }
/// ```
pub trait SimulateWitness: private::Sealed {}
impl SimulateWitness for SimulateA {}
impl SimulateWitness for SimulateB {}

mod private {
    pub trait Sealed {}
    impl Sealed for super::SimulateA {}
    impl Sealed for super::SimulateB {}
}
