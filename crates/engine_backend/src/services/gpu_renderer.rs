//! GPU renderer service — thin wrapper around HelioRenderer.
//!
//! Initialisation is synchronous and lazy: the Helio renderer itself creates its
//! wgpu resources on the first `render_frame_to_surface` call, once the
//! WgpuSurface is available.

use crate::subsystems::render::{HelioRenderer, RenderMetrics};
use crate::scene::SceneDb;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Builder for `GpuRenderer`.
pub struct GpuRendererBuilder {
    scene_db:           Option<Arc<SceneDb>>,
    _game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
    _physics_query:     Option<Arc<crate::services::PhysicsQueryService>>,
}

impl GpuRendererBuilder {
    pub fn new(_width: u32, _height: u32) -> Self {
        Self { scene_db: None, _game_thread_state: None, _physics_query: None }
    }

    pub fn scene_db(mut self, db: Arc<SceneDb>) -> Self {
        self.scene_db = Some(db);
        self
    }

    pub fn game_thread(mut self, gt: Arc<Mutex<crate::subsystems::game::GameState>>) -> Self {
        self._game_thread_state = Some(gt);
        self
    }

    pub fn physics(mut self, pq: Arc<crate::services::PhysicsQueryService>) -> Self {
        self._physics_query = Some(pq);
        self
    }

    pub fn build(self) -> GpuRenderer {
        let scene_db = self.scene_db.unwrap_or_else(|| Arc::new(SceneDb::new()));
        GpuRenderer {
            helio_renderer: Some(HelioRenderer::new(scene_db)),
            frame_count:    0,
            start_time:     Instant::now(),
        }
    }
}

/// GPU renderer — drives Helio through a GPUI `WgpuSurfaceHandle`.
pub struct GpuRenderer {
    pub helio_renderer: Option<HelioRenderer>,
    frame_count: u64,
    start_time:  Instant,
}

impl GpuRenderer {
    /// Render one frame directly into a `WgpuSurfaceHandle` back-buffer view.
    pub fn render_frame_to_surface(
        &mut self,
        device: &wgpu::Device,
        queue:  &wgpu::Queue,
        view:   &wgpu::TextureView,
        width:  u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) {
        println!("[GPU-RENDERER] render_frame_to_surface called, helio_renderer present: {}", self.helio_renderer.is_some());
        if let Some(ref mut r) = self.helio_renderer {
            println!("[GPU-RENDERER] Calling helio_renderer.render_frame...");
            r.render_frame(device, queue, view, width, height, format);
            println!("[GPU-RENDERER] helio_renderer.render_frame returned");
        } else {
            println!("[GPU-RENDERER] No helio_renderer!");
        }
        self.frame_count += 1;
    }

    pub fn get_fps(&self) -> f32 {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        if self.frame_count > 0 && elapsed > 0.0 {
            self.frame_count as f32 / elapsed
        } else {
            self.get_helio_fps()
        }
    }

    pub fn get_helio_fps(&self) -> f32 {
        self.helio_renderer.as_ref()
            .map(|r| r.get_metrics().fps)
            .unwrap_or(0.0)
    }

    pub fn get_render_fps(&self) -> f32 {
        self.get_fps().max(self.get_helio_fps())
    }

    pub fn is_initialized(&self) -> bool {
        self.helio_renderer.as_ref()
            .map(|r| r.is_initialized())
            .unwrap_or(false)
    }

    pub fn get_render_metrics(&self) -> Option<RenderMetrics> {
        self.helio_renderer.as_ref().map(|r| r.get_metrics())
    }

    pub fn get_pipeline_time_us(&self) -> u64 { 0 }
    pub fn get_gpu_time_us(&self)      -> u64 { 0 }
    pub fn get_cpu_time_us(&self)      -> u64 { 0 }

    pub fn get_gpu_profiler_data(&self) -> Option<crate::subsystems::render::GpuProfilerData> {
        self.helio_renderer.as_ref().map(|r| r.get_gpu_profiler_data())
    }

    pub fn get_frame_count(&self) -> u64 { self.frame_count }
}

unsafe impl Send for GpuRenderer {}
unsafe impl Sync for GpuRenderer {}
