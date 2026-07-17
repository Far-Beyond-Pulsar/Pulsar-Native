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
//!   are `pub(crate)`, reachable only through this chain). That closure is
//!   an EXTERNAL-CRATE guarantee only: `pub(crate)` still lets any function
//!   inside this crate call `retire_all`/`compact_all`/`sync_all` directly
//!   (bypassing the witness chain and its phase ordering entirely) — the
//!   types close the door on downstream callers, not on a same-crate bug
//!   that reaches around this module.
//! - STILL on the runtime `Phase` debug-asserts (debug builds only): a
//!   STALE or duplicated Simulate witness. `FrameDriver::begin` does not
//!   lifetime-tie the witness to the frame, and `write_transform`/
//!   `free_deferred` take `&impl SimulateWitness` without consuming it — a
//!   caller who hoards a `SimulateA` across a boundary can mutate during
//!   what is dynamically the wrong window; only the enum catches that, and
//!   only in debug.
//! - Enforced by NOTHING: boundary liveness — no type obliges a caller to
//!   ever end the frame and run the boundary at all.
//! - MEMORY ORDERING (§9.2.1): this module OWNS the Release/Acquire
//!   phase-boundary edge that `LivenessMask`/`LivenessSnapshot` (both
//!   `Relaxed`-only) depend on for cross-thread visibility.
//!   `SimulateB::end` emits the `Release` fence that publishes every
//!   simulate-phase write; `HarvestPhase::end` and `BoundaryPhase::retire`
//!   each emit a paired `Acquire` fence before boundary/harvest code reads
//!   that state. Previously undocumented as anyone's job (see prior audit);
//!   now owned here. Callers that read/write `LivenessMask` outside this
//!   phase machine (there are none in-crate today) must provide their own
//!   equivalent barrier. **Precision:** a bare `fence(Release)`/`fence(Acquire)`
//!   pair does not by itself synchronize two threads — the edge exists only
//!   through an atomic handoff (a store after the Release fence that a load
//!   before the Acquire fence actually reads), and that handoff still needs a
//!   real cross-thread witness (spawn/join, a channel, a mutex) linking writer
//!   and reader so the load is guaranteed to run after the store; two threads
//!   each calling their own fence with no atomic and no such witness between
//!   them do not gain any ordering guarantee from the fences alone.
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

use std::sync::atomic::{fence, Ordering};

use super::{CellId, CellSlot, SceneGpuStore, SyncStats};

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
    ///
    /// §9.2.1: this is THE phase-boundary happens-before edge that
    /// `LivenessMask`/`LivenessSnapshot`'s `Relaxed` loads rely on
    /// (`liveness.rs`'s "Memory ordering contract" doc, `snapshot.rs::capture`'s
    /// doc). All simulate-phase writes (both `SimulateA` and `SimulateB`
    /// sub-phases — `write_transform`, `free_deferred`, `LivenessMask::set_live`/
    /// `set_dead`) are published here with a `Release` fence before any
    /// harvest/boundary reader can observe them; paired with the `Acquire`
    /// fences in `HarvestPhase::end` and `BoundaryPhase::retire`. On a
    /// single-threaded caller (today's only caller shape) this fence costs
    /// effectively nothing — it only orders this thread's own prior stores,
    /// which the compiler already cannot reorder past a fence, and emits no
    /// instruction on strongly-ordered hardware (x86/x86-64); it exists for
    /// the multi-threaded simulate-writer / harvest-reader shape the phase
    /// machine is designed to support.
    pub fn end(self) -> HarvestPhase {
        fence(Ordering::Release);
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
    ///
    /// §9.2.1: paired `Acquire` fence for the `Release` published in
    /// `SimulateB::end` — establishes happens-before for every
    /// `LivenessMask`/`LivenessSnapshot` `Relaxed` load a harvest-phase reader
    /// performed (or will perform via data captured here) against this
    /// frame's simulate-phase writes. Single-threaded callers pay ~nothing.
    pub fn end(self) -> BoundaryPhase {
        fence(Ordering::Acquire);
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
    ///
    /// §9.2.1: a second `Acquire` fence at boundary entry, belt-and-suspenders
    /// with `HarvestPhase::end`'s — this is the first line of code the
    /// frame-boundary machinery runs, so it is the natural place to
    /// (re-)establish happens-before against every simulate-phase write for
    /// any reader that reaches this point without having gone through
    /// `HarvestPhase::end` directly (e.g. the all-in-one `BoundaryPhase::run`
    /// path). Single-threaded callers pay ~nothing.
    pub fn retire(self, store: &mut SceneGpuStore, cells: &mut [CellSlot<'_>]) -> (RetiredPhase, u32) {
        fence(Ordering::Acquire);
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

    /// §9.2.1 / contract #32 lease-gated compact: like [`Self::compact`],
    /// except `ready` (called once per cell, by `CellId`) decides whether
    /// THIS boundary may run swap-and-pop compaction against that cell.
    /// Build `ready` from `HarvestPipeline::compaction_ready` for each
    /// cell's `(LeaseMask, outstanding HarvestLease set)` pair — a cell
    /// reported not-ready keeps its holes until the next boundary (the same
    /// shape as a pinned-tail deferral, `CellStorage::compact_report`'s
    /// doc); ready cells compact exactly as `compact` would. This is the
    /// seam that consults `any_held()` in production (the perf-val T6
    /// review's missing consumer) — see `compaction_ready`'s doc for the
    /// full safety argument for why proceeding against an overdue-revoked
    /// lease is sound.
    pub fn compact_gated(
        self,
        store: &mut SceneGpuStore,
        cells: &mut [CellSlot<'_>],
        ready: impl FnMut(CellId) -> bool,
    ) -> CompactedPhase {
        store.compact_all_gated(cells, ready);
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
