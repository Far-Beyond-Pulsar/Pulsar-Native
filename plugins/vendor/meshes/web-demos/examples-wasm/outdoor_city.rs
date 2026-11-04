//! WASM twin of `outdoor_city` — night-time city grid.
//!
//! Controls: WASD fly + mouse look. Q/E raise/lower.

use std::sync::Arc;

use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::{Entity, SceneDb};

use crate::common::{
    box_mesh, directional_light, insert_object, make_material, plane_mesh, point_light,
};

const LOOK_SENS: f32 = 0.0024;
const FLY_SPEED: f32 = 14.0;

pub struct Demo {
    window_ids: Vec<Entity>,
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Outdoor City"
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
        let road_m = crate::common::spawn_material(
            world,
            make_material([0.11, 0.11, 0.12, 1.0], 0.9, 0.1, [0.0; 3], 0.0),
        );
        let concrete_m = crate::common::spawn_material(
            world,
            make_material([0.55, 0.55, 0.56, 1.0], 0.8, 0.0, [0.0; 3], 0.0),
        );
        let glass_m = crate::common::spawn_material(
            world,
            make_material([0.1, 0.12, 0.2, 1.0], 0.1, 0.9, [0.08, 0.1, 0.15], 0.5),
        );
        let lit_window_m = crate::common::spawn_material(
            world,
            make_material([0.6, 0.55, 0.35, 1.0], 0.5, 0.0, [0.9, 0.8, 0.5], 4.0),
        );
        let street_pole_m = crate::common::spawn_material(
            world,
            make_material([0.3, 0.3, 0.3, 1.0], 0.4, 0.4, [0.0; 3], 0.0),
        );
        let lamp_m = crate::common::spawn_material(
            world,
            make_material([0.9, 0.85, 0.7, 1.0], 0.2, 0.0, [1.0, 0.95, 0.75], 6.0),
        );

        // City ground plane
        let ground = crate::common::spawn_mesh(world, plane_mesh([0.0, 0.0, 0.0], 100.0));
        insert_object(world, ground, road_m, glam::Mat4::IDENTITY, 100.0).unwrap();

        // Buildings on a grid: 7x7 blocks
        let building_data: &[(f32, f32, [f32; 3], f32)] = &[
            (-36.0, -36.0, [6.0, 18.0, 6.0], 18.0),
            (-36.0, -18.0, [5.0, 30.0, 5.0], 30.0),
            (-36.0, 0.0, [7.0, 12.0, 7.0], 12.0),
            (-36.0, 18.0, [6.0, 22.0, 6.0], 22.0),
            (-36.0, 36.0, [5.0, 10.0, 5.0], 10.0),
            (-18.0, -36.0, [6.0, 40.0, 6.0], 40.0),
            (-18.0, -18.0, [8.0, 16.0, 8.0], 16.0),
            (-18.0, 0.0, [5.0, 50.0, 5.0], 50.0),
            (-18.0, 18.0, [6.0, 24.0, 6.0], 24.0),
            (-18.0, 36.0, [7.0, 14.0, 7.0], 14.0),
            (0.0, -36.0, [5.0, 20.0, 5.0], 20.0),
            (0.0, -18.0, [6.0, 36.0, 6.0], 36.0),
            (0.0, 0.0, [9.0, 60.0, 9.0], 60.0),
            (0.0, 18.0, [7.0, 28.0, 7.0], 28.0),
            (0.0, 36.0, [5.0, 15.0, 5.0], 15.0),
            (18.0, -36.0, [6.0, 18.0, 6.0], 18.0),
            (18.0, -18.0, [7.0, 45.0, 7.0], 45.0),
            (18.0, 0.0, [5.0, 32.0, 5.0], 32.0),
            (18.0, 18.0, [6.0, 20.0, 6.0], 20.0),
            (18.0, 36.0, [8.0, 12.0, 8.0], 12.0),
            (36.0, -36.0, [5.0, 25.0, 5.0], 25.0),
            (36.0, -18.0, [6.0, 38.0, 6.0], 38.0),
            (36.0, 0.0, [7.0, 18.0, 7.0], 18.0),
            (36.0, 18.0, [5.0, 42.0, 5.0], 42.0),
            (36.0, 36.0, [6.0, 16.0, 6.0], 16.0),
        ];

        for (bx, bz, half, rad) in building_data.iter() {
            let h = half[1];
            let bm = crate::common::spawn_mesh(world, box_mesh([*bx, h / 2.0, *bz], *half));
            insert_object(world, bm, concrete_m, glam::Mat4::IDENTITY, *rad).unwrap();
            // Glass facade panels
            let gm = crate::common::spawn_mesh(
                world,
                box_mesh([*bx, h / 2.0, *bz], [half[0] - 0.1, h - 0.2, half[2] - 0.1]),
            );
            insert_object(world, gm, glass_m, glam::Mat4::IDENTITY, *rad).unwrap();
            // Random lit windows strip
            let wm = crate::common::spawn_mesh(
                world,
                box_mesh(
                    [*bx, h * 0.6, *bz],
                    [half[0] - 0.15, h * 0.25, half[2] - 0.05],
                ),
            );
            insert_object(
                world,
                wm,
                lit_window_m,
                glam::Mat4::IDENTITY,
                half[1] * 0.25,
            )
            .unwrap();
        }

        // Street lamps — on grid intersections
        let lamp_xs: &[f32] = &[-27.0, -9.0, 9.0, 27.0];
        let lamp_zs: &[f32] = &[-27.0, -9.0, 9.0, 27.0];
        let mut window_ids = Vec::new();
        for &lx in lamp_xs {
            for &lz in lamp_zs {
                let pole =
                    crate::common::spawn_mesh(world, box_mesh([lx, 5.0, lz], [0.08, 5.0, 0.08]));
                insert_object(world, pole, street_pole_m, glam::Mat4::IDENTITY, 5.0).unwrap();
                let arm = crate::common::spawn_mesh(
                    world,
                    box_mesh([lx + 0.5, 9.8, lz], [0.5, 0.06, 0.06]),
                );
                insert_object(world, arm, street_pole_m, glam::Mat4::IDENTITY, 0.5).unwrap();
                let lamp = crate::common::spawn_mesh(
                    world,
                    box_mesh([lx + 0.9, 9.75, lz], [0.12, 0.12, 0.12]),
                );
                insert_object(world, lamp, lamp_m, glam::Mat4::IDENTITY, 0.12).unwrap();
                let id = crate::common::spawn_light(
                    world,
                    point_light([lx + 0.9, 9.5, lz], [1.0, 0.92, 0.72], 80.0, 18.0),
                );
                window_ids.push(id);
            }
        }

        // Rooftop lights on tallest buildings
        for (bx, bz, _, _) in building_data.iter().filter(|(_, _, h, _)| h[1] > 35.0) {
            let id = crate::common::spawn_light(
                world,
                point_light(
                    [*bx, 0.0 /* set dynamically */ + 2.0, *bz],
                    [1.0, 0.1, 0.05],
                    5.0,
                    8.0,
                ),
            );
            window_ids.push(id);
        }

        // Moonlight
        let moon = Vec3::new(-0.3, -0.8, 0.5).normalize();
        crate::common::spawn_light(
            world,
            directional_light([moon.x, moon.y, moon.z], [0.4, 0.5, 0.9], 0.003),
        );
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.1, 0.12, 0.2],
            intensity: 0.02,
        });
        renderer.set_clear_color([0.02, 0.02, 0.06, 1.0]);

        Self {
            window_ids,
            cam_pos: Vec3::new(0.0, 42.0, 90.0),
            cam_yaw: std::f32::consts::PI,
            cam_pitch: -0.25,
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

        // Rooftop beacon lights blink
        for (i, id) in self.window_ids[16..].iter().enumerate() {
            let on = ((elapsed * 1.2 + i as f32 * 0.5).sin() > 0.0) as u8 as f32;
            let _ = renderer.scene().update_light(
                *id,
                point_light([0.0, 0.1, 0.0], [1.0, 0.1, 0.05], 5.0 * on, 8.0),
            );
        }
        let _ = elapsed;

        Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.5,
            600.0,
        )
    }
}
