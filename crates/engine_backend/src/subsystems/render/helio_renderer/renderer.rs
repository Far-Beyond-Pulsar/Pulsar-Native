//! Main HelioRenderer — wgpu + Helio scene renderer with built-in editor state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;
use glam::{EulerRot, Mat4, Quat, Vec2, Vec3};

use helio::{
    Camera, GroupMask, GpuLight, GpuMaterial, LightType,
    MaterialId, MeshId, MeshUpload, Movability, ObjectDescriptor, ObjectId,
    PackedVertex, Renderer, RendererConfig, SceneActor, SkyActor,
};

use crate::scene::{GizmoState, MeshType, ObjectType, SceneObjectSnapshot};

use super::core::{CameraInput, RenderMetrics, GpuProfilerData};

pub mod gizmo_types {
    /// Normalized viewport bounds used by level editor mouse input.
    #[derive(Debug, Clone)]
    pub struct ViewportBounds {
        pub x: f32,
        pub y: f32,
        pub width: f32,
        pub height: f32,
    }
}

#[derive(Debug, Clone)]
pub enum RendererCommand {
    ToggleFeature(String),
}

#[derive(Debug, Clone)]
pub struct ViewportMouseInput {
    pub left_clicked: bool,
    pub left_down: bool,
    pub mouse_pos: glam::Vec2,
    pub mouse_delta: glam::Vec2,
    pub viewport_bounds: Option<gizmo_types::ViewportBounds>,
}

impl Default for ViewportMouseInput {
    fn default() -> Self {
        Self {
            left_clicked: false,
            left_down: false,
            mouse_pos: glam::Vec2::ZERO,
            mouse_delta: glam::Vec2::ZERO,
            viewport_bounds: None,
        }
    }
}

// ── Mesh helpers ─────────────────────────────────────────────────────────────

fn box_mesh(half_extents: [f32; 3]) -> MeshUpload {
    let e = glam::Vec3::from_array(half_extents);
    let corners = [
        glam::Vec3::new(-e.x, -e.y,  e.z), glam::Vec3::new( e.x, -e.y,  e.z),
        glam::Vec3::new( e.x,  e.y,  e.z), glam::Vec3::new(-e.x,  e.y,  e.z),
        glam::Vec3::new(-e.x, -e.y, -e.z), glam::Vec3::new( e.x, -e.y, -e.z),
        glam::Vec3::new( e.x,  e.y, -e.z), glam::Vec3::new(-e.x,  e.y, -e.z),
    ];
    let faces: [([usize; 4], [f32; 3], [f32; 3]); 6] = [
        ([0,1,2,3], [0.,0.,1.],  [1.,0.,0.]),
        ([5,4,7,6], [0.,0.,-1.], [-1.,0.,0.]),
        ([4,0,3,7], [-1.,0.,0.], [0.,0.,1.]),
        ([1,5,6,2], [1.,0.,0.],  [0.,0.,-1.]),
        ([3,2,6,7], [0.,1.,0.],  [1.,0.,0.]),
        ([4,5,1,0], [0.,-1.,0.], [1.,0.,0.]),
    ];
    let mut vertices = Vec::with_capacity(24);
    let mut indices  = Vec::with_capacity(36);
    for (fi, (quad, normal, tangent)) in faces.iter().enumerate() {
        let base = (fi * 4) as u32;
        let uvs = [[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for (i, &ci) in quad.iter().enumerate() {
            vertices.push(PackedVertex::from_components(
                corners[ci].to_array(), *normal, uvs[i], *tangent, 1.0,
            ));
        }
        indices.extend_from_slice(&[base, base+1, base+2, base, base+2, base+3]);
    }
    MeshUpload { vertices, indices }
}

fn sphere_mesh(radius: f32, stacks: u32, slices: u32) -> MeshUpload {
    let mut vertices = Vec::new();
    let mut indices  = Vec::new();
    for s in 0..=stacks {
        let phi = std::f32::consts::PI * s as f32 / stacks as f32;
        let (sp, cp) = phi.sin_cos();
        for sl in 0..=slices {
            let theta = 2.0 * std::f32::consts::PI * sl as f32 / slices as f32;
            let (st, ct) = theta.sin_cos();
            let n = [sp * ct, cp, sp * st];
            let p = [n[0] * radius, n[1] * radius, n[2] * radius];
            let uv = [sl as f32 / slices as f32, s as f32 / stacks as f32];
            let t = [-st, 0.0, ct];
            vertices.push(PackedVertex::from_components(p, n, uv, t, 1.0));
        }
    }
    for s in 0..stacks {
        for sl in 0..slices {
            let a = s * (slices + 1) + sl;
            let b = a + slices + 1;
            indices.extend_from_slice(&[a, b, a+1, b, b+1, a+1]);
        }
    }
    MeshUpload { vertices, indices }
}

fn plane_mesh(half_extent: f32) -> MeshUpload {
    let e = half_extent;
    let normal  = [0.0, 1.0, 0.0];
    let tangent = [1.0, 0.0, 0.0];
    let positions = [
        [-e, 0.0, -e], [ e, 0.0, -e], [ e, 0.0,  e], [-e, 0.0,  e],
    ];
    let uvs = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let vertices = positions.iter().zip(uvs.iter())
        .map(|(p, uv)| PackedVertex::from_components(*p, normal, *uv, tangent, 1.0))
        .collect();
    MeshUpload { vertices, indices: vec![0, 1, 2, 0, 2, 3] }
}

fn make_material(base_color: [f32; 4], roughness: f32, metallic: f32) -> GpuMaterial {
    GpuMaterial {
        base_color,
        emissive: [0.0, 0.0, 0.0, 0.0],
        roughness_metallic: [roughness, metallic, 1.5, 0.5],
        tex_base_color: GpuMaterial::NO_TEXTURE,
        tex_normal:     GpuMaterial::NO_TEXTURE,
        tex_roughness:  GpuMaterial::NO_TEXTURE,
        tex_emissive:   GpuMaterial::NO_TEXTURE,
        tex_occlusion:  GpuMaterial::NO_TEXTURE,
        workflow: 0,
        flags: 0,
        _pad: 0,
    }
}

fn build_transform(snap: &SceneObjectSnapshot) -> Mat4 {
    let pos   = Vec3::from_array(snap.position);
    let rot   = snap.rotation;
    let scale = Vec3::from_array(snap.scale);
    let quat  = Quat::from_euler(EulerRot::YXZ,
        rot[1].to_radians(), rot[0].to_radians(), rot[2].to_radians());
    Mat4::from_scale_rotation_translation(scale, quat, pos)
}

// ── Per-mesh-type cache ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum MeshKey { Cube, Sphere, Plane, Cylinder }

impl From<MeshType> for MeshKey {
    fn from(t: MeshType) -> Self {
        match t {
            MeshType::Cube | MeshType::Custom => MeshKey::Cube,
            MeshType::Sphere                  => MeshKey::Sphere,
            MeshType::Plane                   => MeshKey::Plane,
            MeshType::Cylinder                => MeshKey::Cylinder,
        }
    }
}

fn mesh_for_key(key: MeshKey) -> MeshUpload {
    match key {
        MeshKey::Cube | MeshKey::Cylinder => box_mesh([0.5, 0.5, 0.5]),
        MeshKey::Sphere                   => sphere_mesh(0.5, 16, 16),
        MeshKey::Plane                    => plane_mesh(5.0),
    }
}

// ── HelioRenderer ─────────────────────────────────────────────────────────────

/// Shared camera input written by the dedicated input thread.
pub struct HelioRenderer {
    /// Written by the input thread, read each frame in `render_frame`.
    pub camera_input: Arc<Mutex<CameraInput>>,
    /// Shared scene database owned by the UI and level editor.
    pub scene_db: Arc<crate::scene::SceneDb>,

    pub command_sender: mpsc::Sender<RendererCommand>,
    pub command_receiver: mpsc::Receiver<RendererCommand>,
    pub viewport_mouse_input: Arc<Mutex<ViewportMouseInput>>,
    pub gizmo_state: Arc<Mutex<GizmoState>>,

    // Lazily initialised on the first `render_frame` call.
    inner: Option<HelioInner>,

    // Camera state driven by `camera_input` each frame.
    cam_pos:   Vec3,
    cam_yaw:   f32,
    cam_pitch: f32,

    // Viewport-local cursor position set from GPUI events.
    cursor_pos: (f32, f32),
    viewport_size: (u32, u32),

    pub metrics:     Arc<Mutex<RenderMetrics>>,
    pub gpu_profiler: Arc<Mutex<GpuProfilerData>>,
    last_frame:  Instant,
    frame_count: u64,
}

struct HelioInner {
    renderer: Renderer,
    /// SceneDb id → (helio ObjectId, mesh used)
    object_map: HashMap<String, (ObjectId, MeshId)>,
    /// MeshKey → (MeshId, MaterialId) shared across all objects of that type
    mesh_cache: HashMap<MeshKey, (MeshId, MaterialId)>,
}

impl HelioRenderer {
    pub fn new(scene_db: Arc<crate::scene::SceneDb>) -> Self {
        let (command_sender, command_receiver) = mpsc::channel();
        Self {
            camera_input: Arc::new(Mutex::new(CameraInput::new())),
            scene_db,
            command_sender,
            command_receiver,
            viewport_mouse_input: Arc::new(Mutex::new(ViewportMouseInput::default())),
            gizmo_state: Arc::new(Mutex::new(GizmoState::default())),
            inner: None,
            cam_pos:   Vec3::new(0.0, 5.0, 15.0),
            cam_yaw:   0.0,
            cam_pitch: -0.2,
            cursor_pos: (0.0, 0.0),
            viewport_size: (0, 0),
            metrics:      Arc::new(Mutex::new(RenderMetrics::default())),
            gpu_profiler: Arc::new(Mutex::new(GpuProfilerData::default())),
            last_frame:  Instant::now(),
            frame_count: 0,
        }
    }

    pub fn get_read_index(&self) -> usize {
        0
    }

    /// Called each GPUI frame from the viewport.
    pub fn render_frame(
        &mut self,
        device: &wgpu::Device,
        queue:  &wgpu::Queue,
        view:   &wgpu::TextureView,
        width:  u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) {
        let now = Instant::now();
        let dt  = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame   = now;
        self.frame_count += 1;
        self.viewport_size = (width, height);

        // Lazy init
        if self.inner.is_none() {
            let device = Arc::new(device.clone());
            let queue  = Arc::new(queue.clone());
            // GPUI owns the wgpu device/queue, so Helio must use the
            // external-device path (same as working wgpu_surface examples).
            let mut r  = Renderer::new_with_external_device(
                device,
                queue,
                RendererConfig::new(width, height, format),
            );
            r.set_editor_mode(true);
            r.set_clear_color([0.15, 0.18, 0.25, 1.0]);
            r.set_ambient([0.4, 0.45, 0.55], 0.6);

            let mut inner = HelioInner {
                renderer:   r,
                object_map: HashMap::new(),
                mesh_cache: HashMap::new(),
            };
            self.populate_initial_scene(&mut inner);
            self.inner = Some(inner);
        }

        self.apply_camera_input(dt);

        let inner = match self.inner.as_mut() {
            Some(i) => i,
            None    => return,
        };

        if self.viewport_size.0 != width || self.viewport_size.1 != height {
            inner.renderer.set_render_size(width, height);
        }

        Self::sync_scene(&self.scene_db, inner);

        inner.renderer.debug_clear();

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let fwd = Vec3::new(sy * cp, sp, -cy * cp);
        let aspect = width as f32 / height.max(1) as f32;
        let camera = Camera::perspective_look_at(
            self.cam_pos, self.cam_pos + fwd, Vec3::Y,
            std::f32::consts::FRAC_PI_4, aspect, 0.1, 10_000.0,
        );

        if let Err(e) = inner.renderer.render(&camera, view) {
            tracing::error!("[HELIO] render error: {:?}", e);
        }

        if let Ok(mut m) = self.metrics.lock() {
            m.fps            = if dt > 0.0 { 1.0 / dt } else { 0.0 };
            m.frame_time_ms  = dt * 1000.0;
            m.frames_rendered = self.frame_count;
        }
    }

    fn apply_camera_input(&mut self, dt: f32) {
        const LOOK: f32 = 0.0025;

        let input = match self.camera_input.lock() {
            Ok(mut lock) => {
                let snap = lock.clone();
                lock.mouse_delta_x = 0.0;
                lock.mouse_delta_y = 0.0;
                lock.pan_delta_x   = 0.0;
                lock.pan_delta_y   = 0.0;
                lock.zoom_delta    = 0.0;
                snap
            }
            Err(_) => return,
        };

        self.cam_yaw   += input.mouse_delta_x * LOOK;
        self.cam_pitch -= input.mouse_delta_y * LOOK;
        self.cam_pitch  = self.cam_pitch.clamp(-1.5, 1.5);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let fwd   = Vec3::new(sy, 0.0, -cy);
        let right = Vec3::new(cy, 0.0,  sy);
        let speed = if input.boost { input.move_speed * 3.0 } else { input.move_speed };

        self.cam_pos += fwd   * input.forward * speed * dt;
        self.cam_pos += right * input.right   * speed * dt;
        self.cam_pos += Vec3::Y * input.up    * speed * dt;
    }

    /// Call on mouse move when not in camera fly mode.
    pub fn update_cursor(&mut self, x: f32, y: f32) {
        self.cursor_pos = (x, y);
    }

    /// Call on left mouse press when not in camera fly mode.
    pub fn on_left_press(&mut self, _x: f32, _y: f32) {}

    /// Call on left mouse release.
    pub fn on_left_release(&mut self) {}

    pub fn is_initialized(&self) -> bool {
        self.inner.is_some()
    }

    pub fn get_metrics(&self) -> RenderMetrics {
        self.metrics.lock().map(|m| m.clone()).unwrap_or_default()
    }

    pub fn get_gpu_profiler_data(&self) -> GpuProfilerData {
        self.gpu_profiler.lock().map(|m| m.clone()).unwrap_or_default()
    }

    // ── Scene sync ────────────────────────────────────────────────────────────

    fn populate_initial_scene(&self, inner: &mut HelioInner) {
        // ── Sky ──────────────────────────────────────────────────────────────
        inner.renderer.scene_mut().insert_actor(SceneActor::Sky(
            SkyActor::new().with_sky_color([0.5, 0.7, 1.0]),
        ));

        // ── Sun (directional light) ──────────────────────────────────────────
        let sun_dir = Vec3::new(-0.5, -1.0, -0.3).normalize();
        inner.renderer.scene_mut().insert_actor(SceneActor::light(GpuLight {
            position_range:  [0.0, 0.0, 0.0, f32::MAX],
            direction_outer: [sun_dir.x, sun_dir.y, sun_dir.z, 0.0],
            color_intensity: [1.0, 0.95, 0.9, 5.0],
            shadow_index:    0,
            light_type:      LightType::Directional as u32,
            inner_angle:     0.0,
            _pad:            0,
        }));

        // ── Fill light (softer, from the opposite side) ──────────────────────
        inner.renderer.scene_mut().insert_actor(SceneActor::light(GpuLight {
            position_range:  [0.0, 10.0, 0.0, 100.0],
            direction_outer: [0.0, -1.0, 0.0, 0.0],
            color_intensity: [0.4, 0.5, 0.7, 2.0],
            shadow_index:    u32::MAX,
            light_type:      LightType::Point as u32,
            inner_angle:     0.0,
            _pad:            0,
        }));

        // ── Ground plane ─────────────────────────────────────────────────────
        let ground_upload = plane_mesh(50.0);
        let ground_mesh = match inner.renderer.scene_mut()
            .insert_actor(SceneActor::mesh(ground_upload.clone()))
            .as_mesh()
        {
            Some(id) => id,
            None     => return,
        };
        let ground_mat = inner.renderer.scene_mut()
            .insert_material(make_material([0.35, 0.35, 0.35, 1.0], 0.9, 0.0));
        let _ = inner.renderer.scene_mut()
            .insert_actor(SceneActor::object(ObjectDescriptor {
                mesh:       ground_mesh,
                material:   ground_mat,
                transform:  Mat4::IDENTITY,
                bounds:     [0.0, 0.0, 0.0, 50.0],
                flags:      0,
                groups:     GroupMask::NONE,
                movability: None,
            }));

        // ── Colored cubes around the origin ──────────────────────────────────
        let cube_upload = box_mesh([0.5, 0.5, 0.5]);
        let cube_mesh = match inner.renderer.scene_mut()
            .insert_actor(SceneActor::mesh(cube_upload.clone()))
            .as_mesh()
        {
            Some(id) => id,
            None     => return,
        };

        let positions_and_colors: &[([f32; 3], [f32; 4])] = &[
            ([ 0.0, 1.0,  0.0], [0.8, 0.2, 0.2, 1.0]),  // red center
            ([ 3.0, 1.0,  0.0], [0.2, 0.7, 0.2, 1.0]),  // green right
            ([-3.0, 1.0,  0.0], [0.2, 0.3, 0.9, 1.0]),  // blue left
            ([ 0.0, 1.0,  5.0], [0.9, 0.9, 0.2, 1.0]),  // yellow front
            ([ 0.0, 1.0, -5.0], [0.8, 0.3, 0.8, 1.0]),  // magenta back
        ];

        for &(pos, color) in positions_and_colors {
            let mat = inner.renderer.scene_mut()
                .insert_material(make_material(color, 0.5, 0.1));
            let transform = Mat4::from_translation(Vec3::from_array(pos));
            let _ = inner.renderer.scene_mut()
                .insert_actor(SceneActor::object(ObjectDescriptor {
                    mesh:       cube_mesh,
                    material:   mat,
                    transform,
                    bounds:     [pos[0], pos[1], pos[2], 1.0],
                    flags:      0,
                    groups:     GroupMask::NONE,
                    movability: None,
                }));
        }
    }

    fn sync_scene(scene_db: &crate::scene::SceneDb, inner: &mut HelioInner) {
        let snapshots = scene_db.get_all_snapshots();

        for snap in &snapshots {
            let key = match snap.object_type {
                ObjectType::Mesh(mt) => MeshKey::from(mt),
                _ => continue,
            };
            if !snap.visible { continue; }

            if let Some(&(obj_id, _)) = inner.object_map.get(&snap.id) {
                // Update transform
                let _ = inner.renderer.scene_mut()
                    .update_object_transform(obj_id, build_transform(snap));
            } else {
                // Insert new object
                let (mesh_id, mat_id) = *inner.mesh_cache.entry(key).or_insert_with(|| {
                    let upload = mesh_for_key(key);
                    let mid = inner.renderer.scene_mut()
                        .insert_actor(SceneActor::mesh(upload.clone()))
                        .as_mesh()
                        .expect("mesh insert");
                    let mat = make_material([0.6, 0.6, 0.65, 1.0], 0.7, 0.0);
                    let matid = inner.renderer.scene_mut().insert_material(mat);
                    (mid, matid)
                });

                let transform = build_transform(snap);
                let radius = Vec3::from_array(snap.scale).length() * 0.5;
                let pos    = transform.w_axis.truncate();

                if let Some(obj_id) = inner.renderer.scene_mut()
                    .insert_actor(SceneActor::object(ObjectDescriptor {
                        mesh:       mesh_id,
                        material:   mat_id,
                        transform,
                        bounds:     [pos.x, pos.y, pos.z, radius.max(0.1)],
                        flags:      0,
                        groups:     GroupMask::NONE,
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
