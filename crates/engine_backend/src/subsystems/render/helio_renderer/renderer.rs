//! Main HelioRenderer — wgpu + Helio scene renderer with built-in editor state.

use glam::{EulerRot, Mat4, Quat, Vec3};
use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use helio::{
    Camera, EditorState, GizmoMode, GpuLight, GpuMaterial, GroupMask, LightType, MaterialId,
    MeshId, MeshUpload, Movability, ObjectDescriptor, ObjectId, PackedVertex, Renderer,
    RendererConfig, SceneActor, ScenePicker, SkyActor,
};
use helio_asset_compat::ConvertedScene;

use crate::scene::{MeshType, ObjectType, SceneObjectSnapshot};

use super::core::{CameraInput, GpuProfilerData, RenderMetrics};

// ── Legacy types (unused but referenced by UI code) ──────────────────────────

#[derive(Debug, Clone)]
pub enum RendererCommand {
    ToggleFeature(String),
}

// ── Mesh Generation ──────────────────────────────────────────────────────────

fn box_mesh(half_extents: [f32; 3]) -> MeshUpload {
    let e = glam::Vec3::from_array(half_extents);
    let corners = [
        glam::Vec3::new(-e.x, -e.y, e.z),
        glam::Vec3::new(e.x, -e.y, e.z),
        glam::Vec3::new(e.x, e.y, e.z),
        glam::Vec3::new(-e.x, e.y, e.z),
        glam::Vec3::new(-e.x, -e.y, -e.z),
        glam::Vec3::new(e.x, -e.y, -e.z),
        glam::Vec3::new(e.x, e.y, -e.z),
        glam::Vec3::new(-e.x, e.y, -e.z),
    ];
    let faces: [([usize; 4], [f32; 3], [f32; 3]); 6] = [
        ([0, 1, 2, 3], [0., 0., 1.], [1., 0., 0.]),
        ([5, 4, 7, 6], [0., 0., -1.], [-1., 0., 0.]),
        ([4, 0, 3, 7], [-1., 0., 0.], [0., 0., 1.]),
        ([1, 5, 6, 2], [1., 0., 0.], [0., 0., -1.]),
        ([3, 2, 6, 7], [0., 1., 0.], [1., 0., 0.]),
        ([4, 5, 1, 0], [0., -1., 0.], [1., 0., 0.]),
    ];
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (fi, (quad, normal, tangent)) in faces.iter().enumerate() {
        let base = (fi * 4) as u32;
        let uvs = [[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for (i, &ci) in quad.iter().enumerate() {
            vertices.push(PackedVertex::from_components(
                corners[ci].to_array(),
                *normal,
                uvs[i],
                *tangent,
                1.0,
            ));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    MeshUpload { vertices, indices }
}

fn plane_mesh(half_extent: f32) -> MeshUpload {
    let e = half_extent;
    let normal = [0.0, 1.0, 0.0];
    let tangent = [1.0, 0.0, 0.0];
    let positions = [[-e, 0.0, -e], [e, 0.0, -e], [e, 0.0, e], [-e, 0.0, e]];
    let uvs = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let vertices = positions
        .iter()
        .zip(uvs.iter())
        .map(|(p, uv)| PackedVertex::from_components(*p, normal, *uv, tangent, 1.0))
        .collect();
    // Counter-clockwise winding when viewed from above (positive Y)
    MeshUpload {
        vertices,
        indices: vec![0, 2, 1, 0, 3, 2],
    }
}

fn sphere_mesh(radius: f32) -> MeshUpload {
    let center = Vec3::ZERO;
    let lat_steps = 16;
    let lon_steps = 32;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for i in 0..=lat_steps {
        let phi = std::f32::consts::PI * (i as f32 / lat_steps as f32);
        let y = phi.cos();
        let sin_phi = phi.sin();
        for j in 0..=lon_steps {
            let theta = 2.0 * std::f32::consts::PI * (j as f32 / lon_steps as f32);
            let x = sin_phi * theta.cos();
            let z = sin_phi * theta.sin();

            let position = center + Vec3::new(x, y, z) * radius;
            let normal = [x, y, z];
            let uv = [j as f32 / lon_steps as f32, i as f32 / lat_steps as f32];
            let tangent_vec = Vec3::new(-z, 0.0, x).normalize_or_zero();
            let tangent = tangent_vec.to_array();
            vertices.push(PackedVertex::from_components(
                position.to_array(),
                normal,
                uv,
                tangent,
                1.0,
            ));
        }
    }

    for i in 0..lat_steps {
        for j in 0..lon_steps {
            let a = (i * (lon_steps + 1) + j) as u32;
            let b = a + (lon_steps + 1) as u32;
            // CCW winding when viewed from outside (outward normals).
            indices.extend_from_slice(&[a, a + 1, b]);
            indices.extend_from_slice(&[b, a + 1, b + 1]);
        }
    }

    MeshUpload { vertices, indices }
}

fn make_material(base_color: [f32; 4], roughness: f32, metallic: f32) -> GpuMaterial {
    GpuMaterial {
        base_color,
        emissive: [0.0, 0.0, 0.0, 0.0],
        roughness_metallic: [roughness, metallic, 1.5, 0.5],
        tex_base_color: GpuMaterial::NO_TEXTURE,
        tex_normal: GpuMaterial::NO_TEXTURE,
        tex_roughness: GpuMaterial::NO_TEXTURE,
        tex_emissive: GpuMaterial::NO_TEXTURE,
        tex_occlusion: GpuMaterial::NO_TEXTURE,
        workflow: 0,
        flags: 0,
        _pad: 0,
    }
}

fn build_transform(snap: &SceneObjectSnapshot) -> Mat4 {
    let pos = Vec3::from_array(snap.position);
    let rot = snap.rotation;
    let scale = Vec3::from_array(snap.scale);
    let quat = Quat::from_euler(
        EulerRot::YXZ,
        rot[1].to_radians(),
        rot[0].to_radians(),
        rot[2].to_radians(),
    );
    Mat4::from_scale_rotation_translation(scale, quat, pos)
}

// ── Per-mesh-type cache ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum MeshKey {
    Cube,
    Sphere,
    Plane,
    Cylinder,
}

impl From<MeshType> for MeshKey {
    fn from(t: MeshType) -> Self {
        match t {
            MeshType::Cube | MeshType::Custom | MeshType::Cylinder => MeshKey::Cube,
            MeshType::Sphere => MeshKey::Sphere,
            MeshType::Plane => MeshKey::Plane,
        }
    }
}

fn mesh_for_key(key: MeshKey) -> MeshUpload {
    match key {
        MeshKey::Cube | MeshKey::Cylinder => box_mesh([0.5, 0.5, 0.5]),
        MeshKey::Sphere => sphere_mesh(0.5),
        MeshKey::Plane => plane_mesh(5.0),
    }
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
    /// SceneDb id → (helio ObjectId, mesh used)
    object_map: HashMap<String, (ObjectId, MeshId)>,
    /// MeshKey → (MeshId, MaterialId) shared across all objects of that type
    mesh_cache: HashMap<MeshKey, (MeshId, MaterialId)>,
    /// Helio editor state for gizmo management
    editor_state: EditorState,
    /// Scene picker for raycasting object selection
    scene_picker: ScenePicker,
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
            cam_pos: Vec3::new(8.0, 6.0, 12.0), // Better view angle
            cam_yaw: -0.5,                      // Look left a bit
            cam_pitch: -0.3,                    // Look down to see objects
            cam_local_velocity: Vec3::ZERO,
            viewport_size: (0, 0),
            metrics: Arc::new(Mutex::new(RenderMetrics::default())),
            gpu_profiler: Arc::new(Mutex::new(GpuProfilerData::default())),
            last_frame: Instant::now(),
            frame_count: 0,
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
            let mut r = Renderer::new_with_external_device(
                device_arc.clone(),
                queue_arc.clone(),
                RendererConfig::new(width, height, format),
            );
            r.set_editor_mode(true);
            r.set_clear_color([0.15, 0.18, 0.25, 1.0]);
            // Keep ambient disabled so scene illumination comes only from explicit light actors.
            r.set_ambient([0.0, 0.0, 0.0], 0.0);

            let mut inner = HelioInner {
                renderer: r,
                device: device_arc,
                queue: queue_arc,
                object_map: HashMap::new(),
                mesh_cache: HashMap::new(),
                editor_state: EditorState::new(),
                scene_picker: ScenePicker::new(),
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

        Self::sync_scene(&self.scene_db, inner);

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
        inner.editor_state.draw_gizmos(&mut inner.renderer);

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
                lock.mouse_delta_x = 0.0;
                lock.mouse_delta_y = 0.0;
                lock.pan_delta_x = 0.0;
                lock.pan_delta_y = 0.0;
                lock.zoom_delta = 0.0;
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
        let target_velocity = Vec3::new(input.right * speed, input.up * speed, input.forward * speed);

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

    /// Insert a converted scene into the active Helio scene and place it in front of the camera.
    pub fn insert_converted_scene(&mut self, scene: ConvertedScene) -> Result<(), String> {
        let Some(inner) = &mut self.inner else {
            return Err("Renderer not initialized yet".to_string());
        };

        // Compute local-space bounds over whichever geometry representation is present.
        let mut bb_min = Vec3::splat(f32::INFINITY);
        let mut bb_max = Vec3::splat(f32::NEG_INFINITY);
        let mut saw_vertex = false;

        if let Some(sectioned_mesh) = scene.sectioned_mesh.as_ref() {
            for v in &sectioned_mesh.vertices {
                let p = Vec3::from(v.position);
                bb_min = bb_min.min(p);
                bb_max = bb_max.max(p);
                saw_vertex = true;
            }
        } else {
            for mesh in &scene.meshes {
                for v in &mesh.vertices {
                    let p = mesh.node_transform.transform_point3(Vec3::from(v.position));
                    bb_min = bb_min.min(p);
                    bb_max = bb_max.max(p);
                    saw_vertex = true;
                }
            }
        }

        if !saw_vertex {
            return Err("Converted scene contained no vertices".to_string());
        }

        let local_center = (bb_min + bb_max) * 0.5;
        let local_size = bb_max - bb_min;
        let scene_radius = (local_size * 0.5).length().max(0.5);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let camera_forward = Vec3::new(sy * cp, sp, -cy * cp).normalize_or_zero();
        let spawn_pos = self.cam_pos + camera_forward * 8.0;

        let placement_base = Mat4::from_translation(spawn_pos) * Mat4::from_translation(-local_center);

        let default_mat = inner
            .renderer
            .scene_mut()
            .insert_material(make_material([0.82, 0.84, 0.9, 1.0], 0.5, 0.0));

        let mut inserted_any = false;

        if let Some(sectioned_mesh) = scene.sectioned_mesh.as_ref() {
            let converted_vertices: Vec<PackedVertex> = sectioned_mesh
                .vertices
                .iter()
                .map(|v| PackedVertex {
                    position: v.position,
                    bitangent_sign: v.bitangent_sign,
                    tex_coords0: v.tex_coords0,
                    tex_coords1: v.tex_coords1,
                    normal: v.normal,
                    tangent: v.tangent,
                })
                .collect();

            for section in &sectioned_mesh.sections {
                if section.indices.is_empty() {
                    continue;
                }

                let upload = MeshUpload {
                    vertices: converted_vertices.clone(),
                    indices: section.indices.clone(),
                };

                let Some(mesh_id) = inner
                    .renderer
                    .scene_mut()
                    .insert_actor(SceneActor::mesh(upload.clone()))
                    .as_mesh()
                else {
                    continue;
                };

                inner.scene_picker.register_mesh(mesh_id, &upload);

                let _ = inner
                    .renderer
                    .scene_mut()
                    .insert_actor(SceneActor::object(ObjectDescriptor {
                        mesh: mesh_id,
                        material: default_mat,
                        transform: placement_base,
                        bounds: [spawn_pos.x, spawn_pos.y, spawn_pos.z, scene_radius],
                        flags: 0,
                        groups: GroupMask::NONE,
                        movability: Some(Movability::Movable),
                    }));

                inserted_any = true;
            }
        } else {
            for mesh in &scene.meshes {
                if mesh.indices.is_empty() || mesh.vertices.is_empty() {
                    continue;
                }

                let converted_vertices: Vec<PackedVertex> = mesh
                    .vertices
                    .iter()
                    .map(|v| PackedVertex {
                        position: v.position,
                        bitangent_sign: v.bitangent_sign,
                        tex_coords0: v.tex_coords0,
                        tex_coords1: v.tex_coords1,
                        normal: v.normal,
                        tangent: v.tangent,
                    })
                    .collect();

                let upload = MeshUpload {
                    vertices: converted_vertices,
                    indices: mesh.indices.clone(),
                };

                let Some(mesh_id) = inner
                    .renderer
                    .scene_mut()
                    .insert_actor(SceneActor::mesh(upload.clone()))
                    .as_mesh()
                else {
                    continue;
                };

                inner.scene_picker.register_mesh(mesh_id, &upload);

                let transform = placement_base * mesh.node_transform;
                let pos = transform.w_axis.truncate();

                let _ = inner
                    .renderer
                    .scene_mut()
                    .insert_actor(SceneActor::object(ObjectDescriptor {
                        mesh: mesh_id,
                        material: default_mat,
                        transform,
                        bounds: [pos.x, pos.y, pos.z, scene_radius],
                        flags: 0,
                        groups: GroupMask::NONE,
                        movability: Some(Movability::Movable),
                    }));

                inserted_any = true;
            }
        }

        if !inserted_any {
            return Err("Scene sections contained no renderable indices".to_string());
        }

        inner.scene_picker.rebuild_instances(inner.renderer.scene());
        Ok(())
    }

    // ── Editor Integration ───────────────────────────────────────────────────

    /// Set the gizmo mode (Translate, Rotate, Scale).
    pub fn set_gizmo_mode(&mut self, mode: GizmoMode) {
        if let Some(inner) = &mut self.inner {
            inner.editor_state.set_gizmo_mode(mode);
            tracing::info!("[HELIO] Gizmo mode set to: {:?}", mode);
        }
    }

    /// Get the currently selected object ID.
    pub fn get_selected_object(&self) -> Option<helio::SceneActorId> {
        self.inner.as_ref()?.editor_state.selected()
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
        let (ray_o, ray_d) = self.build_pick_ray(norm_x, norm_y);
        let Some(inner) = &mut self.inner else { return };

        // Try to start gizmo drag first.
        if !inner
            .editor_state
            .try_start_drag(ray_o, ray_d, inner.renderer.scene())
        {
            // No gizmo hit — do object picking.
            // The picker is kept warm (rebuild_instances is called after scene mutations);
            // no need to rebuild on every click.
            if let Some(hit) = inner
                .scene_picker
                .cast_ray(inner.renderer.scene(), ray_o, ray_d)
            {
                inner.editor_state.select(hit.actor_id);
                tracing::info!("[HELIO] Selected object: {:?}", hit.actor_id);
            } else {
                inner.editor_state.deselect();
                tracing::info!("[HELIO] Deselected");
            }
        }
    }

    /// Handle mouse movement for gizmo hover highlighting and dragging.
    /// `norm_x`/`norm_y` must be in [0.0, 1.0] relative to the viewport area.
    pub fn handle_mouse_move(&mut self, norm_x: f32, norm_y: f32) {
        let (ray_o, ray_d) = self.build_pick_ray(norm_x, norm_y);
        let Some(inner) = &mut self.inner else { return };

        // Mirror demo exactly: update_hover is always called (updates gizmo axis highlighting);
        // update_drag is called additionally when a drag is active.
        inner.editor_state.update_hover(ray_o, ray_d, inner.renderer.scene());
        if inner.editor_state.is_dragging() {
            inner
                .editor_state
                .update_drag(ray_o, ray_d, inner.renderer.scene_mut());
        }
    }

    /// Handle left-click release to end gizmo dragging.
    pub fn handle_left_release(&mut self) {
        if let Some(inner) = &mut self.inner {
            inner.editor_state.end_drag();
            // Rebuild picker BVH after an object may have been moved by a drag.
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

        // Sun (directional light)
        let sun_dir = Vec3::new(-0.5, -1.0, -0.3).normalize();
        inner
            .renderer
            .scene_mut()
            .insert_actor(SceneActor::light(GpuLight {
                position_range: [0.0, 0.0, 0.0, f32::MAX],
                direction_outer: [sun_dir.x, sun_dir.y, sun_dir.z, 0.0],
                color_intensity: [1.0, 0.95, 0.9, 5.0],
                shadow_index: 0,
                light_type: LightType::Directional as u32,
                inner_angle: 0.0,
                _pad: 0,
            }));
        tracing::info!("[HELIO SCENE] Added directional light");

        // Fill light (softer ambient)
        inner
            .renderer
            .scene_mut()
            .insert_actor(SceneActor::light(GpuLight {
                position_range: [0.0, 10.0, 0.0, 100.0],
                direction_outer: [0.0, -1.0, 0.0, 0.0],
                color_intensity: [0.4, 0.5, 0.7, 2.0],
                shadow_index: u32::MAX,
                light_type: LightType::Point as u32,
                inner_angle: 0.0,
                _pad: 0,
            }));
        tracing::info!("[HELIO SCENE] Added fill light");

        // Ground plane
        let ground_upload = plane_mesh(50.0);
        let ground_mesh = match inner
            .renderer
            .scene_mut()
            .insert_actor(SceneActor::mesh(ground_upload.clone()))
            .as_mesh()
        {
            Some(id) => id,
            None => {
                tracing::error!("[HELIO SCENE] Failed to insert ground mesh");
                return;
            }
        };
        inner
            .scene_picker
            .register_mesh(ground_mesh, &ground_upload);
        tracing::info!("[HELIO SCENE] Ground mesh registered: {:?}", ground_mesh);

        let ground_mat = inner.renderer.scene_mut().insert_material(make_material(
            [0.35, 0.35, 0.35, 1.0],
            0.9,
            0.0,
        ));
        let ground_obj =
            inner
                .renderer
                .scene_mut()
                .insert_actor(SceneActor::object(ObjectDescriptor {
                    mesh: ground_mesh,
                    material: ground_mat,
                    transform: Mat4::IDENTITY,
                    bounds: [0.0, 0.0, 0.0, 50.0],
                    flags: 0,
                    groups: GroupMask::NONE,
                    movability: None, // Ground stays static
                }));
        tracing::info!("[HELIO SCENE] Ground object created: {:?}", ground_obj);

        // Test cubes
        let cube_upload = box_mesh([0.5, 0.5, 0.5]);
        tracing::info!(
            "[HELIO SCENE] Created cube mesh with {} vertices, {} indices",
            cube_upload.vertices.len(),
            cube_upload.indices.len()
        );
        let cube_mesh = match inner
            .renderer
            .scene_mut()
            .insert_actor(SceneActor::mesh(cube_upload.clone()))
            .as_mesh()
        {
            Some(id) => id,
            None => {
                tracing::error!("[HELIO SCENE] Failed to insert cube mesh");
                return;
            }
        };
        // Register cube mesh with picker
        inner.scene_picker.register_mesh(cube_mesh, &cube_upload);
        tracing::info!("[HELIO SCENE] Cube mesh registered: {:?}", cube_mesh);

        let positions_and_colors: &[([f32; 3], [f32; 4])] = &[
            ([0.0, 1.0, 0.0], [0.8, 0.2, 0.2, 1.0]),  // red center
            ([3.0, 1.0, 0.0], [0.2, 0.7, 0.2, 1.0]),  // green right
            ([-3.0, 1.0, 0.0], [0.2, 0.3, 0.9, 1.0]), // blue left
            ([0.0, 1.0, 5.0], [0.9, 0.9, 0.2, 1.0]),  // yellow front
            ([0.0, 1.0, -5.0], [0.8, 0.3, 0.8, 1.0]), // magenta back
        ];

        for (idx, &(pos, color)) in positions_and_colors.iter().enumerate() {
            let mat = inner
                .renderer
                .scene_mut()
                .insert_material(make_material(color, 0.5, 0.1));
            let transform = Mat4::from_translation(Vec3::from_array(pos));
            let obj =
                inner
                    .renderer
                    .scene_mut()
                    .insert_actor(SceneActor::object(ObjectDescriptor {
                        mesh: cube_mesh,
                        material: mat,
                        transform,
                        bounds: [pos[0], pos[1], pos[2], 1.0],
                        flags: 0,
                        groups: GroupMask::NONE,
                        movability: Some(Movability::Movable), // Make cubes movable!
                    }));
            tracing::info!("[HELIO SCENE] Cube #{} at {:?}: {:?}", idx, pos, obj);
        }

        tracing::info!("[HELIO SCENE] Scene population complete!");
    }

    fn sync_scene(scene_db: &crate::scene::SceneDb, inner: &mut HelioInner) {
        let snapshots = scene_db.get_all_snapshots();

        for snap in &snapshots {
            let key = match snap.object_type {
                ObjectType::Mesh(mt) => MeshKey::from(mt),
                _ => continue,
            };
            if !snap.visible {
                continue;
            }

            if let Some(&(obj_id, _)) = inner.object_map.get(&snap.id) {
                // Update transform
                let _ = inner
                    .renderer
                    .scene_mut()
                    .update_object_transform(obj_id, build_transform(snap));
            } else {
                // Insert new object
                let (mesh_id, mat_id) = *inner.mesh_cache.entry(key).or_insert_with(|| {
                    let upload = mesh_for_key(key);
                    let mid = inner
                        .renderer
                        .scene_mut()
                        .insert_actor(SceneActor::mesh(upload.clone()))
                        .as_mesh()
                        .expect("mesh insert");
                    let mat = make_material([0.6, 0.6, 0.65, 1.0], 0.7, 0.0);
                    let matid = inner.renderer.scene_mut().insert_material(mat);
                    (mid, matid)
                });

                let transform = build_transform(snap);
                let radius = Vec3::from_array(snap.scale).length() * 0.5;
                let pos = transform.w_axis.truncate();

                if let Some(obj_id) = inner
                    .renderer
                    .scene_mut()
                    .insert_actor(SceneActor::object(ObjectDescriptor {
                        mesh: mesh_id,
                        material: mat_id,
                        transform,
                        bounds: [pos.x, pos.y, pos.z, radius.max(0.1)],
                        flags: 0,
                        groups: GroupMask::NONE,
                        movability: Some(Movability::Movable),
                    }))
                    .as_object()
                {
                    inner.object_map.insert(snap.id.clone(), (obj_id, mesh_id));
                }
            }
        }
    }
}
