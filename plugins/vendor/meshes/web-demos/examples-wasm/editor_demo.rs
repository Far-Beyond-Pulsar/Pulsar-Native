//! WASM port of the native `editor_demo` example.
//!
//! Demonstrates the editor API: BVH ray-picking and transform gizmo overlay
//! (translate / rotate / scale).
//!
//! # Controls
//!
//! | Input              | Action                                         |
//! |--------------------|------------------------------------------------|
//! | **Right-click hold** | Capture cursor for free-fly camera           |
//! | **Right-click release** | Release cursor for object picking         |
//! | WASD               | Fly forward / left / back / right (hold RMB)  |
//! | Space / L-Shift    | Fly up / down (hold RMB)                       |
//! | **Left-click**     | Pick object under cursor (cursor free)         |
//! | G                  | Switch to **Translate** gizmo (cursor free)    |
//! | R                  | Switch to **Rotate** gizmo (cursor free)       |
//! | S                  | Switch to **Scale** gizmo (cursor free)        |
//! | Ctrl+D             | **Duplicate** selected object                  |
//! | Delete             | **Delete** selected object                     |
//! | Tab                | Toggle editor grid                             |
//! | **Escape**         | Deselect current object                        |

use std::collections::HashSet;
use std::sync::Arc;

use helio::{Camera, EditorState, GizmoMode, Movability, Renderer, SceneEntity, ScenePicker};
use helio_wasm::{HelioWasmApp, InputState, KeyCode, MouseButton};

use crate::common::{
    box_mesh, cube_mesh, insert_object_with_movability, make_material, plane_mesh, point_light,
    sphere_mesh,
};

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
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Editor Demo  (G=Translate  R=Rotate  S=Scale  RMB=Fly)"
    }

    /// Right-click grabs the cursor for fly-camera mode.
    fn grab_cursor_button() -> MouseButton {
        MouseButton::Right
    }

    /// Right-click is "hold to fly" — release exits fly mode immediately.
    fn release_cursor_on_grab_button_release() -> bool {
        true
    }

    fn init(
        renderer: &mut Renderer,
        _device: Arc<wgpu::Device>,
        _queue: Arc<wgpu::Queue>,
        width: u32,
        height: u32,
    ) -> Self {
        renderer.set_editor_mode(true);
        renderer.set_clear_color([0.08, 0.09, 0.12, 1.0]);
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.12, 0.14, 0.18],
            intensity: 0.25,
        });

        let mut picker = ScenePicker::new();

        // ── Materials ─────────────────────────────────────────────────────────
        let mat_floor = renderer.scene().insert_material(make_material(
            [0.55, 0.55, 0.55, 1.0],
            0.8,
            0.0,
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
        let mat_gold = renderer.scene().insert_material(make_material(
            [1.0, 0.76, 0.1, 1.0],
            0.3,
            0.8,
            [0.0; 3],
            0.0,
        ));
        let mat_sphere = renderer.scene().insert_material(make_material(
            [0.8, 0.5, 0.9, 1.0],
            0.35,
            0.15,
            [0.0; 3],
            0.0,
        ));

        // ── Meshes (clone each upload so the picker can keep the BVH data) ──

        let floor_upload = plane_mesh([0.0; 3], 8.0);
        let floor_mesh = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(floor_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(floor_mesh, &floor_upload);

        let box_a_upload = box_mesh([0.0; 3], [0.55, 0.55, 0.55]);
        let box_a = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(box_a_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(box_a, &box_a_upload);

        let box_b_upload = box_mesh([0.0; 3], [0.4, 0.75, 0.4]);
        let box_b = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(box_b_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(box_b, &box_b_upload);

        let box_c_upload = box_mesh([0.0; 3], [0.6, 0.35, 0.6]);
        let box_c = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(box_c_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(box_c, &box_c_upload);

        let cube_gold_upload = cube_mesh([0.0; 3], 0.45);
        let cube_gold = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(cube_gold_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(cube_gold, &cube_gold_upload);

        let sphere_a_upload = sphere_mesh([0.0; 3], 0.65);
        let sphere_a = renderer
            .scene()
            .insert_entity(SceneEntity::mesh(sphere_a_upload.clone()))
            .as_mesh()
            .unwrap();
        picker.register_mesh(sphere_a, &sphere_a_upload);

        // ── Objects (Floor is static; movable objects are pickable) ───────────

        let _ = insert_object_with_movability(
            renderer,
            floor_mesh,
            mat_floor,
            glam::Mat4::IDENTITY,
            8.5,
            None, // Static
        );

        insert_object_with_movability(
            renderer,
            box_a,
            mat_red,
            glam::Mat4::from_translation(glam::Vec3::new(-2.5, 0.55, 0.5)),
            1.0,
            Some(Movability::Movable),
        )
        .expect("red box");

        insert_object_with_movability(
            renderer,
            box_b,
            mat_green,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.75, -1.0)),
            1.0,
            Some(Movability::Movable),
        )
        .expect("green box");

        insert_object_with_movability(
            renderer,
            box_c,
            mat_blue,
            glam::Mat4::from_translation(glam::Vec3::new(2.5, 0.35, 0.5)),
            0.85,
            Some(Movability::Movable),
        )
        .expect("blue box");

        insert_object_with_movability(
            renderer,
            cube_gold,
            mat_gold,
            glam::Mat4::from_rotation_y(0.6)
                * glam::Mat4::from_translation(glam::Vec3::new(0.5, 0.45, 2.5)),
            0.75,
            Some(Movability::Movable),
        )
        .expect("gold cube");

        insert_object_with_movability(
            renderer,
            sphere_a,
            mat_sphere,
            glam::Mat4::from_translation(glam::Vec3::new(-1.0, 0.65, -2.5)),
            0.85,
            Some(Movability::Movable),
        )
        .expect("purple sphere");

        // Sync picker with all inserted objects.
        picker.rebuild_instances(renderer.scene());

        // ── Lights ────────────────────────────────────────────────────────────

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
            cam_pos: glam::Vec3::new(0.0, 4.0, 9.5),
            cam_yaw: 0.0,
            cam_pitch: -0.35,
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

        // ── Fly camera (cursor grabbed = right-click held) ────────────────────

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

        // ── Pick / edit mode (cursor is free) ─────────────────────────────────

        if !input.cursor_grabbed {
            let aspect = self.width as f32 / self.height.max(1) as f32;
            let proj = glam::Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, aspect, 0.1, 500.0);
            let view = glam::Mat4::look_at_rh(self.cam_pos, self.cam_pos + fwd, glam::Vec3::Y);
            let vp_inv = (proj * view).inverse();
            let (ray_o, ray_d) = EditorState::ray_from_screen(
                input.cursor_pos.0,
                input.cursor_pos.1,
                self.width as f32,
                self.height as f32,
                vp_inv,
            );

            // Hover highlight and drag update every frame.
            self.editor.update_hover(ray_o, ray_d, renderer);
            if self.editor.is_dragging() {
                self.editor.update_drag(ray_o, ray_d, renderer);
            }

            // Left-click: try to start a gizmo drag, else BVH pick.
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

            // Left-click release: finish any active gizmo drag.
            if input.mouse_left_just_released {
                self.editor.end_drag();
            }

            // Keyboard shortcuts (one-shot, using just_pressed).
            if just_pressed.contains(&KeyCode::Escape) {
                self.editor.deselect();
            }
            if just_pressed.contains(&KeyCode::KeyG) {
                self.editor.set_gizmo_mode(GizmoMode::Translate);
            }
            if just_pressed.contains(&KeyCode::KeyR) {
                self.editor.set_gizmo_mode(GizmoMode::Rotate);
            }
            // KeyS for Scale (only when Ctrl is NOT held — Ctrl+S reserved for save)
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

        // ── Camera ────────────────────────────────────────────────────────────
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

        // ── Gizmo overlay ─────────────────────────────────────────────────────
        renderer.debug_clear();
        self.editor.draw_gizmos(renderer);

        // ── Store keys for next frame ─────────────────────────────────────────
        self.prev_keys = input.keys.clone();

        camera
    }
}
