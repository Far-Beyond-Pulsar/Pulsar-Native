//! WASM twin of `render_v2_sky` — scene with controllable sun direction.
//!
//! Controls: WASD + Space/Shift to fly, Q/E to rotate the sun (time of day).

use std::sync::Arc;

use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::{Entity, SceneDb};

use crate::common::{
    box_mesh, cube_mesh, directional_light, insert_object, make_material, plane_mesh, point_light,
    spawn_light, spawn_material, spawn_mesh,
};

const LOOK_SENS: f32 = 0.0024;
const FLY_SPEED: f32 = 5.0;

pub struct Demo {
    _cube1: Entity,
    _cube2: Entity,
    _cube3: Entity,
    _ground: Entity,
    _roof: Entity,
    sun_light: Entity,

    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    sun_angle: f32, // 0 → noon, π → midnight
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Volumetric Sky"
    }

    fn init(
        renderer: &mut Renderer,
        scene_db: &mut SceneDb,
        _device: Arc<wgpu::Device>,
        _queue: Arc<wgpu::Queue>,
        _w: u32,
        _h: u32,
    ) -> Self {
        let mat = spawn_material(
            &mut scene_db.world,
            make_material([0.7, 0.7, 0.72, 1.0], 0.7, 0.0, [0.0, 0.0, 0.0], 0.0),
        );

        let cube1 = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.5, 0.0], 0.5));
        let cube2 = spawn_mesh(&mut scene_db.world, cube_mesh([-2.0, 0.4, -1.0], 0.4));
        let cube3 = spawn_mesh(&mut scene_db.world, cube_mesh([2.0, 0.3, 0.5], 0.3));
        let ground = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 5.0));
        let roof = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 1.2, 0.0], [2.5, 0.1, 2.5]),
        );

        let _ = insert_object(&mut scene_db.world, cube1, mat, glam::Mat4::IDENTITY, 0.5);
        let _ = insert_object(&mut scene_db.world, cube2, mat, glam::Mat4::IDENTITY, 0.4);
        let _ = insert_object(&mut scene_db.world, cube3, mat, glam::Mat4::IDENTITY, 0.3);
        let _ = insert_object(&mut scene_db.world, ground, mat, glam::Mat4::IDENTITY, 5.0);
        let _ = insert_object(&mut scene_db.world, roof, mat, glam::Mat4::IDENTITY, 2.5);

        spawn_light(
            &mut scene_db.world,
            point_light([-3.5, 2.0, -1.5], [0.25, 0.5, 1.0], 5.0, 6.0),
        );
        spawn_light(
            &mut scene_db.world,
            point_light([3.5, 1.5, 1.5], [1.0, 0.3, 0.5], 5.0, 6.0),
        );

        let sun_light = spawn_light(
            &mut scene_db.world,
            directional_light([-0.5, -0.8, -0.3], [1.0, 0.9, 0.7], 1.0),
        );
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.2, 0.25, 0.35],
            intensity: 0.15,
        });
        renderer.set_clear_color([0.53, 0.81, 0.98, 1.0]);

        Self {
            _cube1: cube1,
            _cube2: cube2,
            _cube3: cube3,
            _ground: ground,
            _roof: roof,
            sun_light,
            cam_pos: Vec3::new(0.0, 2.5, 7.0),
            cam_yaw: 0.0,
            cam_pitch: -0.2,
            sun_angle: 0.4,
        }
    }

    fn update(
        &mut self,
        renderer: &mut Renderer,
        dt: f32,
        _elapsed: f32,
        input: &InputState,
    ) -> Camera {
        self.cam_yaw += input.mouse_delta.0 * LOOK_SENS;
        self.cam_pitch = (self.cam_pitch - input.mouse_delta.1 * LOOK_SENS).clamp(-1.55, 1.55);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let fwd = Vec3::new(sy * cp, sp, -cy * cp);
        let right = Vec3::new(cy, 0.0, sy);

        if input.keys.contains(&helio_wasm::KeyCode::KeyW) {
            self.cam_pos += fwd * FLY_SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyS) {
            self.cam_pos -= fwd * FLY_SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyA) {
            self.cam_pos -= right * FLY_SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyD) {
            self.cam_pos += right * FLY_SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::Space) {
            self.cam_pos.y += FLY_SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::ShiftLeft) {
            self.cam_pos.y -= FLY_SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyQ) {
            self.sun_angle -= 1.0 * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyE) {
            self.sun_angle += 1.0 * dt;
        }

        // Compute sun direction from angle
        let sun_dir =
            Vec3::new(self.sun_angle.cos(), -self.sun_angle.sin().abs() - 0.1, 0.5).normalize();
        let sun_elev = (-sun_dir.y).clamp(0.0, 1.0);
        let sun_color = [
            1.0_f32.min(1.0 + (1.0 - sun_elev) * 0.3),
            (0.85 + sun_elev * 0.15).clamp(0.0, 1.0),
            (0.7 + sun_elev * 0.3).clamp(0.0, 1.0),
        ];
        let sun_intensity = (sun_elev * 3.0).clamp(0.001, 3.0);
        let sky_r = (0.15 + sun_elev * 0.38).clamp(0.0, 1.0);
        let sky_g = (0.20 + sun_elev * 0.61).clamp(0.0, 1.0);
        let sky_b = (0.35 + sun_elev * 0.63).clamp(0.0, 1.0);
        renderer.set_clear_color([sky_r, sky_g, sky_b, 1.0]);
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.2 + sun_elev * 0.1, 0.25 + sun_elev * 0.05, 0.35],
            intensity: 0.1 + sun_elev * 0.1,
        });
        Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.1,
            200.0,
        )
    }
}
