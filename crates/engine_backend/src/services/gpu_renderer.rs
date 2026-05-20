//! GPU renderer service — thin wrapper around HelioRenderer.
//!
//! Initialisation is synchronous and lazy: the Helio renderer itself creates its
//! wgpu resources on the first `render_frame_to_surface` call, once the
//! WgpuSurface is available.

use crate::scene::SceneDb;
use crate::subsystems::render::{EditorCameraState, HelioRenderer, RenderMetrics};
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
///
/// This is the **only** public interface to the renderer subsystem.
/// Callers must not access `helio_renderer` directly; use the methods below.
pub struct GpuRenderer {
    helio_renderer: Option<HelioRenderer>,
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
        self.helio_renderer.as_ref().map(|r| r.editor_camera_state())
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

    /// Send a fire-and-forget command to the renderer thread (e.g. ToggleFeature).
    pub fn send_renderer_command(
        &self,
        cmd: crate::subsystems::render::helio_renderer::RendererCommand,
    ) {
        if let Some(r) = &self.helio_renderer {
            let _ = r.command_sender.send(cmd);
        }
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
