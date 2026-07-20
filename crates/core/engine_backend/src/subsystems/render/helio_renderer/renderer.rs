//! Main HelioRenderer — wgpu + Helio scene renderer with built-in editor state.

use glam::{EulerRot, Mat4, Quat, Vec3};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use engine_fs::virtual_fs;
use helio::{
    Camera, DebugDrawState, EditorState, GizmoMode, GpuMaterial, GroupMask, LightId, MaterialId,
    MeshId, MeshUpload, Movability, ObjectDescriptor, ObjectId, RenderGraph, Renderer,
    RendererConfig, Scene, SceneActor, SceneActorId, ScenePicker, SkyActor,
};
use pulsar_events::script_registry;
use pulsar_reflection::{
    apply_runtime_behavior_for_class, scene_id_to_tag, ComponentRuntimeContext, LiveKeySet,
    RuntimeComponentOwner, Subsystems,
};
use pulsar_rendering::subsystems::{MeshCache, SceneObjectCache};
use pulsar_scene::{build_transform_parts, component_instances_from_props};

use crate::scene::{ObjectType, SceneObjectSnapshot};

use super::core::{CameraInput, GpuProfilerData, RenderMetrics};

// ── Legacy types (unused but referenced by UI code) ──────────────────────────

#[derive(Debug, Clone)]
pub enum RendererCommand {
    ToggleFeature(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EditorCameraState {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// Delegates to the shared implementation in `pulsar_scene`.
fn build_transform(snap: &SceneObjectSnapshot) -> Mat4 {
    build_transform_parts(snap.position, snap.rotation, snap.scale)
}

use std::sync::atomic::{AtomicBool, Ordering};

// ── HelioRenderer ─────────────────────────────────────────────────────────────

/// Main renderer coordinating Helio 3D rendering with GPUI.
pub struct HelioRenderer {
    // ── Scene & Input ──
    pub camera_input: Arc<Mutex<CameraInput>>,
    pub scene_db: Arc<crate::scene::SceneDb>,

    // ── Legacy (unused) ──
    pub command_sender: mpsc::Sender<RendererCommand>,
    pub command_receiver: mpsc::Receiver<RendererCommand>,

    // ── Pending editor commands (written by UI thread, read by render thread) ──
    /// Next gizmo mode to apply; consumed at start of render_frame.
    pub pending_gizmo_mode: Arc<Mutex<Option<GizmoMode>>>,
    /// When true, the render thread should call editor_state.deselect() next frame.
    pub pending_deselect: Arc<AtomicBool>,

    // ── Renderer State ──
    /// Error messages from mesh loading failures, drained by the UI viewport for notifications.
    pub pending_errors: Arc<Mutex<Vec<String>>>,

    inner: Option<HelioInner>,

    // ── Camera State ──
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    // Smoothed local-space velocity: x=right, y=up, z=forward (units/sec).
    cam_local_velocity: Vec3,
    viewport_size: (u32, u32),

    // ── Metrics ──
    pub metrics: Arc<Mutex<RenderMetrics>>,
    pub gpu_profiler: Arc<Mutex<GpuProfilerData>>,
    last_frame: Instant,
    frame_count: u64,
}

struct HelioInner {
    renderer: Renderer,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    editor_state: EditorState,
    scene_picker: ScenePicker,
    /// Persists GPU-uploaded mesh geometry across frames so components
    /// don't re-load + re-upload the same asset every sync pass.
    mesh_cache: MeshCache,
    /// Tracks scene object instances keyed by tag for incremental
    /// update (avoid cascade-free on clear-all-insert-each-frame).
    object_cache: SceneObjectCache,
}

impl HelioRenderer {
    pub fn new(scene_db: Arc<crate::scene::SceneDb>) -> Self {
        let (command_sender, command_receiver) = mpsc::channel();
        Self {
            camera_input: Arc::new(Mutex::new(CameraInput::new())),
            scene_db,
            command_sender,
            command_receiver,
            pending_gizmo_mode: Arc::new(Mutex::new(None)),
            pending_deselect: Arc::new(AtomicBool::new(false)),
            inner: None,
            pending_errors: Arc::new(Mutex::new(Vec::new())),
            cam_pos: Vec3::new(8.0, 6.0, 12.0),
            cam_yaw: -0.5,
            cam_pitch: -0.3,
            cam_local_velocity: Vec3::ZERO,
            viewport_size: (0, 0),
            metrics: Arc::new(Mutex::new(RenderMetrics::default())),
            gpu_profiler: Arc::new(Mutex::new(GpuProfilerData::default())),
            last_frame: Instant::now(),
            frame_count: 0,
        }
    }

    pub fn editor_camera_state(&self) -> EditorCameraState {
        EditorCameraState {
            position: self.cam_pos.to_array(),
            yaw: self.cam_yaw,
            pitch: self.cam_pitch,
        }
    }

    pub fn set_editor_camera_state(&mut self, state: EditorCameraState) {
        self.cam_pos = Vec3::from_array(state.position);
        self.cam_yaw = state.yaw;
        self.cam_pitch = state.pitch;
        self.cam_local_velocity = Vec3::ZERO;

        if let Ok(mut input) = self.camera_input.lock() {
            input.forward = 0.0;
            input.right = 0.0;
            input.up = 0.0;
            input.clear_transient_deltas();
        }
    }

    /// Called each GPUI frame from the viewport.
    pub fn render_frame(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.frame_count += 1;

        // Lazy init
        if self.inner.is_none() {
            tracing::info!("Initializing Helio renderer...");

            // Clone device/queue from GPUI's WgpuSurface
            let device_arc = Arc::new(_device.clone());
            let queue_arc = Arc::new(_queue.clone());
            let scene = Scene::new(device_arc.clone(), queue_arc.clone());
            let graph = RenderGraph::new_with_external_device(&device_arc, &queue_arc);
            let debug_camera_buffer = device_arc.create_buffer(&wgpu::BufferDescriptor {
                label: Some("debug_camera"),
                size: 64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let cull_stats_buffer = device_arc.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cull_stats"),
                size: 64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let debug_state = Arc::new(Mutex::new(DebugDrawState::default()));
            let mut r = Renderer::new_with_external_device(
                device_arc.clone(),
                queue_arc.clone(),
                format,
                width,
                height,
                1.0,
                RendererConfig::new(width, height, format),
                scene,
                graph,
                debug_state,
                debug_camera_buffer,
                cull_stats_buffer,
            );
            r.set_editor_mode(true);
            r.set_clear_color([0.15, 0.18, 0.25, 1.0]);
            // Keep ambient disabled so scene illumination comes only from explicit light actors.
            r.set_ambient([0.0, 0.0, 0.0], 0.0);

            let mut inner = HelioInner {
                renderer: r,
                device: device_arc,
                queue: queue_arc,
                editor_state: EditorState::new(),
                scene_picker: ScenePicker::new(),
                mesh_cache: MeshCache::new(),
                object_cache: SceneObjectCache::new(),
            };
            self.populate_initial_scene(&mut inner);
            self.inner = Some(inner);
            self.viewport_size = (width, height);

            tracing::info!(
                "[HELIO] Renderer initialized - camera at {:?}, yaw={}, pitch={}",
                self.cam_pos,
                self.cam_yaw,
                self.cam_pitch
            );
        }

        self.apply_camera_input(dt);

        let inner = match self.inner.as_mut() {
            Some(i) => i,
            None => return,
        };

        if self.viewport_size != (width, height) {
            inner.renderer.set_render_size(width, height);
            self.viewport_size = (width, height);
        }

        // Drain pending editor commands written by the UI thread.
        if self.pending_deselect.swap(false, Ordering::AcqRel) {
            inner.editor_state.deselect();
        }
        if let Ok(mut pending) = self.pending_gizmo_mode.lock() {
            if let Some(mode) = pending.take() {
                inner.editor_state.set_gizmo_mode(mode);
            }
        }

        let t_sync = std::time::Instant::now();
        Self::sync_scene(&self.scene_db, inner, &self.pending_errors);
        let sync_ms = t_sync.elapsed().as_secs_f64() * 1000.0;
        if sync_ms > 5.0 {
            tracing::warn!("[SYNC_SCENE] took {:.2}ms — SLOW", sync_ms);
        }

        let t_render = std::time::Instant::now();
        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let fwd = Vec3::new(sy * cp, sp, -cy * cp);
        let aspect = width as f32 / height.max(1) as f32;
        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            10_000.0,
        );

        // Mirror Helio editor demo exactly: clear debug geometry first, then draw gizmos.
        // Without debug_clear(), each frame's gizmo lines accumulate, making it look like
        // multiple objects are selected and leaving drag trails behind moved objects.
        inner.renderer.debug_clear();
        inner.renderer.set_gizmo_camera(&camera, height as f32);
        inner.editor_state.draw_gizmos(&mut inner.renderer);

        let render_ms = t_render.elapsed().as_secs_f64() * 1000.0;
        if render_ms > 5.0 {
            tracing::warn!("[HELIO RENDER] gpu render took {:.2}ms — SLOW", render_ms);
        }
        if let Err(e) = inner.renderer.render(&camera, &view) {
            tracing::error!("Helio render error: {:?}", e);
        }

        if let Ok(mut m) = self.metrics.lock() {
            m.fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
            m.frame_time_ms = dt * 1000.0;
            m.frames_rendered = self.frame_count;
        }
    }

    fn apply_camera_input(&mut self, dt: f32) {
        const LOOK: f32 = 0.0025;
        // Unreal-style movement feel: ease in/out instead of instant velocity changes.
        const ACCEL_RATE: f32 = 10.0;
        const DECEL_RATE: f32 = 14.0;

        let input = match self.camera_input.lock() {
            Ok(mut lock) => {
                let snap = lock.clone();
                lock.clear_transient_deltas();
                snap
            }
            Err(_) => return,
        };

        self.cam_yaw += input.mouse_delta_x * LOOK;
        self.cam_pitch -= input.mouse_delta_y * LOOK;
        self.cam_pitch = self.cam_pitch.clamp(-1.5, 1.5);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let fwd = Vec3::new(sy, 0.0, -cy);
        let right = Vec3::new(cy, 0.0, sy);
        let speed = if input.boost {
            input.move_speed * 3.0
        } else {
            input.move_speed
        };

        // Target local velocity from input (units/sec).
        let target_velocity =
            Vec3::new(input.right * speed, input.up * speed, input.forward * speed);

        // Smooth each local axis independently for responsive but cinematic acceleration.
        let smooth_axis = |current: f32, target: f32| {
            let rate = if target.abs() > current.abs() {
                ACCEL_RATE
            } else {
                DECEL_RATE
            };
            let alpha = 1.0 - (-rate * dt).exp();
            current + (target - current) * alpha
        };

        self.cam_local_velocity.x = smooth_axis(self.cam_local_velocity.x, target_velocity.x);
        self.cam_local_velocity.y = smooth_axis(self.cam_local_velocity.y, target_velocity.y);
        self.cam_local_velocity.z = smooth_axis(self.cam_local_velocity.z, target_velocity.z);

        self.cam_pos += right * self.cam_local_velocity.x * dt;
        self.cam_pos += Vec3::Y * self.cam_local_velocity.y * dt;
        self.cam_pos += fwd * self.cam_local_velocity.z * dt;
    }

    pub fn is_initialized(&self) -> bool {
        self.inner.is_some()
    }

    pub fn get_metrics(&self) -> RenderMetrics {
        self.metrics.lock().map(|m| m.clone()).unwrap_or_default()
    }

    pub fn get_gpu_profiler_data(&self) -> GpuProfilerData {
        self.gpu_profiler
            .lock()
            .map(|m| m.clone())
            .unwrap_or_default()
    }

    // ── Editor Integration ───────────────────────────────────────────────────

    /// Queue a new gizmo mode to be applied at the start of the next render frame.
    pub fn queue_gizmo_mode(&self, mode: crate::GizmoMode) {
        if let Ok(mut guard) = self.pending_gizmo_mode.lock() {
            *guard = Some(mode);
        }
    }

    /// Request that the editor state deselects the current object next frame.
    pub fn queue_deselect(&self) {
        self.pending_deselect
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Get the active gizmo state from SceneDb (gizmo_type, highlighted_axis, etc.).
    pub fn get_scene_gizmo_type(&self) -> crate::scene::GizmoType {
        self.scene_db.get_gizmo_state().gizmo_type
    }

    /// Set the active gizmo type on SceneDb.
    pub fn set_scene_gizmo_type(&self, t: crate::scene::GizmoType) {
        self.scene_db.set_gizmo_type(t);
    }

    /// Return the SceneDb-level selected object ID (set by `select_object_atomic`
    /// on viewport click or by the hierarchy panel).
    pub fn get_scene_db_selected_id(&self) -> Option<String> {
        self.scene_db.get_selected_id()
    }

    // ── Unified per-object scene mutations ────────────────────────────────────
    // These are called directly by SceneDatabase so every write path (user
    // actions, AI tools, content-drawer drops) hits Helio immediately instead
    // of waiting for the next sync_scene() pass.
    //
    // If Helio isn't initialized yet (first frame) the operation returns false
    // and sync_scene() will pick it up on the first ready frame.

    /// Set the gizmo mode (Translate, Rotate, Scale).
    pub fn set_gizmo_mode(&mut self, mode: GizmoMode) {
        if let Some(inner) = &mut self.inner {
            inner.editor_state.set_gizmo_mode(mode);
            tracing::info!("[HELIO] Gizmo mode set to: {:?}", mode);
        }
    }

    /// Get the currently selected object ID (Helio internal ID).
    pub fn get_selected_object(&self) -> Option<helio::SceneActorId> {
        self.inner.as_ref()?.editor_state.selected()
    }

    /// Get the SceneDb ID of the currently selected object.
    pub fn get_selected_scene_db_id(&self) -> Option<String> {
        use helio::SceneActorId;
        let inner = self.inner.as_ref()?;
        let tag = match inner.editor_state.selected()? {
            SceneActorId::Object(obj_id) => inner
                .renderer
                .scene()
                .iter_objects_for_editor()
                .find(|(id, _, _, _)| *id == obj_id)
                .map(|(_, _, _, t)| t)?,
            SceneActorId::Light(light_id) => inner
                .renderer
                .scene()
                .iter_lights()
                .find(|(id, _, _)| *id == light_id)
                .map(|(_, _, t)| t)?,
            _ => return None,
        };
        self.scene_db
            .get_all_snapshots()
            .into_iter()
            .find(|snap| scene_id_to_tag(&snap.id) == tag)
            .map(|snap| snap.id)
    }

    /// Select an object or light by its SceneDb ID.
    pub fn select_by_scene_db_id(&mut self, scene_db_id: &str) -> bool {
        use helio::SceneActorId;
        let Some(inner) = &mut self.inner else {
            return false;
        };
        let tag = scene_id_to_tag(scene_db_id);

        if let Some((obj_id, _, _, _)) = inner
            .renderer
            .scene()
            .iter_objects_for_editor()
            .find(|(_, _, _, t)| *t == tag)
        {
            inner.editor_state.select(SceneActorId::Object(obj_id));
            true
        } else if let Some((light_id, _, _)) = inner
            .renderer
            .scene()
            .iter_lights()
            .find(|(_, _, t)| *t == tag)
        {
            inner.editor_state.select(SceneActorId::Light(light_id));
            true
        } else {
            false
        }
    }

    /// Deselect the currently selected object.
    pub fn deselect(&mut self) {
        if let Some(inner) = &mut self.inner {
            inner.editor_state.deselect();
            tracing::info!("[HELIO] Deselected");
        }
    }

    /// Atomically select an object by SceneDb ID in both SceneDb and Helio EditorState.
    /// This ensures both systems are always in sync without needing a reconciliation loop.
    /// Returns true if the object was found and selected.
    pub fn select_object_atomic(&mut self, scene_db_id: Option<String>) -> bool {
        use helio::SceneActorId;

        // First update SceneDb (single source of truth for object list)
        self.scene_db.select_object(scene_db_id.clone());

        // Then update Helio EditorState (for gizmo rendering)
        let Some(inner) = &mut self.inner else {
            return false;
        };

        if let Some(ref id) = scene_db_id {
            let tag = scene_id_to_tag(id);
            if let Some((obj_id, _, _, _)) = inner
                .renderer
                .scene()
                .iter_objects_for_editor()
                .find(|(_, _, _, t)| *t == tag)
            {
                inner.editor_state.select(SceneActorId::Object(obj_id));
                tracing::info!("[ATOMIC] Selected object: {}", id);
                true
            } else if let Some((light_id, _, _)) = inner
                .renderer
                .scene()
                .iter_lights()
                .find(|(_, _, t)| *t == tag)
            {
                inner.editor_state.select(SceneActorId::Light(light_id));
                tracing::info!("[ATOMIC] Selected light: {}", id);
                true
            } else {
                tracing::warn!("[ATOMIC] Actor not found for scene ID: {}", id);
                false
            }
        } else {
            // Deselect in both
            inner.editor_state.deselect();
            tracing::info!("[ATOMIC] Deselected");
            true
        }
    }

    /// Build a ray from normalized cursor position for object picking.
    /// `norm_x` and `norm_y` are in [0.0, 1.0] relative to the viewport.
    /// This is DPI-agnostic: both GPUI logical coords and physical pixels normalize the same way.
    fn build_pick_ray(&self, norm_x: f32, norm_y: f32) -> (Vec3, Vec3) {
        let (width, height) = self.viewport_size;
        // Convert normalized [0,1] to physical pixel coordinates that ray_from_screen expects.
        let cursor_x = norm_x * width as f32;
        let cursor_y = norm_y * height as f32;
        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let fwd = Vec3::new(sy * cp, sp, -cy * cp);
        let aspect = width as f32 / height.max(1) as f32;
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, aspect, 0.1, 10_000.0);
        let view = Mat4::look_at_rh(self.cam_pos, self.cam_pos + fwd, Vec3::Y);
        let vp_inv = (proj * view).inverse();
        EditorState::ray_from_screen(cursor_x, cursor_y, width as f32, height as f32, vp_inv)
    }

    /// Handle left-click for object selection or gizmo dragging.
    /// `norm_x`/`norm_y` must be in [0.0, 1.0] relative to the viewport area.
    pub fn handle_left_click(&mut self, norm_x: f32, norm_y: f32) {
        use helio::SceneActorId;
        let (ray_o, ray_d) = self.build_pick_ray(norm_x, norm_y);

        // Determine what to select (if anything) by doing raycast and lookup
        let selection_target: Option<Option<String>> = {
            let Some(inner) = &mut self.inner else { return };

            // Try to start gizmo drag first.
            if inner
                .editor_state
                .try_start_drag(ray_o, ray_d, inner.renderer.scene())
            {
                // Gizmo drag started - don't change selection
                None
            } else {
                // No gizmo hit — do object picking.
                if let Some(hit) = inner
                    .scene_picker
                    .cast_ray(inner.renderer.scene(), ray_o, ray_d)
                {
                    match hit.actor_id {
                        SceneActorId::Object(_) | SceneActorId::Light(_) => {
                            // Resolve SceneDb ID by scanning for matching user_tag.
                            let scene_db_id = self
                                .scene_db
                                .get_all_snapshots()
                                .into_iter()
                                .find(|snap| scene_id_to_tag(&snap.id) == hit.user_tag)
                                .map(|snap| snap.id);
                            Some(scene_db_id)
                        }
                        _ => {
                            inner.editor_state.select(hit.actor_id);
                            None
                        }
                    }
                } else {
                    // No hit - deselect
                    Some(None)
                }
            }
        };

        // Now apply the selection atomically (if needed)
        if let Some(target) = selection_target {
            self.select_object_atomic(target);
        }
    }

    /// Handle mouse movement for gizmo hover highlighting and dragging.
    /// `norm_x`/`norm_y` must be in [0.0, 1.0] relative to the viewport area.
    pub fn handle_mouse_move(&mut self, norm_x: f32, norm_y: f32) {
        let (ray_o, ray_d) = self.build_pick_ray(norm_x, norm_y);
        let Some(inner) = &mut self.inner else { return };

        // Mirror demo exactly: update_hover is always called (updates gizmo axis highlighting);
        // update_drag is called additionally when a drag is active.
        inner
            .editor_state
            .update_hover(ray_o, ray_d, &inner.renderer);
        if inner.editor_state.is_dragging() {
            inner
                .editor_state
                .update_drag(ray_o, ray_d, &mut inner.renderer);
        }
    }

    /// Handle left-click release to end gizmo dragging.
    /// If a gizmo drag was active, reads the final transform back from the Helio
    /// scene and writes it to the SceneDb so properties panels stay in sync.
    pub fn handle_left_release(&mut self) {
        let Some(inner) = &mut self.inner else { return };

        // Capture the selected actor before ending the drag so we can read its final state.
        let dragged_actor = if inner.editor_state.is_dragging() {
            inner.editor_state.selected()
        } else {
            None
        };

        inner.editor_state.end_drag();

        // Write the final gizmo position back to SceneDb for whichever actor type was dragged.
        if let Some(actor) = dragged_actor {
            use helio::SceneActorId;
            match actor {
                SceneActorId::Object(obj_id) => {
                    if let Ok(mat) = inner.renderer.scene().get_object_transform(obj_id) {
                        let (scale_v, quat, pos_v) = mat.to_scale_rotation_translation();
                        let (yaw, pitch, roll) = quat.to_euler(EulerRot::YXZ);
                        let tag = inner
                            .renderer
                            .scene()
                            .iter_objects_for_editor()
                            .find(|(id, _, _, _)| *id == obj_id)
                            .map(|(_, _, _, t)| t)
                            .unwrap_or(0);
                        if let Some(scene_id) = self
                            .scene_db
                            .get_all_snapshots()
                            .into_iter()
                            .find(|snap| scene_id_to_tag(&snap.id) == tag)
                            .map(|snap| snap.id)
                        {
                            self.scene_db.apply_transform(
                                &scene_id,
                                [pos_v.x, pos_v.y, pos_v.z],
                                [pitch.to_degrees(), yaw.to_degrees(), roll.to_degrees()],
                                [scale_v.x, scale_v.y, scale_v.z],
                            );
                        }
                    }
                }
                SceneActorId::Light(light_id) => {
                    if let Some(gpu_light) = inner.renderer.scene().get_light(light_id) {
                        let pos = [
                            gpu_light.position_range[0],
                            gpu_light.position_range[1],
                            gpu_light.position_range[2],
                        ];
                        let tag = inner
                            .renderer
                            .scene()
                            .iter_lights()
                            .find(|(id, _, _)| *id == light_id)
                            .map(|(_, _, t)| t)
                            .unwrap_or(0);
                        if let Some(scene_id) = self
                            .scene_db
                            .get_all_snapshots()
                            .into_iter()
                            .find(|snap| scene_id_to_tag(&snap.id) == tag)
                            .map(|snap| snap.id)
                        {
                            self.scene_db.apply_transform(
                                &scene_id,
                                pos,
                                [0.0, 0.0, 0.0],
                                [1.0, 1.0, 1.0],
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        // Rebuild picker BVH after an object may have been moved by a drag.
        if let Some(inner) = &mut self.inner {
            inner.scene_picker.rebuild_instances(inner.renderer.scene());
        }
    }

    // ── Scene Setup ──────────────────────────────────────────────────────────

    fn populate_initial_scene(&self, inner: &mut HelioInner) {
        tracing::info!("[HELIO SCENE] Populating initial scene...");

        // Sky
        inner.renderer.scene_mut().insert_actor(SceneActor::Sky(
            SkyActor::new().with_sky_color([0.5, 0.7, 1.0]),
        ));
        tracing::info!("[HELIO SCENE] Added sky");

        // Lights and meshes are driven exclusively through SceneDb via sync_scene()
        // so that the hierarchy panel and the renderer always show the same state.
        tracing::info!(
            "[HELIO SCENE] Scene population complete (sky only; all objects driven by SceneDb)"
        );
    }

    fn sync_scene(
        scene_db: &crate::scene::SceneDb,
        inner: &mut HelioInner,
        error_queue: &Arc<Mutex<Vec<String>>>,
    ) {
        // component_instances_from_snap now delegates to pulsar_scene's shared impl.
        fn component_instances_from_snap(
            snap: &SceneObjectSnapshot,
        ) -> Vec<(usize, String, serde_json::Value)> {
            component_instances_from_props(&snap.props, snap.component_instances.as_ref())
        }

        struct HelioRuntimeContext<'a> {
            renderer: &'a mut Renderer,
            subsystems: Subsystems,
            error_queue: &'a Arc<Mutex<Vec<String>>>,
            project_root: PathBuf,
        }

        impl<'a> ComponentRuntimeContext for HelioRuntimeContext<'a> {
            fn subsystems_mut(&mut self) -> &mut Subsystems {
                &mut self.subsystems
            }

            fn project_root(&self) -> &std::path::Path {
                &self.project_root
            }

            fn report_error(&mut self, message: String) {
                tracing::error!("{}", message);
                if let Ok(mut eq) = self.error_queue.lock() {
                    eq.push(message);
                }
            }
        }

        // Skip sync while the gizmo is actively dragging.
        if inner.editor_state.is_dragging() {
            return;
        }

        // Clear lights each frame (no cascade-free — lights don't ref count).
        let light_ids: Vec<LightId> = inner
            .renderer
            .scene()
            .iter_lights()
            .map(|(id, _, _)| id)
            .collect();
        for id in light_ids {
            let _ = inner.renderer.scene_mut().remove_light(id);
        }
        // Objects are managed incrementally through SceneObjectCache:
        // components call get_subsystem!(context, SceneObjectCache) to look up
        // existing objects by tag, then either update transforms in-place or
        // insert new ones.  After the sync pass we remove stale entries (those
        // the component system didn't touch this frame).

        // ── Component sync pass ───────────────────────────────────────────────
        let t_snap = std::time::Instant::now();
        let snapshots = scene_db.get_all_snapshots();
        let snap_ms = t_snap.elapsed().as_secs_f64() * 1000.0;
        if snap_ms > 2.0 {
            tracing::warn!("[SYNC_SCENE] get_all_snapshots took {:.2}ms", snap_ms);
        }
        let mut live_keys = LiveKeySet::new();

        for snap in &snapshots {
            if !snap.visible {
                continue;
            }

            let owner = RuntimeComponentOwner {
                scene_object_id: snap.id.as_str(),
                position: snap.position,
                rotation: snap.rotation,
                scale: snap.scale,
                props: &snap.props,
            };

            let component_instances = component_instances_from_snap(snap);
            let mut subsystems = Subsystems::new();
            subsystems.register_ref::<Renderer>(&mut inner.renderer);
            subsystems.register_ref::<MeshCache>(&mut inner.mesh_cache);
            subsystems.register_ref::<SceneObjectCache>(&mut inner.object_cache);
            subsystems.register_ref::<LiveKeySet>(&mut live_keys);
            let project_root = engine_state::get_project_path()
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let mut ctx = HelioRuntimeContext {
                renderer: &mut inner.renderer,
                subsystems,
                error_queue,
                project_root,
            };

            for (component_index, class_name, data) in component_instances {
                let _ = apply_runtime_behavior_for_class(
                    class_name.as_str(),
                    &owner,
                    component_index,
                    &data,
                    &mut ctx,
                );
            }
        }

        // Remove stale scene objects and cache entries (components didn't touch them).
        let stale_ids: Vec<String> = inner
            .object_cache
            .map
            .keys()
            .filter(|id| !live_keys.inner().contains(*id))
            .cloned()
            .collect();
        for scene_id in stale_ids {
            if let Some((obj_id, _)) = inner.object_cache.remove(&scene_id) {
                let _ = inner.renderer.scene_mut().remove_object(obj_id);
            }
        }

        // Cull script registrations for objects no longer in the scene.
        let registry = script_registry();
        registry.write().retain_keys(live_keys.inner());

        // Rebuild scene picker BVH after any insertions or removals.
        let t_picker = std::time::Instant::now();
        inner.scene_picker.rebuild_instances(inner.renderer.scene());
        let picker_ms = t_picker.elapsed().as_secs_f64() * 1000.0;
        if picker_ms > 2.0 {
            tracing::warn!("[SYNC_SCENE] picker rebuild took {:.2}ms", picker_ms);
        }
    }
}
