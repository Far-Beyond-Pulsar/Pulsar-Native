//! WASM twin of `debug_shapes` — an exact replica of the native demo.
//!
//! The native version draws no scene geometry; it exercises the renderer's
//! immediate-mode debug primitives (`debug_line`, `debug_sphere`, `debug_torus`,
//! …) every frame through the default graph's `DebugDrawPass`, with editor mode
//! enabled. That pass is present in the graph the wasm runner already builds, so
//! this twin reproduces the native draw list, camera, and look/move controls
//! verbatim — no custom graph required.
//!
//! Controls:
//!   WASD / Space / Shift — fly (5 m/s)
//!   Mouse drag           — look (click to grab cursor)
//!   Escape               — release cursor

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState, KeyCode};

const LOOK_SPEED: f32 = 0.003;
const MOVE_SPEED: f32 = 5.0;

pub struct Demo {
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Debug Shapes"
    }

    fn init(
        renderer: &mut Renderer,
        _device: Arc<wgpu::Device>,
        _queue: Arc<wgpu::Queue>,
        _w: u32,
        _h: u32,
    ) -> Self {
        renderer.set_clear_color([0.12, 0.12, 0.16, 1.0]);
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.20, 0.22, 0.30],
            intensity: 0.18,
        });
        renderer.set_editor_mode(true);

        Self {
            cam_pos: Vec3::new(0.0, 3.0, 10.0),
            cam_yaw: 0.0,
            cam_pitch: -0.35,
        }
    }

    fn update(
        &mut self,
        renderer: &mut Renderer,
        dt: f32,
        elapsed: f32,
        input: &InputState,
    ) -> Camera {
        // ── Look ──────────────────────────────────────────────────────────
        self.cam_yaw += input.mouse_delta.0 * LOOK_SPEED;
        self.cam_pitch -= input.mouse_delta.1 * LOOK_SPEED;
        self.cam_pitch = self.cam_pitch.clamp(-FRAC_PI_2 * 0.99, FRAC_PI_2 * 0.99);

        // ── Move (horizontal basis, matching the native demo) ─────────────
        let fwd_h = Vec3::new(self.cam_yaw.sin(), 0.0, -self.cam_yaw.cos());
        let right = Vec3::new(self.cam_yaw.cos(), 0.0, self.cam_yaw.sin());
        let up = Vec3::Y;
        let mut vel = Vec3::ZERO;
        if input.keys.contains(&KeyCode::KeyW) {
            vel += fwd_h;
        }
        if input.keys.contains(&KeyCode::KeyS) {
            vel -= fwd_h;
        }
        if input.keys.contains(&KeyCode::KeyD) {
            vel += right;
        }
        if input.keys.contains(&KeyCode::KeyA) {
            vel -= right;
        }
        if input.keys.contains(&KeyCode::Space) {
            vel += up;
        }
        if input.keys.contains(&KeyCode::ShiftLeft) {
            vel -= up;
        }
        if vel.length_squared() > 0.0 {
            self.cam_pos += vel.normalize() * MOVE_SPEED * dt;
        }

        // ── Debug draw list (verbatim from the native demo) ───────────────
        let t = elapsed;
        renderer.debug_clear();

        // Circle
        let ring_radius = 2.5;
        renderer.debug_circle([0.0, 0.5, 0.0], ring_radius, [1.0, 0.4, 0.1, 1.0], 64);

        // Sphere
        let sphere_center = Vec3::new((t * 0.6).cos() * 3.0, 1.0, (t * 0.6).sin() * 3.0);
        renderer.debug_sphere(sphere_center.to_array(), 1.0, [0.2, 0.8, 0.6, 1.0], 32);

        // Torus
        let torus_center = Vec3::new((t * 0.4).sin() * 3.0, 1.5, (t * 0.4).cos() * 3.0);
        renderer.debug_torus(
            torus_center.to_array(),
            [0.0, 1.0, 0.0],
            1.2,
            0.35,
            [1.0, 0.6, 0.7, 1.0],
            24,
            16,
        );

        // Cylinder
        let cyl_base = Vec3::new(-3.5, 0.0, (t * 0.7).sin() * 3.0);
        renderer.debug_cylinder(
            cyl_base.to_array(),
            [0.0, 1.0, 0.0],
            2.0,
            0.45,
            [0.4, 0.4, 1.0, 1.0],
            28,
        );

        // Cone
        let cone_apex = Vec3::new(3.5, 1.5, (t * 0.7).cos() * 3.0);
        renderer.debug_cone(
            cone_apex.to_array(),
            [0.0, -1.0, 0.0],
            2.0,
            0.8,
            [0.8, 0.5, 0.2, 1.0],
            32,
        );

        // Frustum
        let frustum_origin = Vec3::new(0.0, 0.5, 0.0);
        let frustum_dir = Vec3::new((t * 0.2).sin(), -0.15, (t * 0.2).cos()).normalize_or_zero();
        renderer.debug_frustum(
            frustum_origin.to_array(),
            frustum_dir.to_array(),
            Vec3::new(0.0, 1.0, 0.0).to_array(),
            65.0_f32.to_radians(),
            16.0 / 9.0,
            0.8,
            3.2,
            [0.2, 1.0, 0.2, 1.0],
        );

        // Rotating cross
        let rot = t * 0.8;
        let p = Vec3::new(rot.cos() * 2.0, 0.0, rot.sin() * 2.0);
        let col = [0.2 + 0.8 * ((rot * 1.23).sin() * 0.5 + 0.5), 0.80, 0.2, 1.0];
        renderer.debug_line([p.x, 0.0, p.z], [p.x, 1.2, p.z], col);
        renderer.debug_line([p.x - 0.6, 0.6, p.z], [p.x + 0.6, 0.6, p.z], col);
        renderer.debug_line([p.x, 0.6, p.z - 0.6], [p.x, 0.6, p.z + 0.6], col);

        // Major axis lines
        renderer.debug_line([-40.0, 0.0, 0.0], [40.0, 0.0, 0.0], [1.0, 0.0, 0.0, 1.0]);
        renderer.debug_line([0.0, 0.0, -40.0], [0.0, 0.0, 40.0], [0.0, 1.0, 0.0, 1.0]);
        renderer.debug_line([0.0, 0.0, 0.0], [0.0, 40.0, 0.0], [0.0, 0.0, 1.0, 1.0]);

        // Camera-forward debug vector (full pitch)
        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let fwd = Vec3::new(sy * cp, sp, -cy * cp);
        let debug_origin = self.cam_pos + fwd * 0.2;
        let debug_target = self.cam_pos + fwd * 6.0;
        renderer.debug_line(
            debug_origin.to_array(),
            debug_target.to_array(),
            [1.0, 1.0, 0.0, 1.0],
        );

        // Near-camera cross marker
        let world_cam_mark = self.cam_pos + fwd * 2.0;
        let cross = 0.5;
        renderer.debug_line(
            world_cam_mark.to_array(),
            (world_cam_mark + Vec3::new(cross, 0.0, 0.0)).to_array(),
            [1.0, 0.5, 0.0, 1.0],
        );
        renderer.debug_line(
            world_cam_mark.to_array(),
            (world_cam_mark + Vec3::new(0.0, cross, 0.0)).to_array(),
            [1.0, 0.5, 0.0, 1.0],
        );
        renderer.debug_line(
            world_cam_mark.to_array(),
            (world_cam_mark + Vec3::new(0.0, 0.0, cross)).to_array(),
            [1.0, 0.5, 0.0, 1.0],
        );

        Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            Vec3::Y,
            70.0_f32.to_radians(),
            input.aspect_ratio(),
            0.1,
            1000.0,
        )
    }
}
