// GPU Renderer — thin wrapper around HelioRenderer v2 (helio-render-v2 + WGPUI surface)

use crate::subsystems::render::{HelioRenderer, RenderMetrics, GpuProfilerData, CameraInput};
use crate::scene::SceneDb;
use gpui::WgpuSurfaceHandle;
use std::sync::{Arc, Mutex, Once};
use std::time::Instant;

/// Simple framebuffer structure kept for API compatibility (no longer used for rendering).
pub struct ViewportFramebuffer {
    pub buffer: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub generation: u64,
}

static INIT: Once = Once::new();
static mut RUNTIME: Option<tokio::runtime::Runtime> = None;

fn get_runtime() -> &'static tokio::runtime::Runtime {
    unsafe {
        INIT.call_once(|| {
            RUNTIME = Some(
                tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime")
            );
        });
        RUNTIME.as_ref().unwrap()
    }
}

/// Builder for `GpuRenderer`.
///
/// ```rust,ignore
/// let renderer = GpuRendererBuilder::new(1920, 1080)
///     .scene_db(scene_db)
///     .surface(surface_handle)
///     .build();
/// ```
pub struct GpuRendererBuilder {
    display_width: u32,
    display_height: u32,
    scene_db: Option<Arc<SceneDb>>,
    game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
    physics_query: Option<Arc<crate::services::PhysicsQueryService>>,
    surface_handle: Option<WgpuSurfaceHandle>,
}

impl GpuRendererBuilder {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            display_width: width,
            display_height: height,
            scene_db: None,
            game_thread_state: None,
            physics_query: None,
            surface_handle: None,
        }
    }

    pub fn scene_db(mut self, db: Arc<SceneDb>) -> Self {
        self.scene_db = Some(db);
        self
    }

    pub fn game_thread(mut self, gt: Arc<Mutex<crate::subsystems::game::GameState>>) -> Self {
        self.game_thread_state = Some(gt);
        self
    }

    pub fn physics(mut self, pq: Arc<crate::services::PhysicsQueryService>) -> Self {
        self.physics_query = Some(pq);
        self
    }

    /// Provide the WGPUI surface handle. If not provided the renderer thread will
    /// spin until one is supplied via `GpuRenderer::set_surface_handle`.
    pub fn surface(mut self, handle: WgpuSurfaceHandle) -> Self {
        self.surface_handle = Some(handle);
        self
    }

    pub fn build(self) -> GpuRenderer {
        let width = self.display_width;
        let height = self.display_height;
        let scene_db = self.scene_db.unwrap_or_else(|| Arc::new(SceneDb::new()));
        let game_thread_state = self.game_thread_state;
        let physics_query = self.physics_query;
        let surface_handle = self.surface_handle;

        tracing::info!("[GPU-RENDERER] 🚀 Initializing Helio v2 renderer at {}x{}", width, height);

        let pending_surface: Arc<Mutex<Option<WgpuSurfaceHandle>>> =
            Arc::new(Mutex::new(surface_handle));

        let pending_clone = pending_surface.clone();
        let scene_db_clone = scene_db.clone();

        let runtime = get_runtime();
        let helio_renderer = runtime.block_on(async move {
            // Wait up to 30 s for a surface to appear
            let surface = {
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(30);
                loop {
                    {
                        let mut guard = pending_clone.lock().unwrap();
                        if let Some(h) = guard.take() {
                            break h;
                        }
                    }
                    if tokio::time::Instant::now() > deadline {
                        tracing::error!("[GPU-RENDERER] ❌ Timed out waiting for WgpuSurfaceHandle");
                        return None;
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(16)).await;
                }
            };

            tracing::info!("[GPU-RENDERER] ✅ WgpuSurfaceHandle received, creating renderer");
            Some(HelioRenderer::new_with_all(
                width, height, surface,
                game_thread_state, scene_db_clone, physics_query,
            ).await)
        });

        GpuRenderer {
            helio_renderer,
            pending_surface,
            render_width: width,
            render_height: height,
            frame_count: 0,
            start_time: Instant::now(),
            last_metrics_print: Instant::now(),
        }
    }
}

/// GPU Renderer — proxy to the HelioRenderer v2 render thread.
pub struct GpuRenderer {
    pub helio_renderer: Option<HelioRenderer>,
    /// Receives a WgpuSurfaceHandle after construction when needed.
    pending_surface: Arc<Mutex<Option<WgpuSurfaceHandle>>>,
    render_width: u32,
    render_height: u32,
    frame_count: u64,
    start_time: Instant,
    last_metrics_print: Instant,
}

impl GpuRenderer {
    /// Supply the surface handle after construction (used when `panel.rs` calls
    /// `window.create_wgpu_surface()` and then needs to hand the handle to the renderer).
    pub fn set_surface_handle(&self, handle: WgpuSurfaceHandle) {
        if let Ok(mut guard) = self.pending_surface.lock() {
            *guard = Some(handle);
        }
    }

    /// Tick — logs metrics periodically.
    pub fn render(&mut self, _framebuffer: &mut ViewportFramebuffer) {
        self.frame_count += 1;

        if let Some(ref renderer) = self.helio_renderer {
            if self.last_metrics_print.elapsed().as_secs() >= 5 {
                let m = renderer.get_metrics();
                tracing::info!(
                    "[GPU-RENDERER] FPS: {:.1}  frame: {:.2}ms  total: {}",
                    m.fps, m.frame_time_ms, m.frames_rendered
                );
                self.last_metrics_print = Instant::now();
            }
        }
    }

    pub fn get_frame_count(&self) -> u64 {
        self.frame_count
    }

    pub fn get_fps(&self) -> f32 {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        if elapsed > 0.0 { self.frame_count as f32 / elapsed } else { 0.0 }
    }

    pub fn get_helio_fps(&self) -> f32 {
        self.helio_renderer.as_ref().map_or(0.0, |r| r.get_metrics().fps)
    }

    pub fn get_render_metrics(&self) -> Option<RenderMetrics> {
        self.helio_renderer.as_ref().map(|r| r.get_metrics())
    }

    pub fn get_gpu_profiler_data(&self) -> Option<GpuProfilerData> {
        self.helio_renderer.as_ref().map(|r| r.get_gpu_profiler_data())
    }

    /// Returns the last GPU pipeline execution time in microseconds.
    /// With helio-render-v2, detailed pipeline timing comes from `GpuProfilerData`;
    /// this convenience method returns the total frame time converted to µs.
    pub fn get_pipeline_time_us(&self) -> u64 {
        self.helio_renderer
            .as_ref()
            .map(|r| {
                let m = r.get_metrics();
                (m.frame_time_ms * 1000.0) as u64
            })
            .unwrap_or(0)
    }

    pub fn update_camera_input(&mut self, input: CameraInput) {
        if let Some(ref renderer) = self.helio_renderer {
            if let Ok(mut cam) = renderer.camera_input.lock() {
                *cam = input;
            }
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.render_width  = width;
        self.render_height = height;
        tracing::info!("[GPU-RENDERER] Resize to {}x{}", width, height);
        // Surface handle resize is handled by the WgpuSurface element in WGPUI
    }
}

unsafe impl Send for GpuRenderer {}
unsafe impl Sync for GpuRenderer {}