//! Core data types for the Bevy renderer
//! These are pure data structures with no rendering logic

use bevy::prelude::*;

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
    pub name: String,           // Human readable name like "Early Mesh Preprocessing"
    pub path: String,           // Full diagnostic path like "render/early_mesh_preprocessing/elapsed_gpu"
    pub value_ms: f32,          // Value in milliseconds
    pub percentage: f32,        // Percentage of total frame time
    pub is_gpu: bool,           // True if GPU timing, false if CPU timing
}

/// GPU Pipeline profiling data - like Unreal's "stat gpu"
/// Shows timing for each render pass/phase
#[derive(Debug, Clone, Default)]
pub struct GpuProfilerData {
    pub total_frame_ms: f32,
    pub fps: f32,
    pub frame_count: u64,
    pub total_gpu_ms: f32,
    
    // Dynamic collection of all available diagnostic metrics
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

/// Camera controller
#[derive(Resource, Default, Clone)]
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
    /// Viewport bounds in pixels (x, y, width, height)
    /// This defines the exact region of the back buffer where Bevy should render
    pub viewport_x: f32,
    pub viewport_y: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
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
            move_speed: 30.0,
            pan_speed: 0.01,
            zoom_speed: 1.5,
            look_sensitivity: 0.3,
            boost: false,
            orbit_mode: false,
            orbit_distance: 10.0,
            focus_point: Vec3::ZERO,
            viewport_x: 0.0,
            viewport_y: 0.0,
            viewport_width: 1600.0,
            viewport_height: 900.0,
        }
    }
}

/// View mode for the viewport - controls how the scene is rendered
#[derive(Debug, Clone, Copy, PartialEq, Eq, Resource)]
pub enum ViewMode {
    /// Normal rendering with full lighting and textures
    Lit,
    /// Unlit rendering (no lighting, just base colors)
    Unlit,
    /// Wireframe only (no fill)
    Wireframe,
    /// Lit rendering with wireframe overlay
    WireframeOnShaded,
    /// Show only lighting (no textures)
    LightingOnly,
    /// Show only base color textures (no lighting)
    BaseColor,
    /// Show normals as colors
    Normals,
    /// Show roughness values
    Roughness,
    /// Show metallic values
    Metallic,
}

impl Default for ViewMode {
    fn default() -> Self {
        Self::Lit
    }
}

/// Debug visualization modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugVisualization {
    None,
    /// Show shader complexity (instruction count) as heatmap
    ShaderComplexity,
    /// Show light complexity (number of lights affecting each pixel)
    LightComplexity,
    /// Show overdraw (how many times each pixel is drawn)
    Overdraw,
    /// Show triangle density
    TriangleDensity,
    /// Show LOD levels with color coding
    LODVisualization,
}

/// Rendering settings that control how the scene is displayed
#[derive(Resource, Clone)]
pub struct RenderingSettings {
    /// Current view mode
    pub view_mode: ViewMode,
    /// Enable/disable lighting globally
    pub lighting_enabled: bool,
    /// Show wireframe overlay
    pub wireframe_enabled: bool,
    /// Show grid
    pub grid_enabled: bool,
    /// Debug visualization mode
    pub debug_visualization: DebugVisualization,
    /// Show gizmos (transform handles)
    pub gizmos_enabled: bool,
}

impl Default for RenderingSettings {
    fn default() -> Self {
        Self {
            view_mode: ViewMode::Lit,
            lighting_enabled: true,
            wireframe_enabled: false,
            grid_enabled: true,
            debug_visualization: DebugVisualization::None,
            gizmos_enabled: true,
        }
    }
}

/// Shared textures - double buffered for zero-copy rendering
#[derive(Clone)]
pub struct SharedGpuTextures {
    pub textures: std::sync::Arc<[Handle<Image>; 2]>,
    pub native_handles: std::sync::Arc<std::sync::Mutex<Option<[crate::subsystems::render::NativeTextureHandle; 2]>>>,
    pub write_index: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub read_index: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub frame_number: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub width: u32,
    pub height: u32,
}
