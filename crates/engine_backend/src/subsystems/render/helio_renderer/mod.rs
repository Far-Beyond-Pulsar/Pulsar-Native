//! Helio Renderer v2 — wgpu-native renderer integrated with WGPUI WgpuSurfaceHandle

// Core data structures
pub mod core;

// Gizmo stub types (no rendering, just type definitions)
pub mod gizmo_types;

// Disabled blade-graphics modules (kept as files for future reference):
// - gizmos, scene_builder: used helio_core/helio_render which are removed
// - gizmo_overlay, gizmo_feature, debug_line_renderer: use helio_render_v2::debug_draw when re-implemented
// - dxgi_textures, dxgi_unsafe, blade_d3d12_ext, helio_d3d12_ext: blade/DXGI path removed

// Main renderer implementation
pub mod renderer;

// Re-export public API
pub use core::{CameraInput, RenderMetrics, GpuProfilerData, DiagnosticMetric};
pub use renderer::{HelioRenderer, RendererCommand};

// Re-export gizmo stub types
pub use gizmo_types::{
    BevyGizmoType, BevyGizmoAxis, GizmoStateResource,
    ViewportMouseInput, GizmoInteractionState, ActiveRaycastTask, RaycastResult,
};

// Render dimensions
pub const RENDER_WIDTH: u32 = 1600;
pub const RENDER_HEIGHT: u32 = 900;
