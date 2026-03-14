//! Shared data types used across all renderer backends.

use glam::Vec3;
use std::sync::{Arc, Mutex, atomic::{AtomicUsize, AtomicU64}};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// GPU texture handle (platform-native zero-copy interop)
// ---------------------------------------------------------------------------

/// A native OS handle to a GPU texture that can be shared between APIs
/// (D3D12 ↔ D3D11, Metal ↔ IOSurface, Vulkan ↔ dma-buf).
#[derive(Clone, Debug)]
pub struct GpuTextureHandle {
    /// Platform-native handle:
    /// - Windows: NT HANDLE value cast to `isize`
    /// - macOS: IOSurface ID
    /// - Linux: dma-buf file descriptor
    pub handle: isize,
    pub width: u32,
    pub height: u32,
}

impl GpuTextureHandle {
    pub fn new(handle: isize, width: u32, height: u32) -> Self {
        Self { handle, width, height }
    }
}

// ---------------------------------------------------------------------------
// Per-frame input & scene description
// ---------------------------------------------------------------------------

/// Camera controller state passed from the engine to the renderer each frame.
#[derive(Default, Clone)]
pub struct CameraInput {
    pub forward: f32,
    pub right: f32,
    pub up: f32,
    pub mouse_delta_x: f32,
    pub mouse_delta_y: f32,
    pub pan_delta_x: f32,
    pub pan_delta_y: f32,
    pub zoom_delta: f32,
    pub move_speed: f32,
    pub pan_speed: f32,
    pub zoom_speed: f32,
    pub look_sensitivity: f32,
    pub boost: bool,
    pub orbit_mode: bool,
    pub orbit_distance: f32,
    pub focus_point: Vec3,
    pub viewport_x: f32,
    pub viewport_y: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub needs_resize: bool,
}

impl CameraInput {
    pub fn new() -> Self {
        Self {
            move_speed: 10.0,
            pan_speed: 5.0,
            zoom_speed: 20.0,
            look_sensitivity: 0.3,
            orbit_distance: 10.0,
            viewport_width: 2560.0,
            viewport_height: 1440.0,
            ..Default::default()
        }
    }
}

/// Everything the renderer needs to produce one frame.
///
/// This is intentionally lightweight for Phase 1; backends that need richer
/// scene data (e.g. HelioRenderer) read from a shared `SceneDb` directly and
/// only use this struct for camera/viewport metadata.
#[derive(Default, Clone)]
pub struct RenderScene {
    pub camera: CameraInput,
    pub viewport_width: u32,
    pub viewport_height: u32,
}

// ---------------------------------------------------------------------------
// Per-frame output
// ---------------------------------------------------------------------------

/// The result produced by one call to [`Renderer::render_frame`].
#[derive(Default, Clone)]
pub struct RenderOutput {
    /// The shared GPU texture handle for the rendered frame, if available.
    pub texture: Option<GpuTextureHandle>,
    /// Performance snapshot for this frame.
    pub metrics: RenderMetrics,
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// Per-frame rendering performance metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RenderMetrics {
    pub fps: f32,
    pub frame_time_ms: f32,
    pub draw_calls: u32,
    pub memory_usage_mb: f32,
    pub vertices_drawn: u64,
    pub frames_rendered: u64,
    /// Legacy field kept for backwards compatibility with the old BevyRenderer API.
    pub bevy_fps: f32,
    pub pipeline_time_us: f32,
    pub gpu_time_us: f32,
    pub cpu_time_us: f32,
}

/// A single named GPU/CPU pass timing entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticMetric {
    pub name: String,
    pub path: String,
    pub value_ms: f32,
    pub percentage: f32,
    pub is_gpu: bool,
}

/// Detailed GPU pipeline profiling data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GpuProfilerData {
    pub total_frame_ms: f32,
    pub fps: f32,
    pub frame_count: u64,
    pub total_gpu_ms: f32,
    pub render_metrics: Vec<DiagnosticMetric>,
    // Legacy per-pass fields kept for backwards compatibility.
    pub shadow_pass_ms: f32,
    pub shadow_pass_pct: f32,
    pub opaque_pass_ms: f32,
    pub opaque_pass_pct: f32,
    pub alpha_mask_pass_ms: f32,
    pub alpha_mask_pass_pct: f32,
    pub transparent_pass_ms: f32,
    pub transparent_pass_pct: f32,
    pub lighting_ms: f32,
    pub lighting_pct: f32,
    pub post_processing_ms: f32,
    pub post_processing_pct: f32,
    pub ui_pass_ms: f32,
    pub ui_pass_pct: f32,
}

// ---------------------------------------------------------------------------
// Shared GPU textures (double-buffered)
// ---------------------------------------------------------------------------

/// Double-buffered GPU textures shared between the renderer thread and the UI.
#[derive(Clone)]
pub struct SharedGpuTextures {
    pub native_handles: Arc<Mutex<Option<[GpuTextureHandle; 2]>>>,
    pub write_index: Arc<AtomicUsize>,
    pub read_index: Arc<AtomicUsize>,
    pub frame_number: Arc<AtomicU64>,
    pub width: u32,
    pub height: u32,
}

// ---------------------------------------------------------------------------
// CPU framebuffer (headless / software rendering)
// ---------------------------------------------------------------------------

/// A CPU-side RGBA8 framebuffer used by the headless renderer and for testing.
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    /// RGBA8 linear pixel data: `width * height * 4` bytes, row-major top-to-bottom.
    pub buffer: Vec<u8>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let buffer_size = (width * height * 4) as usize;
        Self { width, height, buffer: vec![0; buffer_size] }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.buffer.resize((width * height * 4) as usize, 0);
    }

    pub fn clear(&mut self, color: [u8; 4]) {
        for chunk in self.buffer.chunks_exact_mut(4) {
            chunk.copy_from_slice(&color);
        }
    }
}
