//! Android port of the `editor_demo_mini` example.
//!
//! A simple shipyard scene with procedural geometry, editor gizmos,
//! and ray-picking. Designed for touch/Android via the HelioWasmApp trait.
//!
//! # Controls (desktop)
//!
//! | Input              | Action                                         |
//! |--------------------|------------------------------------------------|
//! | **Right-click hold** | Capture cursor for free-fly camera           |
//! | **Right-click release** | Release cursor for object picking         |
//! | WASD               | Fly forward / left / back / right (hold RMB)  |
//! | Space / L-Shift    | Fly up / down (hold RMB)                       |
//! | **Left-click**     | Pick object under cursor (cursor free)         |
//! | G                  | Switch to **Translate** gizmo (cursor free)    |
//! | R                  | Switch to **Rotate** gizmo (cursor free)        |
//! | S                  | Switch to **Scale** gizmo (cursor free)        |
//! | Ctrl+D             | **Duplicate** selected object                  |
//! | Delete             | **Delete** selected object                     |
//! | Tab                | Toggle editor grid                             |
//! | Escape             | Deselect current object                        |
//!
//! # Controls (Android touch)
//!
//! Single-finger drag = look around (cursor grabbed style).
//! Two-finger pinch = not yet mapped (fly forward/back).
//! Tap on object = select / gizmo drag.

use std::collections::HashSet;
use std::sync::Arc;

use helio::{Camera, EditorState, GizmoMode, Movability, Renderer, SceneEntity, ScenePicker};
use helio_wasm::{HelioWasmApp, InputState, KeyCode, MouseButton};

// ── Shared helpers (inlined from helio-web-demos common) ──────────────────────

use glam::{Mat4, Vec3};
use helio::{GpuLight, GpuMaterial, LightType, MaterialId, MeshId, ObjectDescriptor};
use helio_mesh_api::{MeshUpload, PackedVertex};

fn make_material(
    base_color: [f32; 4],
    roughness: f32,
    metallic: f32,
    emissive: [f32; 3],
    emissive_strength: f32,
) -> GpuMaterial {
    GpuMaterial {
        base_color,
        emissive: [emissive[0], emissive[1], emissive[2], emissive_strength],
        roughness_metallic: [roughness, metallic, 1.5, 0.5],
        tex_base_color: GpuMaterial::NO_TEXTURE,
        tex_normal: GpuMaterial::NO_TEXTURE,
        tex_roughness: GpuMaterial::NO_TEXTURE,
        tex_emissive: GpuMaterial::NO_TEXTURE,
        tex_occlusion: GpuMaterial::NO_TEXTURE,
        workflow: 0,
        flags: 0,
        material_class: 0,
        class_params: [0.0; 4],
    }
}

fn point_light(position: [f32; 3], color: [f32; 3], intensity: f32, range: f32) -> GpuLight {
    GpuLight {
        position_range: [position[0], position[1], position[2], range],
        direction_outer: [0.0, 0.0, -1.0, 0.0],
        color_intensity: [color[0], color[1], color[2], intensity],
        shadow_index: 0,
        light_type: LightType::Point as u32,
        inner_angle: 0.0,
        _pad: 0,
        ..Default::default()
    }
}

fn insert_object_with_movability(
    renderer: &mut Renderer,
    mesh: MeshId,
    material: MaterialId,
    transform: Mat4,
    radius: f32,
    movability: Option<helio::Movability>,
) -> helio::SceneResult<helio::ObjectId> {
    let object_actor_id =
        renderer
            .scene()
            .insert_entity(helio::SceneEntity::object(ObjectDescriptor {
                mesh,
                material,
                transform,
                bounds: [
                    transform.w_axis.x,
                    transform.w_axis.y,
                    transform.w_axis.z,
                    radius,
                ],
                flags: 0,
                groups: helio::GroupMask::NONE,
                movability,
                user_tag: 0,
            }));
    object_actor_id
        .as_object()
        .ok_or(helio::SceneError::InvalidHandle { resource: "object" })
}

fn cube_mesh(center: [f32; 3], half_extent: f32) -> MeshUpload {
    box_mesh(center, [half_extent, half_extent, half_extent])
}

fn box_mesh(center: [f32; 3], half_extents: [f32; 3]) -> MeshUpload {
    let c = Vec3::from_array(center);
    let e = Vec3::from_array(half_extents);
    let corners = [
        c + Vec3::new(-e.x, -e.y, e.z),
        c + Vec3::new(e.x, -e.y, e.z),
        c + Vec3::new(e.x, e.y, e.z),
        c + Vec3::new(-e.x, e.y, e.z),
        c + Vec3::new(-e.x, -e.y, -e.z),
        c + Vec3::new(e.x, -e.y, -e.z),
        c + Vec3::new(e.x, e.y, -e.z),
        c + Vec3::new(-e.x, e.y, -e.z),
    ];
    let faces: [([usize; 4], [f32; 3], [f32; 3]); 6] = [
        ([0, 1, 2, 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([5, 4, 7, 6], [0.0, 0.0, -1.0], [-1.0, 0.0, 0.0]),
        ([4, 0, 3, 7], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([1, 5, 6, 2], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([3, 2, 6, 7], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ([4, 5, 1, 0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]),
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

fn plane_mesh(center: [f32; 3], half_extent: f32) -> MeshUpload {
    let c = Vec3::from_array(center);
    let e = half_extent;
    let normal = [0.0, 1.0, 0.0];
    let tangent = [1.0, 0.0, 0.0];
    let positions = [
        c + Vec3::new(-e, 0.0, -e),
        c + Vec3::new(e, 0.0, -e),
        c + Vec3::new(e, 0.0, e),
        c + Vec3::new(-e, 0.0, e),
    ];
    let uvs = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let vertices = positions
        .into_iter()
        .zip(uvs)
        .map(|(pos, uv)| PackedVertex::from_components(pos.to_array(), normal, uv, tangent, 1.0))
        .collect();
    MeshUpload {
        vertices,
        indices: vec![0, 2, 1, 0, 3, 2],
    }
}

fn sphere_mesh(center: [f32; 3], radius: f32) -> MeshUpload {
    let center = Vec3::from_array(center);
    let lat_steps = 16u32;
    let lon_steps = 32u32;
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
            let tangent = Vec3::new(-z, 0.0, x).normalize_or_zero().to_array();
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
            let a = i * (lon_steps + 1) + j;
            let b = a + (lon_steps + 1);
            indices.extend_from_slice(&[a, a + 1, b]);
            indices.extend_from_slice(&[b, a + 1, b + 1]);
        }
    }

    MeshUpload { vertices, indices }
}

// ── Demo struct ───────────────────────────────────────────────────────────────

pub struct Demo {
    // Camera
    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    width: u32,
    height: u32,

    // Editor
    editor: EditorState,
    picker: ScenePicker,
    grid_enabled: bool,

    // One-frame "just pressed" tracking for keyboard shortcuts.
    prev_keys: HashSet<KeyCode>,
}

// ── HelioWasmApp impl ─────────────────────────────────────────────────────────

impl HelioWasmApp for Demo {
    fn features() -> helio_feature_api::FeatureSet {
        helio_mesh_host::features()
    }

    fn register_columns(
        store: &mut pulsar_scenedb::gpu::SceneGpuStore,
        device: &std::sync::Arc<wgpu::Device>,
    ) {
        helio_mesh_host::register_columns(store, device);
    }

    fn title() -> &'static str {
        "Helio — Editor Demo Mini  (G=Translate  R=Rotate  S=Scale  RMB=Fly)"
    }

    fn grab_cursor_button() -> MouseButton {
        MouseButton::Right
    }

    fn release_cursor_on_grab_button_release() -> bool {
        true
    }

    fn render_scale() -> f32 {
        1.0
    }

    fn init(
        renderer: &mut Renderer,
        _device: Arc<wgpu::Device>,
        _queue: Arc<wgpu::Queue>,
        width: u32,
        height: u32,
    ) -> Self {
        renderer.set_editor_mode(true);
        renderer.set_clear_color([0.03, 0.05, 0.10, 1.0]);
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.18, 0.22, 0.32],
            intensity: 0.35,
        });

        let mut picker = ScenePicker::new();

        // ── Materials ─────────────────────────────────────────────────────
        let mat_dock = renderer.scene().insert_material(make_material(
            [0.42, 0.40, 0.38, 1.0],
            0.95,
            0.0,
            [0.0; 3],
            0.0,
        ));
        let mat_steel = renderer.scene().insert_material(make_material(
            [0.25, 0.26, 0.28, 1.0],
            0.15,
            0.6,
            [0.0; 3],
            0.0,
        ));
        let mat_orange = renderer.scene().insert_material(make_material(
            [0.85, 0.35, 0.05, 1.0],
            0.3,
            0.4,
            [0.0; 3],
            0.0,
        ));
        let mat_warning = renderer.scene().insert_material(make_material(
            [1.0, 0.1, 0.05, 1.0],
            0.4,
            0.0,
            [1.0, 0.05, 0.0],
            1.5,
        ));
        let mat_water = renderer.scene().insert_material(make_material(
            [0.04, 0.12, 0.20, 1.0],
            0.05,
            0.95,
            [0.0; 3],
            0.0,
        ));
        let mat_red = renderer.scene().insert_material(make_material(
            [0.9, 0.15, 0.15, 1.0],
            0.5,
            0.0,
            [0.0; 3],
            0.0,
        ));
        let mat_green = renderer.scene().insert_material(make_material(
            [0.15, 0.85, 0.25, 1.0],
            0.5,
            0.0,
            [0.0; 3],
            0.0,
        ));
        let mat_blue = renderer.scene().insert_material(make_material(
            [0.15, 0.35, 0.95, 1.0],
            0.5,
            0.0,
            [0.0; 3],
            0.0,
        ));
        let mat_lamp = renderer.scene().insert_material(make_material(
            [0.90, 0.85, 0.50, 1.0],
            0.3,
            0.0,
            [0.6, 0.55, 0.1],
            0.8,
        ));
        let mat_bollard = renderer.scene().insert_material(make_material(
            [0.18, 0.14, 0.10, 1.0],
            0.85,
            0.0,
            [0.0; 3],
            0.0,
        ));

        // ── Ground — large dock apron ──────────────────────────────────────
        let dock_upload = plane_mesh([0.0, 0.0, 0.0], 100.0);
        let dock_mesh = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(dock_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(dock_mesh, &dock_upload);
        insert_object_with_movability(
            renderer,
            dock_mesh,
            mat_dock,
            glam::Mat4::IDENTITY,
            120.0,
            None,
        )
        .ok();

        // ── Harbour water ──────────────────────────────────────────────────
        let water_upload = plane_mesh([0.0, -0.15, 0.0], 80.0);
        let water_mesh = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(water_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(water_mesh, &water_upload);
        insert_object_with_movability(
            renderer,
            water_mesh,
            mat_water,
            glam::Mat4::from_translation(glam::Vec3::new(115.0, 0.0, 0.0)),
            90.0,
            None,
        )
        .ok();

        // ── Crane (one unit, pickable) ─────────────────────────────────────
        let leg_upload = box_mesh([0.0; 3], [1.0, 18.0, 1.0]);
        let leg_mesh = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(leg_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(leg_mesh, &leg_upload);
        let cx = 0.0_f32;
        let cz = -38.0_f32;
        for leg_dx in [-5.0_f32, 5.0_f32] {
            insert_object_with_movability(
                renderer,
                leg_mesh,
                mat_steel,
                glam::Mat4::from_translation(glam::Vec3::new(cx + leg_dx, 9.0, cz)),
                10.0,
                Some(Movability::Movable),
            )
            .ok();
        }
        let beam_upload = box_mesh([0.0; 3], [13.0, 1.0, 1.0]);
        let beam_mesh = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(beam_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(beam_mesh, &beam_upload);
        insert_object_with_movability(
            renderer,
            beam_mesh,
            mat_orange,
            glam::Mat4::from_translation(glam::Vec3::new(cx, 18.0, cz)),
            8.0,
            Some(Movability::Movable),
        )
        .ok();

        let boom_upload = box_mesh([0.0; 3], [18.0, 0.8, 0.8]);
        let boom_mesh = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(boom_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(boom_mesh, &boom_upload);
        insert_object_with_movability(
            renderer,
            boom_mesh,
            mat_steel,
            glam::Mat4::from_translation(glam::Vec3::new(cx + 10.0, 17.5, cz)),
            12.0,
            Some(Movability::Movable),
        )
        .ok();

        let beacon_upload = sphere_mesh([0.0; 3], 0.45);
        let beacon_mesh = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(beacon_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(beacon_mesh, &beacon_upload);
        insert_object_with_movability(
            renderer,
            beacon_mesh,
            mat_warning,
            glam::Mat4::from_translation(glam::Vec3::new(cx + 27.5, 17.5, cz)),
            0.6,
            Some(Movability::Movable),
        )
        .ok();

        // ── Movable objects (coloured boxes + sphere) ──────────────────────
        let box_a_upload = box_mesh([0.0; 3], [0.55, 0.55, 0.55]);
        let box_a = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(box_a_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(box_a, &box_a_upload);
        insert_object_with_movability(
            renderer,
            box_a,
            mat_red,
            glam::Mat4::from_translation(glam::Vec3::new(-2.5, 0.55, 0.5)),
            1.0,
            Some(Movability::Movable),
        )
        .ok();

        let box_b_upload = box_mesh([0.0; 3], [0.4, 0.75, 0.4]);
        let box_b = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(box_b_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(box_b, &box_b_upload);
        insert_object_with_movability(
            renderer,
            box_b,
            mat_green,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.75, -1.0)),
            1.0,
            Some(Movability::Movable),
        )
        .ok();

        let box_c_upload = box_mesh([0.0; 3], [0.6, 0.35, 0.6]);
        let box_c = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(box_c_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(box_c, &box_c_upload);
        insert_object_with_movability(
            renderer,
            box_c,
            mat_blue,
            glam::Mat4::from_translation(glam::Vec3::new(2.5, 0.35, 0.5)),
            0.85,
            Some(Movability::Movable),
        )
        .ok();

        let cube_gold_upload = cube_mesh([0.0; 3], 0.45);
        let cube_gold = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(cube_gold_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(cube_gold, &cube_gold_upload);
        insert_object_with_movability(
            renderer,
            cube_gold,
            mat_lamp,
            glam::Mat4::from_rotation_y(0.6)
                * glam::Mat4::from_translation(glam::Vec3::new(0.5, 0.45, 2.5)),
            0.75,
            Some(Movability::Movable),
        )
        .ok();

        // ── Bollards along the quay edge ───────────────────────────────────
        let bollard_upload = box_mesh([0.0; 3], [0.28, 0.6, 0.28]);
        let bollard_mesh = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(bollard_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(bollard_mesh, &bollard_upload);
        for bi in 0..6i32 {
            let bx = -25.0 + bi as f32 * 10.0;
            insert_object_with_movability(
                renderer,
                bollard_mesh,
                mat_bollard,
                glam::Mat4::from_translation(glam::Vec3::new(bx, 0.6, -43.5)),
                0.8,
                None,
            )
            .ok();
        }

        // Sync picker with all inserted objects.
        picker.rebuild_instances(renderer.scene());

        // ── Lights ─────────────────────────────────────────────────────────
        renderer
            .scene()
            .insert_entity(SceneEntity::light(point_light(
                [cx, 20.0, cz],
                [0.90, 0.96, 1.0],
                400.0,
                35.0,
            )));
        renderer
            .scene()
            .insert_entity(SceneEntity::light(point_light(
                [0.0, 4.5, 2.0],
                [1.0, 0.85, 0.7],
                14.0,
                12.0,
            )));
        renderer
            .scene()
            .insert_entity(SceneEntity::light(point_light(
                [-4.0, 3.0, -3.0],
                [0.4, 0.55, 1.0],
                8.0,
                9.0,
            )));
        renderer
            .scene()
            .insert_entity(SceneEntity::light(point_light(
                [4.0, 2.5, -2.0],
                [1.0, 0.4, 0.3],
                6.0,
                8.0,
            )));

        Demo {
            cam_pos: glam::Vec3::new(0.0, 8.0, 20.0),
            cam_yaw: 0.0,
            cam_pitch: -0.25,
            width,
            height,
            editor: EditorState::new(),
            picker,
            grid_enabled: true,
            prev_keys: HashSet::new(),
        }
    }

    fn on_resize(&mut self, _renderer: &mut Renderer, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    fn update(
        &mut self,
        renderer: &mut Renderer,
        dt: f32,
        _elapsed: f32,
        input: &InputState,
    ) -> Camera {
        const LOOK: f32 = 0.0025;
        const MOVE: f32 = 6.0;

        // Keys that transitioned from up → down this frame.
        let just_pressed: HashSet<KeyCode> =
            input.keys.difference(&self.prev_keys).copied().collect();

        // ── Fly camera (cursor grabbed = right-click held) ─────────────────

        if input.cursor_grabbed {
            self.cam_yaw += input.mouse_delta.0 * LOOK;
            self.cam_pitch -= input.mouse_delta.1 * LOOK;
            self.cam_pitch = self.cam_pitch.clamp(
                -std::f32::consts::FRAC_PI_2 * 0.99,
                std::f32::consts::FRAC_PI_2 * 0.99,
            );
        }

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let fwd = glam::Vec3::new(sy * cp, sp, -cy * cp);
        let right = glam::Vec3::new(cy, 0.0, sy);

        if input.cursor_grabbed {
            let mut vel = glam::Vec3::ZERO;
            if input.keys.contains(&KeyCode::KeyW) {
                vel += fwd;
            }
            if input.keys.contains(&KeyCode::KeyS) {
                vel -= fwd;
            }
            if input.keys.contains(&KeyCode::KeyD) {
                vel += right;
            }
            if input.keys.contains(&KeyCode::KeyA) {
                vel -= right;
            }
            if input.keys.contains(&KeyCode::Space) {
                vel += glam::Vec3::Y;
            }
            if input.keys.contains(&KeyCode::ShiftLeft) {
                vel -= glam::Vec3::Y;
            }
            if vel.length_squared() > 0.0 {
                self.cam_pos += vel.normalize() * MOVE * dt;
            }
        }

        // ── Pick / edit mode (cursor is free) ──────────────────────────────

        if !input.cursor_grabbed {
            let aspect = self.width as f32 / self.height.max(1) as f32;
            let proj = glam::camera::rh::proj::directx::perspective(
                std::f32::consts::FRAC_PI_4,
                aspect,
                0.1,
                500.0,
            );
            let view = glam::camera::rh::view::look_at_mat4(
                self.cam_pos,
                self.cam_pos + fwd,
                glam::Vec3::Y,
            );
            let vp_inv = (proj * view).inverse();
            let (ray_o, ray_d) = EditorState::ray_from_screen(
                input.cursor_pos.0,
                input.cursor_pos.1,
                self.width as f32,
                self.height as f32,
                vp_inv,
            );

            self.editor.update_hover(ray_o, ray_d, renderer);
            if self.editor.is_dragging() {
                self.editor.update_drag(ray_o, ray_d, renderer);
            }

            if input.mouse_left_just_pressed {
                if !self.editor.try_start_drag(ray_o, ray_d, renderer.scene()) {
                    self.picker.rebuild_instances(renderer.scene());
                    if let Some(hit) = self.picker.cast_ray(renderer.scene(), ray_o, ray_d) {
                        self.editor.select(hit.actor_id);
                    } else {
                        self.editor.deselect();
                    }
                }
            }

            if input.mouse_left_just_released {
                self.editor.end_drag();
            }

            if just_pressed.contains(&KeyCode::Escape) {
                self.editor.deselect();
            }
            if just_pressed.contains(&KeyCode::KeyG) {
                self.editor.set_gizmo_mode(GizmoMode::Translate);
            }
            if just_pressed.contains(&KeyCode::KeyR) {
                self.editor.set_gizmo_mode(GizmoMode::Rotate);
            }
            if just_pressed.contains(&KeyCode::KeyS)
                && !input.keys.contains(&KeyCode::ControlLeft)
                && !input.keys.contains(&KeyCode::ControlRight)
            {
                self.editor.set_gizmo_mode(GizmoMode::Scale);
            }
            if just_pressed.contains(&KeyCode::Delete) {
                if self.editor.delete_selected(renderer.scene()) {
                    self.picker.rebuild_instances(renderer.scene());
                }
            }
            if just_pressed.contains(&KeyCode::KeyD)
                && (input.keys.contains(&KeyCode::ControlLeft)
                    || input.keys.contains(&KeyCode::ControlRight))
            {
                if self.editor.duplicate_selected(renderer).is_some() {
                    self.picker.rebuild_instances(renderer.scene());
                }
            }
            if just_pressed.contains(&KeyCode::Tab) {
                self.grid_enabled = !self.grid_enabled;
                renderer.set_editor_mode(self.grid_enabled);
            }
        }

        // ── Camera ────────────────────────────────────────────────────────
        let aspect = self.width as f32 / self.height.max(1) as f32;
        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            500.0,
        );

        // ── Gizmo overlay ──────────────────────────────────────────────────
        renderer.debug_clear();
        self.editor.draw_gizmos(renderer);

        // ── Store keys for next frame ──────────────────────────────────────
        self.prev_keys = input.keys.clone();

        camera
    }
}
