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

        let placement_base =
            Mat4::from_translation(spawn_pos) * Mat4::from_translation(-local_center);

        let default_mat = inner.renderer.scene_mut().insert_material(make_material(
            [0.82, 0.84, 0.9, 1.0],
            0.5,
            0.0,
        ));

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

                let _ =
                    inner
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

                let _ =
                    inner
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

    /// Get the currently selected object ID (Helio internal ID).
    pub fn get_selected_object(&self) -> Option<helio::SceneActorId> {
        self.inner.as_ref()?.editor_state.selected()
    }

    /// Get the SceneDb ID of the currently selected object.
    pub fn get_selected_scene_db_id(&self) -> Option<String> {
        use helio::SceneActorId;
        let selected = self.inner.as_ref()?.editor_state.selected()?;

        // Extract ObjectId from SceneActorId
        let helio_obj_id = match selected {
            SceneActorId::Object(obj_id) => obj_id,
            _ => return None, // Not an object (maybe a light or camera)
        };

        // Reverse lookup in object_map
        for (scene_db_id, (obj_id, _)) in &self.inner.as_ref()?.object_map {
            if *obj_id == helio_obj_id {
                return Some(scene_db_id.clone());
            }
        }
        None
    }

    /// Select an object by its SceneDb ID.
    /// Returns true if the object was found and selected, false otherwise.
    pub fn select_by_scene_db_id(&mut self, scene_db_id: &str) -> bool {
        use helio::SceneActorId;
        let Some(inner) = &mut self.inner else {
            return false;
        };

        // Look up the Helio ObjectId from the scene_db_id
        if let Some((helio_obj_id, _)) = inner.object_map.get(scene_db_id) {
            inner
                .editor_state
                .select(SceneActorId::Object(*helio_obj_id));
            tracing::info!(
                "[HELIO] Selected object by SceneDb ID: {} -> {:?}",
                scene_db_id,
                helio_obj_id
            );
            true
        } else {
            tracing::warn!(
                "[HELIO] Failed to select object by SceneDb ID: {} (not found in object_map)",
                scene_db_id
            );
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
            // Look up the Helio ObjectId from the scene_db_id
            if let Some((helio_obj_id, _)) = inner.object_map.get(id) {
                inner.editor_state.select(SceneActorId::Object(*helio_obj_id));
                tracing::info!("[ATOMIC] Selected object: {} -> {:?}", id, helio_obj_id);
                true
            } else {
                tracing::warn!("[ATOMIC] Object not found in object_map: {}", id);
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
                    // Extract Helio ObjectId from SceneActorId
                    if let SceneActorId::Object(helio_obj_id) = hit.actor_id {
                        // Look up SceneDb ID from object_map
                        if let Some((scene_db_id, _)) = inner
                            .object_map
                            .iter()
                            .find(|(_, (obj_id, _))| *obj_id == helio_obj_id)
                            .map(|(id, ids)| (id.clone(), ids))
                        {
                            // Found object - select it atomically
                            Some(Some(scene_db_id))
                        } else {
                            // Object not in map (shouldn't happen), just select in Helio
                            inner.editor_state.select(hit.actor_id);
                            tracing::warn!(
                                "[VIEWPORT] Selected object not in object_map: {:?}",
                                helio_obj_id
                            );
                            None
                        }
                    } else {
                        // Not an object (light, camera, etc.) - just select in Helio
                        inner.editor_state.select(hit.actor_id);
                        tracing::info!("[VIEWPORT] Selected non-object actor: {:?}", hit.actor_id);
                        None
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
            .update_hover(ray_o, ray_d, inner.renderer.scene());
        if inner.editor_state.is_dragging() {
            inner
                .editor_state
                .update_drag(ray_o, ray_d, inner.renderer.scene_mut());
        }
    }

    /// Handle left-click release to end gizmo dragging.
    /// If a gizmo drag was active, reads the final transform back from the Helio
    /// scene and writes it to the SceneDb so properties panels stay in sync.
    pub fn handle_left_release(&mut self) {
        let Some(inner) = &mut self.inner else { return };

        // Capture the selected object before ending the drag so we can read its transform.
        let dragged_object: Option<helio::ObjectId> = if inner.editor_state.is_dragging() {
            inner.editor_state.selected_object()
        } else {
            None
        };

        inner.editor_state.end_drag();

        // Write the final gizmo transform back to the SceneDb.
        if let Some(obj_id) = dragged_object {
            if let Ok(mat) = inner.renderer.scene().get_object_transform(obj_id) {
                // Decompose Mat4 → (translation, rotation_euler_yxz_degrees, scale).
                let (scale_v, quat, pos_v) = mat.to_scale_rotation_translation();
                let (yaw, pitch, roll) = quat.to_euler(EulerRot::YXZ);
                let pos = [pos_v.x, pos_v.y, pos_v.z];
                let rot = [pitch.to_degrees(), yaw.to_degrees(), roll.to_degrees()];
                let scale = [scale_v.x, scale_v.y, scale_v.z];

                // Find the SceneDb id for this Helio ObjectId.
                if let Some(scene_id) = inner
                    .object_map
                    .iter()
                    .find(|(_, &(oid, _))| oid == obj_id)
                    .map(|(id, _)| id.clone())
                {
                    self.scene_db.apply_transform(&scene_id, pos, rot, scale);
                    tracing::debug!(
                        "[HELIO] Wrote gizmo transform back to SceneDb: {} pos={:?} rot={:?}",
                        scene_id,
                        pos,
                        rot
                    );
                }
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

        // Three spaced point lights: blue, red, and yellow.
        inner
            .renderer
            .scene_mut()
            .insert_actor(SceneActor::light(GpuLight {
                position_range: [-8.0, 6.0, -6.0, 100.0],
                direction_outer: [0.0, 0.0, 0.0, 0.0],
                color_intensity: [0.2, 0.45, 1.0, 7.0],
                shadow_index: u32::MAX,
                light_type: LightType::Point as u32,
                inner_angle: 0.0,
                _pad: 0,
            }));
        tracing::info!("[HELIO SCENE] Added blue point light");

        inner
            .renderer
            .scene_mut()
            .insert_actor(SceneActor::light(GpuLight {
                position_range: [8.0, 6.0, -6.0, 100.0],
                direction_outer: [0.0, 0.0, 0.0, 0.0],
                color_intensity: [1.0, 0.22, 0.2, 7.0],
                shadow_index: u32::MAX,
                light_type: LightType::Point as u32,
                inner_angle: 0.0,
                _pad: 0,
            }));
        tracing::info!("[HELIO SCENE] Added red point light");

        inner
            .renderer
            .scene_mut()
            .insert_actor(SceneActor::light(GpuLight {
                position_range: [0.0, 7.0, 8.0, 100.0],
                direction_outer: [0.0, 0.0, 0.0, 0.0],
                color_intensity: [1.0, 0.9, 0.2, 6.0],
                shadow_index: u32::MAX,
                light_type: LightType::Point as u32,
                inner_angle: 0.0,
                _pad: 0,
            }));
        tracing::info!("[HELIO SCENE] Added yellow point light");

        // Mesh objects (ground, cubes, etc.) are driven exclusively through SceneDb
        // via sync_scene() so that the hierarchy panel and the renderer always show
        // the same state.  Nothing is hardcoded here.
        tracing::info!("[HELIO SCENE] Scene population complete (sky + 3 colored point lights; meshes driven by SceneDb)");
    }

    fn sync_scene(scene_db: &crate::scene::SceneDb, inner: &mut HelioInner) {
        let snapshots = scene_db.get_all_snapshots();
        let mut picker_dirty = false;

        // While a gizmo drag is active the selected object's transform is owned
        // by Helio — don't overwrite it from the SceneDb this frame.
        let gizmo_dragging = inner.editor_state.is_dragging();
        let dragged_obj_id: Option<ObjectId> = if gizmo_dragging {
            inner.editor_state.selected_object()
        } else {
            None
        };

        for snap in &snapshots {
            let key = match snap.object_type {
                ObjectType::Mesh(mt) => MeshKey::from(mt),
                _ => continue,
            };
            if !snap.visible {
                continue;
            }

            if let Some(&(obj_id, _)) = inner.object_map.get(&snap.id) {
                // Skip transform update while the gizmo is dragging this object —
                // the transform is owned by Helio until the drag ends.
                let skip = dragged_obj_id.map_or(false, |did| did == obj_id);
                if !skip {
                    let _ = inner
                        .renderer
                        .scene_mut()
                        .update_object_transform(obj_id, build_transform(snap));
                }
            } else {
                // Insert new object — track whether this is a fresh mesh type so
                // we can register it with the scene picker afterward.
                let is_new_mesh_type = !inner.mesh_cache.contains_key(&key);
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

                // Register the mesh with the picker the first time it is seen.
                if is_new_mesh_type {
                    let upload = mesh_for_key(key);
                    inner.scene_picker.register_mesh(mesh_id, &upload);
                }

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
                    picker_dirty = true;
                }
            }
        }

        // Rebuild the BVH once per sync if any new objects were inserted.
        if picker_dirty {
            inner.scene_picker.rebuild_instances(inner.renderer.scene());
        }
    }
}
