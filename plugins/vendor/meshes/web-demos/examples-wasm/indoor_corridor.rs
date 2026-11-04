//! WASM twin of `indoor_corridor` — long corridor with periodic overhead lights.

use std::sync::Arc;

use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::{Entity, SceneDb};

use crate::common::{
    box_mesh, insert_object, make_material, point_light, spawn_light, spawn_material, spawn_mesh,
};

const LOOK_SENS: f32 = 0.0024;
const FLY_SPEED: f32 = 4.0;

pub struct Demo {
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Indoor Corridor"
    }

    fn init(
        renderer: &mut Renderer,
        scene_db: &mut SceneDb,
        _device: Arc<wgpu::Device>,
        _queue: Arc<wgpu::Queue>,
        _w: u32,
        _h: u32,
    ) -> Self {
        let concrete = spawn_material(
            &mut scene_db.world,
            make_material([0.60, 0.58, 0.55, 1.0], 0.85, 0.0, [0.0; 3], 0.0),
        );
        let tile_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.80, 0.78, 0.75, 1.0], 0.6, 0.0, [0.0; 3], 0.0),
        );

        // Long corridor: 2.4m wide, 3m tall, 40m long (z: 0..40)
        let floor = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, -0.05, 20.0], [1.2, 0.05, 20.0]),
        );
        let ceiling = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 3.05, 20.0], [1.2, 0.05, 20.0]),
        );
        let wall_l = spawn_mesh(
            &mut scene_db.world,
            box_mesh([-1.3, 1.5, 20.0], [0.1, 1.5, 20.0]),
        );
        let wall_r = spawn_mesh(
            &mut scene_db.world,
            box_mesh([1.3, 1.5, 20.0], [0.1, 1.5, 20.0]),
        );
        let back_w = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 1.5, 40.1], [1.4, 1.5, 0.1]),
        );
        let front_w = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 1.5, -0.1], [1.4, 1.5, 0.1]),
        );

        for (m, mat, r) in [
            (floor, tile_mat, 20.0),
            (ceiling, concrete, 20.0),
            (wall_l, concrete, 20.0),
            (wall_r, concrete, 20.0),
            (back_w, concrete, 5.0),
            (front_w, concrete, 5.0),
        ] {
            let _ = insert_object(&mut scene_db.world, m, mat, glam::Mat4::IDENTITY, r);
        }

        // Periodic overhead lights every 5m
        for i in 0..8 {
            let z = 2.5 + i as f32 * 5.0;
            spawn_light(
                &mut scene_db.world,
                point_light([0.0, 2.8, z], [1.0, 0.95, 0.85], 3.0, 6.0),
            );
        }
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.15, 0.15, 0.18],
            intensity: 0.05,
        });
        renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);

        Self {
            cam_pos: Vec3::new(0.0, 1.7, 2.0),
            cam_yaw: 0.0,
            cam_pitch: 0.0,
        }
    }

    fn update(
        &mut self,
        _renderer: &mut Renderer,
        dt: f32,
        _elapsed: f32,
        input: &InputState,
    ) -> Camera {
        self.cam_yaw += input.mouse_delta.0 * LOOK_SENS;
        self.cam_pitch = (self.cam_pitch - input.mouse_delta.1 * LOOK_SENS).clamp(-1.4, 1.4);

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

        Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.1,
            60.0,
        )
    }
}
