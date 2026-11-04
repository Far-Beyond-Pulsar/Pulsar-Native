//! WASM twin of `indoor_room` — simple furnished interior with flickering light.

use std::sync::Arc;

use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::{Entity, SceneDb};

use crate::common::{
    box_mesh, insert_object, make_material, plane_mesh, point_light, spawn_light, spawn_material,
    spawn_mesh,
};

const LOOK_SENS: f32 = 0.0024;
const FLY_SPEED: f32 = 3.0;

pub struct Demo {
    overhead_light: Entity,
    _meshes: Vec<Entity>,

    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Indoor Room"
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
        let wall_mat = spawn_material(
            world,
            make_material([0.7, 0.68, 0.62, 1.0], 0.8, 0.0, [0.0; 3], 0.0),
        );
        let floor_mat = spawn_material(
            world,
            make_material([0.55, 0.45, 0.35, 1.0], 0.9, 0.0, [0.0; 3], 0.0),
        );
        let wood_mat = spawn_material(
            world,
            make_material([0.50, 0.35, 0.20, 1.0], 0.7, 0.0, [0.0; 3], 0.0),
        );

        let mut meshes = Vec::new();

        // Room shell
        let floor = spawn_mesh(world, plane_mesh([0.0, 0.0, 0.0], 5.0));
        let ceiling = spawn_mesh(world, box_mesh([0.0, 3.0, 0.0], [5.0, 0.05, 5.0]));
        let wall_n = spawn_mesh(world, box_mesh([0.0, 1.5, -5.0], [5.0, 1.5, 0.1]));
        let wall_s = spawn_mesh(world, box_mesh([0.0, 1.5, 5.0], [5.0, 1.5, 0.1]));
        let wall_e = spawn_mesh(world, box_mesh([5.0, 1.5, 0.0], [0.1, 1.5, 5.0]));
        let wall_w = spawn_mesh(world, box_mesh([-5.0, 1.5, 0.0], [0.1, 1.5, 5.0]));
        // Furniture
        let table = spawn_mesh(world, box_mesh([0.0, 0.4, 0.0], [1.2, 0.05, 0.7]));
        let bookcase = spawn_mesh(world, box_mesh([-3.5, 1.0, -4.0], [0.3, 1.0, 1.5]));
        let sofa = spawn_mesh(world, box_mesh([3.0, 0.45, -2.5], [1.5, 0.45, 0.6]));

        for (mesh, mat, r) in [
            (floor, floor_mat, 7.0),
            (ceiling, wall_mat, 7.0),
            (wall_n, wall_mat, 5.0),
            (wall_s, wall_mat, 5.0),
            (wall_e, wall_mat, 5.0),
            (wall_w, wall_mat, 5.0),
            (table, wood_mat, 1.2),
            (bookcase, wood_mat, 1.5),
            (sofa, wood_mat, 1.5),
        ] {
            let _ = insert_object(world, mesh, mat, glam::Mat4::IDENTITY, r);
            meshes.push(mesh);
        }

        let overhead_light = spawn_light(
            world,
            point_light([0.0, 2.85, 0.0], [1.0, 0.85, 0.6], 4.0, 7.0),
        );
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.4, 0.35, 0.3],
            intensity: 0.05,
        });
        renderer.set_clear_color([0.05, 0.05, 0.08, 1.0]);

        Self {
            overhead_light,
            _meshes: meshes,
            cam_pos: Vec3::new(0.0, 1.7, 4.0),
            cam_yaw: std::f32::consts::PI,
            cam_pitch: 0.0,
        }
    }

    fn update(
        &mut self,
        renderer: &mut Renderer,
        dt: f32,
        elapsed: f32,
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

        // Subtle flicker
        let flicker = 1.0 + (elapsed * 11.3).sin() * 0.04 + (elapsed * 7.7).cos() * 0.02;
        let _ = flicker;

        Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.1,
            50.0,
        )
    }
}
