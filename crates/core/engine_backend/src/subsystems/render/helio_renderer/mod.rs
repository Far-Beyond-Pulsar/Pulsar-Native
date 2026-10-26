//! Helio renderer — wgpu-based, renders directly into a WgpuSurface each frame.

mod animated_content;
pub mod core;
pub mod fly_camera;
mod gizmo_geometry;
mod gpu_trace;
mod interaction;
pub mod material_sampler;
pub mod pipelines;
pub mod project_settings;
pub mod renderer;
pub mod scene_renderer;
pub mod voxel_backend;
pub mod voxel_mesh_backend;

pub use core::{
    CameraInput, DiagnosticMetric, GpuProfilerAvailability, GpuProfilerData, RenderMetrics,
    RenderSpikeLogConfig,
};
pub use fly_camera::{altitude_speed_scale, FlyCamera};
pub use renderer::{
    CameraSync, CompletedDrag, EditorCameraState, HelioEditorMailbox, HelioRenderer,
    PendingPointerEvent, PortalDebugText, RendererCommand, StaticDragWarning, ViewKind,
    ViewportDebugLegend, ViewportDebugView, VoxelBrushRequest, VoxelBrushTool,
    CAMERA_PIVOT_DISTANCE, PORTAL_DEBUG_VIEW_MODE,
};
pub use scene_renderer::{
    DeviceOwner, FrameProjection, FrameView, PreparedFrame, SceneRenderer, SceneRendererOptions,
    VoxelGround,
};
pub use voxel_backend::{VoxelChunkSaves, VoxelSavedChunks};
