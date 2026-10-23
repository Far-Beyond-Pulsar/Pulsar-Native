//! Helio renderer — wgpu-based, renders directly into a WgpuSurface each frame.

mod animated_content;
pub mod core;
pub mod material_sampler;
pub mod project_settings;
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
    CameraSync, CompletedDrag, EditorCameraState, CAMERA_PIVOT_DISTANCE, HelioEditorMailbox, HelioRenderer,
    PendingPointerEvent,
    PortalDebugText, RendererCommand, StaticDragWarning, ViewKind, ViewportDebugLegend, ViewportDebugView,
    PORTAL_DEBUG_VIEW_MODE,
    VoxelBrushRequest,
    VoxelBrushTool,
};
pub use voxel_backend::{VoxelChunkSaves, VoxelSavedChunks};
