//! WASM twin of `render_v2_basic` — 3 cubes + ground, three orbiting point lights.

use std::sync::Arc;

use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::{Entity, SceneDb};

use crate::common::{
    cube_mesh, insert_object, make_material, plane_mesh, point_light, spawn_light, spawn_material,
    spawn_mesh,
};

const LOOK_SENS: f32 = 0.0024;
const FLY_SPEED: f32 = 5.0;

pub struct Demo {
    _cube1: Entity,
    _cube2: Entity,
    _cube3: Entity,
    _ground: Entity,
    _light_p0: Entity,

    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Basic Render"
    }

    fn init(
        _renderer: &mut Renderer,
        scene_db: &mut SceneDb,
        _device: Arc<wgpu::Device>,
        _queue: Arc<wgpu::Queue>,
        _w: u32,
        _h: u32,
    ) -> Self {
        let world = &mut scene_db.world;
        let mat = spawn_material(
            world,
            make_material([0.7, 0.7, 0.72, 1.0], 0.7, 0.0, [0.0, 0.0, 0.0], 0.0),
        );

        let cube1_mesh = spawn_mesh(world, cube_mesh([0.0, 0.5, 0.0], 0.5));
        let cube2_mesh = spawn_mesh(world, cube_mesh([-2.0, 0.4, -1.0], 0.4));
        let cube3_mesh = spawn_mesh(world, cube_mesh([2.0, 0.3, 0.5], 0.3));
        let ground_mesh = spawn_mesh(world, plane_mesh([0.0, 0.0, 0.0], 5.0));

        let cube1 = insert_object(world, cube1_mesh, mat, glam::Mat4::IDENTITY, 0.5)
            .expect("cube mesh should have a GPU range");
        let cube2 = insert_object(world, cube2_mesh, mat, glam::Mat4::IDENTITY, 0.4)
            .expect("cube mesh should have a GPU range");
        let cube3 = insert_object(world, cube3_mesh, mat, glam::Mat4::IDENTITY, 0.3)
            .expect("cube mesh should have a GPU range");
        let ground = insert_object(world, ground_mesh, mat, glam::Mat4::IDENTITY, 5.0)
            .expect("ground mesh should have a GPU range");

        let light_p0 = spawn_light(
            world,
            point_light([0.0, 2.2, 0.0], [1.0, 0.55, 0.15], 6.0, 5.0),
        );
        spawn_light(
            world,
            point_light([-3.5, 2.0, -1.5], [0.25, 0.5, 1.0], 5.0, 6.0),
        );
        spawn_light(
            world,
            point_light([3.5, 1.5, 1.5], [1.0, 0.3, 0.5], 5.0, 6.0),
        );

        Self {
            _cube1: cube1,
            _cube2: cube2,
            _cube3: cube3,
            _ground: ground,
            _light_p0: light_p0,
            cam_pos: Vec3::new(0.0, 2.5, 7.0),
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
        // Mouse look
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
