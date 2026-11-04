//! WASM twin of `light_benchmark` — flat ground with many point lights.
//!
//! Stress-tests the renderer's deferred lighting pipeline.
//! Controls: WASD fly, mouse look.

use std::sync::Arc;

use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::{Entity, SceneDb};

use crate::common::{
    box_mesh, cube_mesh, directional_light, insert_object, make_material, plane_mesh, point_light,
};

const LOOK_SENS: f32 = 0.0024;
const FLY_SPEED: f32 = 12.0;
const LIGHT_COUNT: usize = 128;
const GRID_W: usize = 16;

pub struct Demo {
    light_ids: Vec<Entity>,
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Light Benchmark"
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
        let ground_m = crate::common::spawn_material(
            world,
            make_material([0.4, 0.4, 0.4, 1.0], 0.8, 0.05, [0.0; 3], 0.0),
        );
        let box_m = crate::common::spawn_material(
            world,
            make_material([0.55, 0.52, 0.48, 1.0], 0.7, 0.0, [0.0; 3], 0.0),
        );
        let light_m = crate::common::spawn_material(
            world,
            make_material([1.0, 1.0, 1.0, 1.0], 0.0, 0.0, [2.0, 2.0, 2.0], 5.0),
        );

        // Ground plane
        let ground = crate::common::spawn_mesh(world, plane_mesh([0.0, 0.0, 0.0], 64.0));
        insert_object(world, ground, ground_m, glam::Mat4::IDENTITY, 64.0).unwrap();

        // Grid of obstacle boxes
        let n = 8usize;
        for iz in 0..n {
            for ix in 0..n {
                let x = -28.0 + ix as f32 * 8.0;
                let z = -28.0 + iz as f32 * 8.0;
                let h = 0.5 + (ix * n + iz) as f32 % 3.0;
                let bm = crate::common::spawn_mesh(world, box_mesh([x, h / 2.0, z], [0.8, h, 0.8]));
                insert_object(world, bm, box_m, glam::Mat4::IDENTITY, h).unwrap();
            }
        }

        // Light emitter spheres (small cubes)
        let mut light_ids = Vec::with_capacity(LIGHT_COUNT);
        for i in 0..LIGHT_COUNT {
            let col = i % GRID_W;
            let row = i / GRID_W;
            let x = -28.0 + col as f32 * (56.0 / (GRID_W as f32 - 1.0));
            let z = -28.0 + row as f32 * (56.0 / (LIGHT_COUNT / GRID_W - 1).max(1) as f32);
            let hue = i as f32 / LIGHT_COUNT as f32;
            let (r, g, b) = hsv_to_rgb(hue, 0.8, 1.0);
            let bulb = crate::common::spawn_mesh(world, cube_mesh([x, 1.8, z], 0.07));
            insert_object(world, bulb, light_m, glam::Mat4::IDENTITY, 0.07).unwrap();
            let id =
                crate::common::spawn_light(world, point_light([x, 2.0, z], [r, g, b], 10.0, 8.0));
            light_ids.push(id);
        }

        // Faint directional ambient
        crate::common::spawn_light(
            world,
            directional_light([0.2, -0.9, 0.4], [0.8, 0.85, 1.0], 0.002),
        );
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.1, 0.1, 0.15],
            intensity: 0.01,
        });
        renderer.set_clear_color([0.01, 0.01, 0.02, 1.0]);

        Self {
            light_ids,
            cam_pos: Vec3::new(0.0, 14.0, 60.0),
            cam_yaw: std::f32::consts::PI,
            cam_pitch: -0.4,
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

        // Animate lights: each drifts up and down at its own phase
        for (i, id) in self.light_ids.iter().enumerate() {
            let col = i % GRID_W;
            let row = i / GRID_W;
            let x = -28.0 + col as f32 * (56.0 / (GRID_W as f32 - 1.0));
            let z = -28.0 + row as f32 * (56.0 / (LIGHT_COUNT / GRID_W - 1).max(1) as f32);
            let phase = i as f32 * 0.618;
            let y = 2.0 + (elapsed * 1.5 + phase).sin() * 1.2;
            let hue = (i as f32 / LIGHT_COUNT as f32 + elapsed * 0.05).fract();
            let (r, g, b) = hsv_to_rgb(hue, 0.8, 1.0);
            let _ = renderer
                .scene()
                .update_light(*id, point_light([x, y, z], [r, g, b], 10.0, 8.0));
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

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h6 = h * 6.0;
    let i = h6.floor() as i32;
    let f = h6 - h6.floor();
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}
