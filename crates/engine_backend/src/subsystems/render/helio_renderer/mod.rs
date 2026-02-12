//! Helio Renderer with DIRECT rendering to DXGI shared textures
//! Matches BevyRenderer's architecture but uses blade-graphics + Helio features
//!
//! ## Architecture
//!
//! This renderer mirrors bevy_renderer's structure but uses:
//! - blade-graphics instead of wgpu/bevy
//! - Helio's feature-driven rendering system
//! - glam for math types instead of bevy math

// Core data structures
pub mod core;

// Main renderer implementation
pub mod renderer;

// Re-export public API (matches bevy_renderer exports)
pub use core::{
    RenderMetrics, GpuProfilerData, DiagnosticMetric, CameraInput, SharedGpuTextures,
};
pub use renderer::HelioRenderer;

// Re-export gizmo types from bevy_renderer (temporary - will implement Helio gizmos later)
pub use crate::subsystems::render::bevy_renderer::{
    BevyGizmoType, BevyGizmoAxis, GizmoStateResource,
    ViewportMouseInput, GizmoInteractionState, ActiveRaycastTask, RaycastResult,
};

// Render dimensions (match bevy_renderer)
pub const RENDER_WIDTH: u32 = 1600;
pub const RENDER_HEIGHT: u32 = 900;
