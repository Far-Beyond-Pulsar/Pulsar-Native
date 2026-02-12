//! Core data types for the Helio renderer
//! These match BevyRenderer's API but use glam instead of Bevy types

use glam::Vec3;
use std::sync::{Arc, Mutex};
use gpui::GpuTextureHandle;

/// Rendering metrics
#[derive(Debug, Clone, Default)]
pub struct RenderMetrics {
    pub fps: f32,
    pub frame_time_ms: f32,
    pub draw_calls: u32,
    pub memory_usage_mb: f32,
    pub vertices_drawn: u64,
    pub frames_rendered: u64,
    pub bevy_fps: f32,
    pub pipeline_time_us: f32,
    pub gpu_time_us: f32,
    pub cpu_time_us: f32,
}

/// Represents a single diagnostic metric for GPU profiling
#[derive(Debug, Clone)]
pub struct DiagnosticMetric {
    pub name: String,
    pub path: String,
    pub value_ms: f32,
    pub percentage: f32,
    pub is_gpu: bool,
}

/// GPU Pipeline profiling data
#[derive(Debug, Clone, Default)]
pub struct GpuProfilerData {
    pub total_frame_ms: f32,
    pub fps: f32,
    pub frame_count: u64,
    pub total_gpu_ms: f32,
    
    pub render_metrics: Vec<DiagnosticMetric>,
    
    // Legacy fields for backwards compatibility
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

/// Camera controller - matches BevyRenderer's CameraInput
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
            forward: 0.0,
            right: 0.0,
            up: 0.0,
            mouse_delta_x: 0.0,
            mouse_delta_y: 0.0,
            pan_delta_x: 0.0,
            pan_delta_y: 0.0,
            zoom_delta: 0.0,
            move_speed: 10.0,          // Units per second
            pan_speed: 5.0,            // Pan sensitivity
            zoom_speed: 20.0,          // Zoom sensitivity  
            look_sensitivity: 0.3,     // Match Helio's default FpsCamera look_speed
            boost: false,
            orbit_mode: false,
            orbit_distance: 10.0,
            focus_point: Vec3::ZERO,
            viewport_x: 0.0,
            viewport_y: 0.0,
            viewport_width: 2560.0,
            viewport_height: 1440.0,
            needs_resize: false,
        }
    }
}


/// Shared GPU textures for double-buffered rendering
/// Uses blade-graphics textures instead of Bevy Image handles
#[derive(Clone)]
pub struct SharedGpuTextures {
    // We'll store blade_graphics::Texture handles here
    // For now, use placeholder type that we'll fill in during DXGI integration
    pub native_handles: Arc<Mutex<Option<[GpuTextureHandle; 2]>>>,
    pub write_index: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub read_index: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub frame_number: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub width: u32,
    pub height: u32,
}
