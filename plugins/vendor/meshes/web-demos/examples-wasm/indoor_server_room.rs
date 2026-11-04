//! WASM twin of `indoor_server_room` — datacenter with blinking indicator lights.
//!
//! Controls: WASD walk, mouse look.

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
const WALK_SPEED: f32 = 3.0;

pub struct Demo {
    indicator_ids: Vec<Entity>,
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Indoor Server Room"
    }

    fn init(
        renderer: &mut Renderer,
        scene_db: &mut SceneDb,
        _device: Arc<wgpu::Device>,
        _queue: Arc<wgpu::Queue>,
        _w: u32,
        _h: u32,
    ) -> Self {
        // Materials
        let floor_m = spawn_material(
            &mut scene_db.world,
            make_material([0.18, 0.18, 0.20, 1.0], 0.6, 0.2, [0.0; 3], 0.0),
        );
        let ceil_m = spawn_material(
            &mut scene_db.world,
            make_material([0.22, 0.22, 0.24, 1.0], 0.7, 0.1, [0.0; 3], 0.0),
        );
        let wall_m = spawn_material(
            &mut scene_db.world,
            make_material([0.25, 0.25, 0.27, 1.0], 0.8, 0.0, [0.0; 3], 0.0),
        );
        let rack_m = spawn_material(
            &mut scene_db.world,
            make_material([0.08, 0.08, 0.10, 1.0], 0.5, 0.3, [0.0; 3], 0.0),
        );
        let blade_m = spawn_material(
            &mut scene_db.world,
            make_material([0.12, 0.13, 0.15, 1.0], 0.3, 0.5, [0.0; 3], 0.0),
        );
        let led_green = spawn_material(
            &mut scene_db.world,
            make_material([0.0, 0.15, 0.0, 1.0], 1.0, 0.0, [0.0, 1.0, 0.0], 3.5),
        );
        let led_red = spawn_material(
            &mut scene_db.world,
            make_material([0.15, 0.0, 0.0, 1.0], 1.0, 0.0, [1.0, 0.0, 0.0], 3.5),
        );
        let led_amber = spawn_material(
            &mut scene_db.world,
            make_material([0.15, 0.08, 0.0, 1.0], 1.0, 0.0, [1.0, 0.6, 0.0], 3.5),
        );
        let led_blue = spawn_material(
            &mut scene_db.world,
            make_material([0.0, 0.0, 0.15, 1.0], 1.0, 0.0, [0.1, 0.2, 1.0], 3.5),
        );
        let cable_m = spawn_material(
            &mut scene_db.world,
            make_material([0.05, 0.05, 0.06, 1.0], 0.9, 0.0, [0.0; 3], 0.0),
        );
        let strip_m = spawn_material(
            &mut scene_db.world,
            make_material([0.8, 0.85, 0.9, 1.0], 0.3, 0.0, [0.6, 0.7, 0.9], 2.0),
        );

        // Room shell
        let floor = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 12.0));
        insert_object(
            &mut scene_db.world,
            floor,
            floor_m,
            glam::Mat4::IDENTITY,
            12.0,
        )
        .unwrap();
        let ceiling = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 3.0, 0.0], [12.0, 0.1, 12.0]),
        );
        insert_object(
            &mut scene_db.world,
            ceiling,
            ceil_m,
            glam::Mat4::IDENTITY,
            12.0,
        )
        .unwrap();
        for (pos, size, rad) in [
            ([-12.0, 1.5, 0.0], [0.1, 3.0, 12.0], 12.0_f32),
            ([12.0, 1.5, 0.0], [0.1, 3.0, 12.0], 12.0),
            ([0.0, 1.5, -12.0], [12.0, 3.0, 0.1], 12.0),
            ([0.0, 1.5, 12.0], [12.0, 3.0, 0.1], 12.0),
        ] {
            let wm = spawn_mesh(&mut scene_db.world, box_mesh(pos, size));
            insert_object(&mut scene_db.world, wm, wall_m, glam::Mat4::IDENTITY, rad).unwrap();
        }

        // Server racks — two rows
        let rack_positions: &[[f32; 3]] = &[
            [-8.0, 1.0, -8.0],
            [-8.0, 1.0, -4.0],
            [-8.0, 1.0, 0.0],
            [-8.0, 1.0, 4.0],
            [-8.0, 1.0, 8.0],
            [8.0, 1.0, -8.0],
            [8.0, 1.0, -4.0],
            [8.0, 1.0, 0.0],
            [8.0, 1.0, 4.0],
            [8.0, 1.0, 8.0],
        ];
        for &pos in rack_positions {
            let rack = spawn_mesh(&mut scene_db.world, box_mesh(pos, [0.5, 2.0, 0.9]));
            insert_object(&mut scene_db.world, rack, rack_m, glam::Mat4::IDENTITY, 2.0).unwrap();
            // Server blades (6 per rack)
            for blade_y in 0..6 {
                let blade_pos = [pos[0] + 0.26, pos[1] - 0.85 + blade_y as f32 * 0.3, pos[2]];
                let blade =
                    spawn_mesh(&mut scene_db.world, box_mesh(blade_pos, [0.04, 0.12, 0.85]));
                insert_object(
                    &mut scene_db.world,
                    blade,
                    blade_m,
                    glam::Mat4::IDENTITY,
                    0.85,
                )
                .unwrap();
                // LEDs
                let led_x = pos[0] + 0.48;
                for (k, (led_mat, dz)) in [
                    (led_green, -0.3),
                    (led_amber, -0.1),
                    (led_red, 0.1),
                    (led_blue, 0.3),
                ]
                .iter()
                .enumerate()
                {
                    let _ = k;
                    let led = spawn_mesh(
                        &mut scene_db.world,
                        cube_mesh([led_x, blade_pos[1], pos[2] + dz], 0.018),
                    );
                    insert_object(
                        &mut scene_db.world,
                        led,
                        *led_mat,
                        glam::Mat4::IDENTITY,
                        0.018,
                    )
                    .unwrap();
                }
            }
            // Cable bundles at rear
            let cable = spawn_mesh(
                &mut scene_db.world,
                box_mesh([pos[0] - 0.55, pos[1] + 0.4, pos[2]], [0.12, 0.4, 0.8]),
            );
            insert_object(
                &mut scene_db.world,
                cable,
                cable_m,
                glam::Mat4::IDENTITY,
                0.8,
            )
            .unwrap();
        }

        // Overhead LED strips
        let strip_positions: &[[f32; 3]] = &[[-8.0, 2.95, 0.0], [8.0, 2.95, 0.0], [0.0, 2.95, 0.0]];
        let mut indicator_ids = Vec::new();
        for &pos in strip_positions {
            let strip = spawn_mesh(&mut scene_db.world, box_mesh(pos, [0.1, 0.05, 10.0]));
            insert_object(
                &mut scene_db.world,
                strip,
                strip_m,
                glam::Mat4::IDENTITY,
                10.0,
            )
            .unwrap();
            let id = spawn_light(
                &mut scene_db.world,
                point_light(pos, [0.65, 0.75, 0.95], 80.0, 12.0),
            );
            indicator_ids.push(id);
        }

        // Indicator accent lights per rack row
        for &pos in rack_positions.iter().step_by(2) {
            let id = spawn_light(
                &mut scene_db.world,
                point_light([pos[0], 0.8, pos[2]], [0.0, 0.9, 0.3], 3.0, 4.0),
            );
            indicator_ids.push(id);
        }

        renderer.set_feature_live(helio::AmbientLight {
            color: [0.25, 0.3, 0.4],
            intensity: 0.03,
        });
        renderer.set_clear_color([0.01, 0.01, 0.02, 1.0]);

        Self {
            indicator_ids,
            cam_pos: Vec3::new(0.0, 1.7, 10.0),
            cam_yaw: std::f32::consts::PI,
            cam_pitch: -0.05,
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
        self.cam_pitch = (self.cam_pitch - input.mouse_delta.1 * LOOK_SENS).clamp(-1.3, 1.3);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let fwd = Vec3::new(sy * cp, sp, -cy * cp);
        let right = Vec3::new(cy, 0.0, sy);
        let fwd_flat = Vec3::new(sy, 0.0, -cy);

        if input.keys.contains(&helio_wasm::KeyCode::KeyW) {
            self.cam_pos += fwd_flat * WALK_SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyS) {
            self.cam_pos -= fwd_flat * WALK_SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyA) {
            self.cam_pos -= right * WALK_SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyD) {
            self.cam_pos += right * WALK_SPEED * dt;
        }

        // Dynamic indicator updates require a mutable World in the update hook.
        Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.05,
            50.0,
        )
    }
}
