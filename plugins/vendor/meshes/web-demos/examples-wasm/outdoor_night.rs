//! WASM twin of `outdoor_night` — nighttime plaza with streetlamps.

use std::sync::Arc;

use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::SceneDb;

use crate::common::{
    box_mesh, insert_object, make_material, plane_mesh, point_light, spawn_light, spawn_material,
    spawn_mesh,
};

const LOOK_SENS: f32 = 0.0024;
const FLY_SPEED: f32 = 5.0;

pub struct Demo {
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Outdoor Night"
    }

    fn init(
        renderer: &mut Renderer,
        scene_db: &mut SceneDb,
        _device: Arc<wgpu::Device>,
        _queue: Arc<wgpu::Queue>,
        _w: u32,
        _h: u32,
    ) -> Self {
        let world = &mut scene_db.world;
        let concrete = spawn_material(
            world,
            make_material([0.7, 0.7, 0.72, 1.0], 0.8, 0.0, [0.0; 3], 0.0),
        );
        let glass = spawn_material(
            world,
            make_material([0.3, 0.35, 0.4, 0.5], 0.1, 0.9, [0.0; 3], 0.0),
        );
        let pole_mat = spawn_material(
            world,
            make_material([0.2, 0.2, 0.22, 1.0], 0.3, 0.8, [0.0; 3], 0.0),
        );

        // Ground
        let ground = spawn_mesh(world, plane_mesh([0.0, 0.0, 0.0], 30.0));
        let _ = insert_object(world, ground, concrete, glam::Mat4::IDENTITY, 30.0);

        // Buildings arranged around a central plaza
        let bld_data = [
            ([-12.0, 5.0, -12.0], [4.0, 5.0, 4.0]),
            ([12.0, 7.0, -12.0], [4.0, 7.0, 4.0]),
            ([-12.0, 6.0, 12.0], [4.0, 6.0, 4.0]),
            ([12.0, 4.5, 12.0], [4.0, 4.5, 4.0]),
            ([0.0, 8.0, -20.0], [6.0, 8.0, 3.0]),
        ];
        for (pos, ext) in bld_data {
            let m = spawn_mesh(world, box_mesh(pos, ext));
            let _ = insert_object(world, m, concrete, glam::Mat4::IDENTITY, 10.0);
            // Glass band near top
            let gw = spawn_mesh(
                world,
                box_mesh([pos[0], pos[1] * 2.0 - 1.0, pos[2]], [ext[0], 0.4, ext[2]]),
            );
            let _ = insert_object(world, gw, glass, glam::Mat4::IDENTITY, 4.0);
        }

        // Streetlamp poles (4 corners of the plaza)
        let lamp_positions = [[-8.0_f32, -8.0], [8.0, -8.0], [-8.0, 8.0], [8.0, 8.0]];
        for [lx, lz] in lamp_positions {
            let pole = spawn_mesh(world, box_mesh([lx, 2.5, lz], [0.08, 2.5, 0.08]));
            let _ = insert_object(world, pole, pole_mat, glam::Mat4::IDENTITY, 2.5);
            // Warm streetlight
            spawn_light(
                world,
                point_light([lx, 5.2, lz], [1.0, 0.85, 0.55], 6.0, 14.0),
            );
        }

        renderer.set_feature_live(helio::AmbientLight {
            color: [0.05, 0.08, 0.15],
            intensity: 0.03,
        });
        renderer.set_clear_color([0.01, 0.01, 0.04, 1.0]);

        Self {
            cam_pos: Vec3::new(0.0, 5.0, 20.0),
            cam_yaw: 0.0,
            cam_pitch: -0.2,
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
