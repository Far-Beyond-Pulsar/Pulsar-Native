//! GPU renderer service — thin wrapper around HelioRenderer.
//!
//! Initialisation is synchronous and lazy: the Helio renderer itself creates its
//! wgpu resources on the first `render_frame_to_surface` call, once the
//! WgpuSurface is available.

use crate::scene::WorldSceneStore;
use crate::subsystems::render::{
    EditorCameraState, HelioRenderer, RenderMetrics, RenderSpikeLogConfig,
};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

/// Builder for `GpuRenderer`.
pub struct GpuRendererBuilder {
    scene_store: Option<Arc<RwLock<WorldSceneStore>>>,
    #[cfg(feature = "physics")]
    _physics_query: Option<Arc<crate::services::PhysicsQueryService>>,
}

impl GpuRendererBuilder {
    pub fn new(_width: u32, _height: u32) -> Self {
        Self {
            scene_store: None,
            #[cfg(feature = "physics")]
            _physics_query: None,
        }
    }

    pub fn scene_db(mut self, store: Arc<RwLock<WorldSceneStore>>) -> Self {
        self.scene_store = Some(store);
        self
    }

    #[cfg(feature = "physics")]
    pub fn physics(mut self, pq: Arc<crate::services::PhysicsQueryService>) -> Self {
        self._physics_query = Some(pq);
        self
    }

    pub fn build(self) -> GpuRenderer {
        let scene_store = self
            .scene_store
            .unwrap_or_else(|| Arc::new(RwLock::new(WorldSceneStore::new())));
        GpuRenderer {
            helio_renderer: Some(HelioRenderer::new(scene_store)),
            frame_count: 0,
            start_time: Instant::now(),
            fps_window_start: Instant::now(),
            fps_window_frames: 0,
            fps: 0.0,
        }
    }
}

/// GPU renderer — drives Helio through a GPUI `WgpuSurfaceHandle`.
///
/// This is the **only** public interface to the renderer subsystem.
/// Callers must not access `helio_renderer` directly; use the methods below.
pub struct GpuRenderer {
    helio_renderer: Option<HelioRenderer>,
    frame_count: u64,
    start_time: Instant,
    // Rolling-window frame rate. `frame_count / start_time.elapsed()` is a
    // lifetime average: it converges to the mean over the whole process and
    // barely moves in response to a change, which makes it useless for judging
    // whether a pacing or caching change did anything. These three fields
    // recompute the rate over a short window instead.
    fps_window_start: Instant,
    fps_window_frames: u64,
    fps: f32,
}

/// Length of the rolling window used to compute [`GpuRenderer::get_fps`].
/// Short enough to react within a fraction of a second, long enough that the
/// number isn't visibly jittery.
const FPS_WINDOW: std::time::Duration = std::time::Duration::from_millis(500);

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
    ) -> Option<wgpu::SubmissionIndex> {
        let result = if let Some(ref mut r) = self.helio_renderer {
            r.render_frame(device, queue, view, width, height, format)
        } else {
            None
        };
        self.frame_count += 1;

        self.fps_window_frames += 1;
        let window_elapsed = self.fps_window_start.elapsed();
        if window_elapsed >= FPS_WINDOW {
            self.fps = self.fps_window_frames as f32 / window_elapsed.as_secs_f32();
            self.fps_window_start = Instant::now();
            self.fps_window_frames = 0;
        }

        result
    }

    /// Frames per second submitted by the renderer, averaged over the last
    /// [`FPS_WINDOW`].
    ///
    /// Note this counts *renderer* frames — one per `render_frame_to_surface`
    /// call on the Helio render thread — not GPUI UI frames. The two are not
    /// the same: the compositor promotes at most one renderer frame per UI
    /// draw, and UI draws are skipped entirely when nothing is dirty.
    pub fn get_fps(&self) -> f32 {
        if self.fps > 0.0 {
            self.fps
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

    pub fn set_spike_log_config(&mut self, config: RenderSpikeLogConfig) {
        if let Some(renderer) = self.helio_renderer.as_mut() {
            renderer.set_spike_log_config(config);
        }
    }

    pub fn spike_log_config(&self) -> Option<RenderSpikeLogConfig> {
        self.helio_renderer
            .as_ref()
            .map(HelioRenderer::spike_log_config)
    }

    pub fn get_frame_count(&self) -> u64 {
        self.frame_count
    }

    // ── Gizmo and selection API ───────────────────────────────────────────────

    /// Drain all pending mesh-load error messages accumulated since the last call.
    /// Returns the drained messages so the UI can display notifications.
    pub fn drain_pending_errors(&self) -> Vec<String> {
        self.helio_renderer
            .as_ref()
            .and_then(|r| r.pending_errors.lock().ok())
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }

    /// Camera input handle for the viewport input thread.
    pub fn camera_input(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<crate::subsystems::render::CameraInput>>> {
        self.helio_renderer.as_ref().map(|r| r.camera_input.clone())
    }

    pub fn editor_camera_state(&self) -> Option<EditorCameraState> {
        self.helio_renderer
            .as_ref()
            .map(|r| r.editor_camera_state())
    }

    pub fn set_editor_camera_state(&mut self, state: EditorCameraState) {
        if let Some(r) = self.helio_renderer.as_mut() {
            r.set_editor_camera_state(state);
        }
    }

    /// Queue a new Helio gizmo mode for the next render frame.
    pub fn queue_gizmo_mode(&self, mode: crate::GizmoMode) {
        if let Some(r) = &self.helio_renderer {
            r.queue_gizmo_mode(mode);
        }
    }

    /// Request deselection at the start of the next render frame.
    pub fn queue_deselect(&self) {
        if let Some(r) = &self.helio_renderer {
            r.queue_deselect();
        }
    }

    /// Get the current SceneDb-level gizmo type.
    pub fn get_scene_gizmo_type(&self) -> crate::scene::GizmoType {
        self.helio_renderer
            .as_ref()
            .map(|r| r.get_scene_gizmo_type())
            .unwrap_or(crate::scene::GizmoType::None)
    }

    /// Set the SceneDb-level gizmo type.
    pub fn set_scene_gizmo_type(&mut self, t: crate::scene::GizmoType) {
        if let Some(r) = &mut self.helio_renderer {
            r.set_scene_gizmo_type(t);
        }
    }

    /// Get the ID of the currently selected object in SceneDb.
    pub fn get_scene_db_selected_id(&self) -> Option<String> {
        self.helio_renderer
            .as_ref()
            .and_then(|r| r.get_scene_db_selected_id())
    }

    /// Get the ID of the object currently selected inside the Helio editor state
    /// (set by viewport click/gizmo; may lag one frame behind SceneDb selection).
    pub fn get_helio_selected_scene_db_id(&self) -> Option<String> {
        self.helio_renderer
            .as_ref()
            .and_then(|r| r.get_selected_scene_db_id())
    }

    /// Sync the SceneDb selection into Helio's editor state so the gizmo appears.
    pub fn sync_selection_to_helio(&mut self) {
        if let Some(r) = &mut self.helio_renderer {
            let scene_selected = r.get_scene_db_selected_id();
            let helio_selected = r.get_selected_scene_db_id();
            if scene_selected != helio_selected {
                match &scene_selected {
                    Some(id) => {
                        r.select_by_scene_db_id(id);
                    }
                    None => {
                        r.deselect();
                    }
                }
            }
        }
    }

    // ── Mouse / viewport input ────────────────────────────────────────────────

    /// Forward a normalized mouse-move event to the Helio editor state (gizmo drag).
    pub fn handle_mouse_move(&mut self, norm_x: f32, norm_y: f32) {
        if let Some(r) = &mut self.helio_renderer {
            r.handle_mouse_move(norm_x, norm_y);
        }
    }

    /// Forward a normalized left-click to the Helio scene picker.
    pub fn handle_left_click(&mut self, norm_x: f32, norm_y: f32) {
        if let Some(r) = &mut self.helio_renderer {
            r.handle_left_click(norm_x, norm_y);
        }
    }

    /// Signal the end of a left-button drag (finalises gizmo transform).
    pub fn handle_left_release(&mut self) {
        if let Some(r) = &mut self.helio_renderer {
            r.handle_left_release();
        }
    }

    /// Request TAA history reset on the next rendered frame.
    pub fn reset_taa(&mut self) {
        if let Some(r) = &mut self.helio_renderer {
            r.reset_taa();
        }
    }

    /// Send a fire-and-forget command to the renderer thread (e.g. ToggleFeature).
    pub fn send_renderer_command(
        &self,
        cmd: crate::subsystems::render::helio_renderer::RendererCommand,
    ) {
        if let Some(r) = &self.helio_renderer {
            let _ = r.command_sender.send(cmd);
        }
    }
}

unsafe impl Send for GpuRenderer {}
unsafe impl Sync for GpuRenderer {}

#[cfg(test)]
mod tests {
    use super::GpuRendererBuilder;
    use crate::subsystems::render::RenderSpikeLogConfig;
    use std::time::Duration;

    #[test]
    fn spike_log_config_is_available_through_the_public_renderer_service() {
        let mut renderer = GpuRendererBuilder::new(1, 1).build();
        let config = RenderSpikeLogConfig {
            enabled: true,
            cpu_threshold_ms: 18.0,
            gpu_threshold_ms: 12.0,
            min_interval: Duration::from_millis(125),
        };

        renderer.set_spike_log_config(config);

        assert_eq!(renderer.spike_log_config(), Some(config));
    }
}
