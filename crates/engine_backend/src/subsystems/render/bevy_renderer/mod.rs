//! Bevy Renderer with DIRECT rendering to DXGI shared textures
//! Double-buffered implementation - write to one buffer while GPUI reads from the other
//!
//! ## Architecture
//!
//! This renderer is organized into logical modules for maintainability and clarity:
//!
//! ### Core (`core/`)
//! Fundamental data structures with no rendering logic:
//! - `types` - RenderMetrics, GpuProfilerData, CameraInput, SharedGpuTextures
//! - `components` - Bevy ECS components (MainCamera, GameObjectId, Selected)
//! - `resources` - Bevy ECS resources (shared state between systems)
//!
//! ### Systems (`systems/`)
//! Bevy ECS systems that run every frame:
//! - `camera` - Camera input synchronization and Unreal-style movement
//! - `sync` - Thread synchronization (game state, gizmo state, input)
//! - `metrics` - Performance tracking and GPU profiling
//! - `scene` - Scene setup (objects, lights, camera spawning)
//!
//! ### Interaction (`interaction/`)
//! User interaction with the viewport:
//! - `viewport` - Object selection and gizmo manipulation (async raycasting)
//! - `gizmo_interaction` - **DEPRECATED** - Legacy code kept for reference
//!
//! ### Gizmos (`gizmos/`)
//! 3D manipulation visuals for the level editor:
//! - `rendering` - Gizmo mesh generation and rendering systems
//!
//! ### Root Level
//! Critical infrastructure kept at the root for clarity:
//! - `textures` - **CRITICAL** - DXGI shared texture creation and management
//! - `renderer` - Main BevyRenderer struct and initialization
//!
//! ## Double-Buffered Rendering
//!
//! The renderer uses two shared DXGI textures:
//! 1. **Write Buffer** - Bevy renders to this buffer
//! 2. **Read Buffer** - GPUI reads from this buffer for display
//!
//! Buffers are swapped atomically each frame to avoid tearing and blocking.

// Core data structures
pub mod core;

// Bevy systems
pub mod systems;

// User interaction
pub mod interaction;

// Gizmo system
pub mod gizmos;

// Critical root-level modules
pub mod textures;
pub mod renderer;

// Re-export public API
pub use core::{
    RenderMetrics, GpuProfilerData, DiagnosticMetric, CameraInput, SharedGpuTextures,
};
pub use renderer::BevyRenderer;
pub use textures::{RENDER_WIDTH, RENDER_HEIGHT};
pub use gizmos::rendering::{
    GizmoType as BevyGizmoType, GizmoAxis as BevyGizmoAxis, GizmoStateResource,
};
pub use interaction::viewport::{
    ViewportMouseInput, GizmoInteractionState, ActiveRaycastTask, RaycastResult,
};
pub use systems::sync::{sync_gizmo_state_system, sync_viewport_mouse_input_system};
