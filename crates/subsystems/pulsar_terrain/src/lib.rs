//! Authoritative sparse planetary voxel terrain state.
//!
//! This crate owns canonical planet-space addressing, deterministic generation
//! and edits, the mutable sparse hierarchy, stable page encoding, and durable
//! snapshots. Rendering and physics consume derived data and never become the
//! source of truth.

mod component;
mod core;
mod edit;
mod generator;
mod hierarchy;
mod page;
mod runtime;
mod snapshot;
mod store;
mod types;

pub use component::{
    ComponentError, PlanetDefinition, PlanetTerrainComponent, PLANET_TERRAIN_CLASS_NAME,
};
pub use core::{
    PageBuildCommitOutcome, PageBuildPreparation, PageBuildRequest, PageBuildResult, TerrainCore,
    TerrainCoreError, TerrainMemoryCounters, TerrainWorkCounters,
};
pub use edit::{EditError, EditLog, EditMode, EditOp, EditShape};
pub use generator::{DeterministicGenerator, FixedSphereGenerator};
pub use hierarchy::{HierarchyError, SparseBrickTree};
pub use page::{
    PageCodecError, VoxelPage, CELL_COUNT, MICROBRICKS_PER_AXIS, MICROBRICK_COUNT, MICROBRICK_EDGE,
    PAGE_EDGE,
};
pub use runtime::{
    TerrainBackpressure, TerrainRequestClass, TerrainRequestOutcome, TerrainRuntimeConfig,
    TerrainRuntimeCounters, TerrainRuntimeError, TerrainRuntimeEvent, TerrainRuntimeHandle,
    TerrainSubsystem, TERRAIN_SUBSYSTEM_ID,
};
pub use snapshot::{CompactedPageRecord, SnapshotCodecError, TerrainSnapshot};
pub use store::{SnapshotRecord, TerrainStore, TerrainStoreError};
pub use types::{
    CellWord, ContentHash, MaterialId, NodeState, PageId, PageKey, PlanetId, PlanetIdParseError,
    PlanetPosition, LOD0_CELL_SIZE_METERS, PAGE_EDGE_CELLS,
};
