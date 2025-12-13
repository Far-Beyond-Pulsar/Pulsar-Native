//! Bevy resources for the renderer

use bevy::prelude::*;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU32, AtomicU64}};
use super::types::{CameraInput, SharedGpuTextures, GpuProfilerData};

#[derive(Resource)]
pub struct SharedTexturesResource(pub Arc<Mutex<Option<SharedGpuTextures>>>);

#[derive(Resource, Clone)]
pub struct ShutdownFlag(pub Arc<AtomicBool>);

/// Resource containing reference to the game thread for syncing object positions
#[derive(Resource, Clone)]
pub struct GameThreadResource(pub Option<Arc<Mutex<crate::subsystems::game::GameState>>>);

/// Resource containing the shared camera input from the input thread
#[derive(Resource, Clone)]
pub struct CameraInputResource(pub Arc<Mutex<CameraInput>>);

/// Resource containing shared gizmo state from GPUI (updated by editor, read by Bevy)
#[derive(Resource, Clone)]
pub struct SharedGizmoStateResource(pub Arc<Mutex<crate::subsystems::render::bevy_renderer::gizmos::rendering::GizmoStateResource>>);

/// Resource containing shared viewport mouse input from GPUI (for object selection raycasts)
#[derive(Resource, Clone)]
pub struct SharedViewportMouseInputResource(pub Arc<parking_lot::Mutex<crate::subsystems::render::bevy_renderer::interaction::viewport::ViewportMouseInput>>);


/// Metrics tracking resource - shared between Bevy and the main thread
#[derive(Resource, Clone)]
pub struct MetricsResource {
    pub frames_rendered: Arc<AtomicU64>,
    pub last_frame_time: Arc<Mutex<std::time::Instant>>,
    pub fps: Arc<Mutex<f32>>,
    pub frame_time_ms: Arc<Mutex<f32>>,
    pub draw_calls: Arc<AtomicU32>,
    pub vertices_drawn: Arc<AtomicU64>,
    pub memory_usage_mb: Arc<Mutex<f32>>,
}

impl Default for MetricsResource {
    fn default() -> Self {
        Self {
            frames_rendered: Arc::new(AtomicU64::new(0)),
            last_frame_time: Arc::new(Mutex::new(std::time::Instant::now())),
            fps: Arc::new(Mutex::new(0.0)),
            frame_time_ms: Arc::new(Mutex::new(0.0)),
            draw_calls: Arc::new(AtomicU32::new(0)),
            vertices_drawn: Arc::new(AtomicU64::new(0)),
            memory_usage_mb: Arc::new(Mutex::new(0.0)),
        }
    }
}

/// GPU Profiler Resource - stores detailed render pipeline timing (shared with main thread)
#[derive(Resource, Clone)]
pub struct GpuProfilerResource {
    pub data: Arc<Mutex<GpuProfilerData>>,
}

impl Default for GpuProfilerResource {
    fn default() -> Self {
        Self {
            data: Arc::new(Mutex::new(GpuProfilerData::default())),
        }
    }
}

/// Render world resource that holds the actual wgpu-profiler instance
#[derive(Resource)]
pub struct WgpuProfilerResource {
    pub profiler: wgpu_profiler::GpuProfiler,
}
