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

// Gizmo stub types
pub mod gizmo_types;

// Gizmo generation (disabled)
pub mod gizmos;

// Gizmo overlay renderer (renders on top of scene)
pub mod gizmo_overlay;

// Gizmo procedural rendering feature
pub mod gizmo_feature;

// Scene building system
pub mod scene_builder;

// DXGI shared texture management
#[cfg(target_os = "windows")]
pub mod dxgi_textures;

// DXGI unsafe operations for raw D3D12 access
#[cfg(target_os = "windows")]
pub mod dxgi_unsafe;

// D3D12 extensions for blade-graphics integration
#[cfg(target_os = "windows")]
pub mod blade_d3d12_ext;

// Helio-specific D3D12 integration
#[cfg(target_os = "windows")]
pub mod helio_d3d12_ext;

// Debug line overlay renderer (raycasts, physics queries, etc.)
pub mod debug_line_renderer;

// Helio Skies volumetric atmosphere feature
pub mod helio_skies_feature;

// Main renderer implementation
pub mod renderer;

// Re-export public API
pub use core::{CameraInput, RenderMetrics, GpuProfilerData, DiagnosticMetric, SharedGpuTextures};
pub use renderer::HelioRenderer;
pub use gizmo_overlay::GizmoFeature;
pub use helio_skies_feature::{HelioSkiesRenderer, HelioSkiesConfig, QualityLevel, ComponentFlags};

// Re-export gizmo stub types
pub use gizmo_types::{
    BevyGizmoType, BevyGizmoAxis, GizmoStateResource,
    ViewportMouseInput, GizmoInteractionState, ActiveRaycastTask, RaycastResult,
};

// Render dimensions (match bevy_renderer)
pub const RENDER_WIDTH: u32 = 1600;
pub const RENDER_HEIGHT: u32 = 900;
