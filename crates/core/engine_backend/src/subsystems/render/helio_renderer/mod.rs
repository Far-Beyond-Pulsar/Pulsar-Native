//! Helio renderer — wgpu-based, renders directly into a WgpuSurface each frame.

mod animated_content;
pub mod camera_ground;
pub mod core;
pub mod fly_camera;
mod gizmo_geometry;
mod gpu_trace;
mod interaction;
pub mod material_sampler;
pub mod pipelines;
pub mod project_settings;
pub mod render_features;
pub mod renderer;
pub mod scene_renderer;

pub use animated_content::AnimatedContentRegistration;
pub use camera_ground::{altitude_speed_scale, CameraGround};
pub use core::{
    CameraInput, DiagnosticMetric, GpuProfilerAvailability, GpuProfilerData, RenderMetrics,
    RenderSpikeLogConfig,
};
pub use fly_camera::FlyCamera;
pub use render_features::{
    EditorLines, EditorLinesRegistration, RenderFeatureRegistration, ViewOverscanRegistration,
};
pub use renderer::{
    CameraSync, CompletedDrag, EditorCameraState, HelioEditorMailbox, HelioRenderer,
    PendingPointerEvent, RendererCommand, StaticDragWarning, ToolRay, ViewKind, ViewPayload,
    ViewportDebugLegend, ViewportDebugView, CAMERA_PIVOT_DISTANCE,
};
pub use scene_renderer::{
    DeviceOwner, FrameCrop, FrameProjection, FrameView, PreparedFrame, SceneRenderer,
    SceneRendererOptions,
};
