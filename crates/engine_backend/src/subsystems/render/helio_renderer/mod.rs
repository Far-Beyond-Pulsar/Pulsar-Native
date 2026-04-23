//! Helio renderer — wgpu-based, renders directly into a WgpuSurface each frame.

pub mod core;
pub mod renderer;

pub use core::{CameraInput, DiagnosticMetric, GpuProfilerData, RenderMetrics};
pub use renderer::{gizmo_types, HelioRenderer, RendererCommand};

pub const RENDER_WIDTH: u32 = 1600;
pub const RENDER_HEIGHT: u32 = 900;
