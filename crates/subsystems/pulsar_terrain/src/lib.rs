//! Authoritative sparse planetary voxel terrain state.
//!
//! This crate owns canonical planet-space addressing, deterministic generation
//! and edits, the mutable sparse hierarchy, stable page encoding, and durable
//! snapshots. Rendering and physics consume derived data and never become the
//! source of truth.

mod controller;
mod core;
mod edit;
mod generator;
mod hierarchy;
mod mutation;
mod page;
mod persistence;
mod planet;
mod planning;
mod refinement;
mod render;
mod runtime;
mod sampling;
mod snapshot;
mod store;
mod streaming;
mod types;

pub use controller::{
    TerrainControllerConfig, TerrainControllerError, TerrainControllerFrame,
    TerrainPlanningFailure, TerrainStreamingController,
};
pub use core::{
    PageBuildCommitOutcome, PageBuildPreparation, PageBuildRequest, PageBuildResult, TerrainCore,
    TerrainCoreError, TerrainMemoryCounters, TerrainPlanningSnapshot, TerrainWorkCounters,
};
pub use edit::{EditError, EditLog, EditMode, EditOp, EditShape};
pub use generator::{DeterministicGenerator, FixedSphereGenerator};
pub use hierarchy::{HierarchyError, SparseBrickTree};
pub use mutation::{
    TerrainOverrideError, TerrainOverrideLog, TerrainOverrideOp, TerrainOverrideTarget,
};
pub use page::{
    PageCodecError, VoxelPage, CELL_COUNT, MICROBRICKS_PER_AXIS, MICROBRICK_COUNT, MICROBRICK_EDGE,
    PAGE_EDGE,
};
pub use persistence::{
    TerrainPersistenceConfig, TerrainPersistenceCounters, TerrainPersistenceError,
    TerrainPersistenceEvent, TerrainPersistenceFailureKind, TerrainPersistenceHandle,
    TerrainPersistenceRequestKind, TerrainPersistenceRequestOutcome, TerrainPersistenceTicket,
};
pub use planet::PlanetDefinition;
pub use planning::{
    TerrainPlanningConfig, TerrainPlanningCounters, TerrainPlanningError, TerrainPlanningHandle,
    TerrainPlanningResult, TerrainPlanningTicket,
};
pub use refinement::{
    TerrainIncrementalResidencySession, TerrainRefinementConfig, TerrainRefinementCounters,
    TerrainRefinementError, TerrainRefinementFrontier, TerrainRefinementReport,
};
pub use render::{
    TerrainPageEvict, TerrainPageUpload, TerrainPlanetEvict, TerrainRenderCommand,
    TerrainRenderCommandDisposition, TerrainRenderCommandFeedback, TerrainRenderCommandId,
    TerrainRenderDelta, TerrainRenderDeltaConfig, TerrainRenderDeltaCounters,
    TerrainRenderDeltaError, TerrainRenderDeltaPublisher, TerrainRenderFeedback,
    TerrainTransitionFace, TerrainVisiblePage, TerrainVisiblePageSet, TERRAIN_TRANSITION_FACE_MASK,
};
pub use runtime::{
    TerrainBackpressure, TerrainRequestClass, TerrainRequestOutcome, TerrainResidentPageGeneration,
    TerrainRuntimeConfig, TerrainRuntimeCounters, TerrainRuntimeError, TerrainRuntimeEvent,
    TerrainRuntimeHandle, TerrainSubsystem, TERRAIN_SUBSYSTEM_ID,
};
pub use sampling::{terrain_surface_required_pages, TerrainSurfaceSamplingError};
pub use snapshot::{CompactedPageRecord, SnapshotCodecError, TerrainSnapshot};
pub use store::{SnapshotRecord, TerrainStore, TerrainStoreError};
pub use streaming::{
    PageDemand, PlanetView, TerrainRegion, TerrainRegionClassifier, TerrainStreamingConfig,
    TerrainStreamingCounters, TerrainStreamingError, TerrainStreamingLimit, TerrainStreamingPlan,
    TerrainStreamingPlanner,
};
pub use types::{
    CellWord, ContentHash, MaterialId, NodeState, PageId, PageKey, PlanetFrame, PlanetFramePayload,
    PlanetId, PlanetIdParseError, PlanetPosition, PositionError, TerrainNodeSummary,
    LOD0_CELL_SIZE_METERS, MILLIMETER_INTERACTION_RADIUS_METERS, PAGE_EDGE_CELLS,
};
