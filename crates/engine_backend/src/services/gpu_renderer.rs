//! GPU renderer service — thin wrapper around HelioRenderer.
//!
//! Initialisation is synchronous and lazy: the Helio renderer itself creates its
//! wgpu resources on the first `render_frame_to_surface` call, once the
//! WgpuSurface is available.

use crate::scene::SceneDb;
use crate::subsystems::render::{HelioRenderer, RenderMetrics};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Builder for `GpuRenderer`.
pub struct GpuRendererBuilder {
    scene_db: Option<Arc<SceneDb>>,
    _game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
    _physics_query: Option<Arc<crate::services::PhysicsQueryService>>,
}

impl GpuRendererBuilder {
    pub fn new(_width: u32, _height: u32) -> Self {
        Self {
            scene_db: None,
            _game_thread_state: None,
            _physics_query: None,
        }
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
            pending_scene_inserts: Vec::new(),
            frame_count: 0,
            start_time: Instant::now(),
        }
    }
}

/// GPU renderer — drives Helio through a GPUI `WgpuSurfaceHandle`.
pub struct GpuRenderer {
    pub helio_renderer: Option<HelioRenderer>,
    pending_scene_inserts: Vec<helio_asset_compat::ConvertedScene>,
    frame_count: u64,
    start_time: Instant,
}

impl GpuRenderer {
    /// Render one frame directly into a `WgpuSurfaceHandle` back-buffer view.
    pub fn render_frame_to_surface(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) {
        if let Some(ref mut r) = self.helio_renderer {
            r.render_frame(device, queue, view, width, height, format);

            // Imports can be requested before the first render pass initializes
            // Helio internals. Defer and replay once ready.
            if r.is_initialized() && !self.pending_scene_inserts.is_empty() {
                let pending = std::mem::take(&mut self.pending_scene_inserts);
                for scene in pending {
                    if let Err(err) = r.insert_converted_scene(scene) {
                        tracing::error!("Failed to insert deferred scene: {err}");
                    }
                }
            }
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
        self.helio_renderer
            .as_ref()
            .map(|r| r.get_metrics().fps)
            .unwrap_or(0.0)
    }

    pub fn get_render_fps(&self) -> f32 {
        self.get_fps().max(self.get_helio_fps())
    }

    pub fn is_initialized(&self) -> bool {
        self.helio_renderer
            .as_ref()
            .map(|r| r.is_initialized())
            .unwrap_or(false)
    }

    pub fn get_render_metrics(&self) -> Option<RenderMetrics> {
        self.helio_renderer.as_ref().map(|r| r.get_metrics())
    }

    pub fn get_gpu_profiler_data(&self) -> Option<crate::subsystems::render::GpuProfilerData> {
        self.helio_renderer
            .as_ref()
            .map(|r| r.get_gpu_profiler_data())
    }

    pub fn get_frame_count(&self) -> u64 {
        self.frame_count
    }

    /// Insert a loaded scene object into the Helio renderer.
    ///
    /// This method takes a `ConvertedScene` from helio-asset-compat and inserts
    /// the meshes, materials, and textures into the active scene.
    pub fn insert_scene_object(
        &mut self,
        scene: helio_asset_compat::ConvertedScene,
    ) -> Result<(), Box<dyn std::error::Error>> {
        tracing::info!(
            "Inserting scene object: {} ({} meshes, {} materials, {} textures)",
            scene.name,
            scene.meshes.len(),
            scene.materials.len(),
            scene.textures.len()
        );

        let Some(renderer) = self.helio_renderer.as_mut() else {
            return Err(std::io::Error::other("Helio renderer is not available").into());
        };

        if !renderer.is_initialized() {
            tracing::info!(
                "Helio renderer not initialized yet; deferring scene insertion for '{}'",
                scene.name
            );
            self.pending_scene_inserts.push(scene);
            return Ok(());
        }

        renderer
            .insert_converted_scene(scene)
            .map_err(|e| std::io::Error::other(e).into())
    }
}

unsafe impl Send for GpuRenderer {}
unsafe impl Sync for GpuRenderer {}
