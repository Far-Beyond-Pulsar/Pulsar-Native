//! Helio renderer — wgpu-based, renders directly into a WgpuSurface each frame.

mod animated_content;
pub mod core;
mod gizmo_geometry;
mod gpu_trace;
mod interaction;
pub mod renderer;
pub mod voxel_backend;
pub mod voxel_mesh_backend;

pub use core::{
    CameraInput, DiagnosticMetric, GpuProfilerAvailability, GpuProfilerData, RenderMetrics,
    RenderSpikeLogConfig,
};
pub use renderer::{
    CompletedDrag, EditorCameraState, HelioEditorMailbox, HelioRenderer, PendingPointerEvent,
    RendererCommand, StaticDragWarning, ViewKind, ViewportDebugLegend, ViewportDebugView,
    VoxelBrushRequest,
    VoxelBrushTool,
};
