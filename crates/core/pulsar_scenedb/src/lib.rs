//! SceneDB 2.0 — Layer 1 storage core (spec Rev 2.3, CONTRACTS.md C0–C6;
//! C5 material layout and Test 13 pending M3).
//!
//! Seeded from `pulsar_ecs` (which remains in-tree as the reference
//! implementation). This crate adds the spec-conformant storage layer:
//!
//! - [`Handle`] — packed u64, stable slot index + generation, gen 0 invalid
//! - [`HandleRegistry`] — slot allocator, generation validation, slot→row
//!   indirection, permanent retirement at gen `u32::MAX`
//! - [`Page`]/[`PageLayout`] — single-allocation 64-byte-aligned SoA pages,
//!   128-byte stride guardrail, 1024-element ceiling
//! - [`LivenessMask`] — atomic per-element liveness, deferred deletion
//! - [`CellStorage`] — alloc/free/deref + frame-boundary swap-and-pop
//!   compaction that preserves handle validity
//! - [`SpatialCell`] — six SoA bounds columns + the §8 AABB query writing
//!   sentinel-aligned row tokens into caller scratch (scalar reference;
//!   SIMD paths land in M1b and must match bit-for-bit)
//! - [`TypeToken`]/[`CellType`] — dense column-type tokens bridged to
//!   `pulsar_reflection`; holistic-stride-checked cell composition
//! - SIMD query dispatch (internal `simd` kernels) — AVX2 arms verified
//!   bit-for-bit against the scalar reference; frustum + AABB
//! - [`LeaseMask`]/[`Scratchpad`]/[`LivenessSnapshot`] — read-lease pool,
//!   decaying scratchpads, double-buffered revocation (§9; phase machine is M2)
//! - `gpu` (feature `gpu`) — M2a/M2b GPU-resident store: `EngineGpuContext`,
//!   `SceneBuffer<T>` row-indexed SSBOs with coalescing delta-sync (M2a);
//!   M2b-α adds region-partitioned global buffers, size-class pools (C2
//!   default 256 / max 1024 per class), per-cell `CellGpuState` (dirty masks,
//!   pending retires, gen shadow, slot shadow), self-healing slot-mirror
//!   boundary scan, `register_cell` promotion primitive, `rebuild` for device
//!   loss. Asset store (M2b-α): `GeometryArena` RangeList suballocation,
//!   `MeshRegistry` (C5: 72 B, XOR-validated), `ClusterBuffer` (C5: 48 B,
//!   NaN-rejecting error-monotonicity validation), both with corrupted-VRAM
//!   rebuild gates. Phase machine (M2b-α):
//!   `FrameDriver` → SimulateA→SimulateB→Harvest→Boundary witnesses;
//!   `BoundaryPhase::retire` returns drain count; compile_fail + positive
//!   doc-tests enforce correctness. The core stays graphics-free (C0); CI
//!   guards `--no-default-features`.
//!
//! The inherited archetype ECS modules (`world`, `archetype`, `query`, …)
//! are retained and will be migrated onto paged storage in later milestones
//! (the SceneDB-replaces-ECS path, design doc §7).
//!
//! Milestone status: M1 (Layer 1) complete; M2a (GPU store, delta-sync,
//! pin-by-serial retirement) complete — verified headless by Tests 3, 6 (host),
//! and 14. M2b-α (region-partitioned `SceneGpuStore`, asset store, phase
//! machine, compile-time correctness gates) complete — verified by Tests 3, 6,
//! 14 extended suites and compile_fail doc-tests. M2b-β (streaming grid,
//! harvest pipeline, DEI compaction) and M3 (Helio inversion) follow.

pub mod actor;
pub mod archetype;
pub mod cell;
pub mod cell_type;
pub mod component;
pub mod component_store;
pub mod entity;
pub mod handle;
pub mod lease;
pub mod liveness;
pub mod page;
pub mod query;
pub mod registry;
pub mod schedule;
pub mod simd;
pub mod snapshot;
pub mod spatial;
pub mod token;
pub mod world;

#[cfg(feature = "gpu")]
pub mod gpu;

pub use actor::{Actor, ActorRegistry};
pub use archetype::{Archetype, ArchetypeId, ArchetypeKey};
pub use cell::CellStorage;
pub use cell_type::{CellType, CellTypeError, RegisteredCellType};
pub use component::{component_id, Component, ComponentId};
pub use component_store::{__bp_clear_comp_ctx, __bp_set_comp_ctx, __bp_with_comp, ComponentStore};
pub use entity::Entity;
pub use handle::Handle;
pub use lease::{Lease, LeaseMask, Scratchpad, DECAY_FRAMES, LEASE_SLOTS};
pub use liveness::LivenessMask;
pub use page::{
    ColumnDesc, LayoutError, Page, PageLayout, Pod, DEFAULT_PAGE_CAPACITY, MAX_PAGE_CAPACITY,
    MAX_STRIDE_BYTES,
};
pub use pulsar_core::GameTime;
pub use query::{QueryIter, WorldQuery};
pub use registry::{HandleRegistry, NULL_ROW};
pub use schedule::Schedule;
pub use snapshot::{LivenessSnapshot, RevocationFlag};
pub use spatial::{Aabb, Frustum, SpatialCell, SPATIAL_COLUMNS, TRANSFORM_COLUMN};
pub use token::TypeToken;
pub use world::World;
