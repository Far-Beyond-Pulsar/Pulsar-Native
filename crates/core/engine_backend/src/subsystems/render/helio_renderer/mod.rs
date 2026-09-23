//! Helio renderer — wgpu-based, renders directly into a WgpuSurface each frame.

pub mod core;
mod interaction;
mod gizmo_geometry;
pub mod renderer;

pub use core::{
    CameraInput, DiagnosticMetric, GpuProfilerAvailability, GpuProfilerData, RenderMetrics,
    RenderSpikeLogConfig,
};
pub use renderer::{
    EditorCameraState, HelioEditorMailbox, HelioRenderer, PendingPointerEvent, RendererCommand,
};

pub const RENDER_WIDTH: u32 = 1600;
pub const RENDER_HEIGHT: u32 = 900;
