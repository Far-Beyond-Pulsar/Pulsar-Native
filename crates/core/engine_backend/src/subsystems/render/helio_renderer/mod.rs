//! Helio renderer — wgpu-based, renders directly into a WgpuSurface each frame.

mod animated_content;
pub mod core;
mod gizmo_geometry;
mod gpu_trace;
mod interaction;
mod native_voxel_flight;
pub mod renderer;
pub mod voxel_backend;
pub mod voxel_mesh_backend;

pub use core::{
    CameraInput, DiagnosticMetric, GpuProfilerAvailability, GpuProfilerData, RenderMetrics,
    RenderSpikeLogConfig,
};
pub use renderer::{
    EditorCameraState, HelioEditorMailbox, HelioRenderer, PendingPointerEvent, RendererCommand,
    StaticDragWarning, VoxelBrushRequest, VoxelBrushTool,
};

pub const RENDER_WIDTH: u32 = 1600;
pub const RENDER_HEIGHT: u32 = 900;
