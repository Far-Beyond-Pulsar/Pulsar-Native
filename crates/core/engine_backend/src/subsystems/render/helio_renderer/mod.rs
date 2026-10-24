//! Helio renderer — wgpu-based, renders directly into a WgpuSurface each frame.

mod animated_content;
pub mod core;
pub mod fly_camera;
pub mod material_sampler;
pub mod project_settings;
mod gizmo_geometry;
mod gpu_trace;
mod interaction;
pub mod renderer;
pub mod scene_renderer;
pub mod voxel_backend;
pub mod voxel_mesh_backend;

pub use fly_camera::{altitude_speed_scale, FlyCamera};
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
pub use scene_renderer::{
    DeviceOwner, FrameProjection, FrameView, PreparedFrame, SceneRenderer, SceneRendererOptions,
    VoxelGround,
};
pub use voxel_backend::{VoxelChunkSaves, VoxelSavedChunks};
