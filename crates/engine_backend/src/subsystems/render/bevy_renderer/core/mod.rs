//! Core Data Structures
//!
//! This module contains the fundamental data types and structures used throughout
//! the Bevy renderer. These are pure data definitions with no rendering logic.
//!
//! ## Organization
//!
//! - `types` - Core data structures (RenderMetrics, GpuProfilerData, CameraInput, etc.)
//! - `components` - Bevy ECS components (MainCamera, GameObjectId, Selected, etc.)
//! - `resources` - Bevy ECS resources (shared state accessible across systems)
//!
//! ## Design Philosophy
//!
//! These modules contain only data definitions and simple constructors. All business
//! logic and rendering code belongs in the systems/ or other functional modules.

pub mod types;
pub mod components;
pub mod resources;

// Re-export commonly used types for convenience
pub use types::{
    RenderMetrics, GpuProfilerData, DiagnosticMetric, CameraInput, SharedGpuTextures,
};
pub use components::{MainCamera, GameObjectId, Selected, SelectionOutline};
pub use resources::{
    SharedTexturesResource, ShutdownFlag, GameThreadResource, CameraInputResource,
    SharedGizmoStateResource, SharedViewportMouseInputResource, MetricsResource,
    GpuProfilerResource, WgpuProfilerResource,
};
