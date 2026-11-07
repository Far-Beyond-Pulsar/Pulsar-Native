//! WASM twin of `outdoor_volcano` — active volcano with lava flows.
//!
//! Controls: WASD fly, mouse look. Q/E raise/lower.

use std::sync::Arc;

use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::{Entity, SceneDb};

use crate::common::{
    box_mesh, directional_light, insert_object, make_material, plane_mesh, point_light,
};

const LOOK_SENS: f32 = 0.0024;
const FLY_SPEED: f32 = 10.0;

pub struct Demo {
    lava_lights: Vec<Entity>,
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Outdoor Volcano"
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
        let basalt_m = crate::common::spawn_material(
            world,
            make_material([0.08, 0.07, 0.07, 1.0], 0.95, 0.0, [0.0; 3], 0.0),
        );
        let rock_m = crate::common::spawn_material(
            world,
            make_material([0.14, 0.10, 0.09, 1.0], 0.9, 0.0, [0.0; 3], 0.0),
        );
        let lava_m = crate::common::spawn_material(
            world,
            make_material([0.2, 0.05, 0.01, 1.0], 1.0, 0.0, [1.0, 0.2, 0.0], 12.0),
        );
        let lava_hot_m = crate::common::spawn_material(
            world,
            make_material([0.3, 0.1, 0.02, 1.0], 1.0, 0.0, [1.5, 0.5, 0.05], 20.0),
        );
        let ash_m = crate::common::spawn_material(
            world,
            make_material([0.22, 0.20, 0.18, 1.0], 0.99, 0.0, [0.0; 3], 0.0),
        );

        // Lava plain (ground)
        let ground = crate::common::spawn_mesh(world, plane_mesh([0.0, 0.0, 0.0], 80.0));
        insert_object(world, ground, basalt_m, glam::Mat4::IDENTITY, 80.0).unwrap();

        // Crater rim
        let rim_segs: &[([f32; 3], [f32; 3])] = &[
            ([-20.0, 8.0, 0.0], [6.0, 8.0, 20.0]),
            ([20.0, 7.5, 0.0], [6.0, 7.5, 20.0]),
            ([0.0, 9.0, -20.0], [20.0, 9.0, 6.0]),
            ([0.0, 8.5, 20.0], [20.0, 8.5, 6.0]),
            ([-14.0, 5.5, -14.0], [5.0, 5.5, 5.0]),
            ([14.0, 5.5, -14.0], [5.0, 5.5, 5.0]),
            ([-14.0, 5.0, 14.0], [5.0, 5.0, 5.0]),
            ([14.0, 5.0, 14.0], [5.0, 5.0, 5.0]),
        ];
        for (pos, size) in rim_segs {
            let m = crate::common::spawn_mesh(world, box_mesh(*pos, *size));
            insert_object(world, m, rock_m, glam::Mat4::IDENTITY, size[1]).unwrap();
        }

        // Lava lake (inside crater)
        let lava_lake =
            crate::common::spawn_mesh(world, box_mesh([0.0, 0.3, 0.0], [14.0, 0.3, 14.0]));
        insert_object(world, lava_lake, lava_m, glam::Mat4::IDENTITY, 14.0).unwrap();

        // Hot lava vents
        let vent_positions: &[[f32; 3]] = &[
            [-4.0, 0.4, -4.0],
            [4.0, 0.4, 4.0],
            [-4.0, 0.4, 4.0],
            [4.0, 0.4, -4.0],
            [0.0, 0.4, 7.0],
            [0.0, 0.4, -7.0],
            [7.0, 0.4, 0.0],
            [-7.0, 0.4, 0.0],
        ];
        for &pos in vent_positions {
            let v = crate::common::spawn_mesh(world, box_mesh(pos, [1.5, 0.15, 1.5]));
            insert_object(world, v, lava_hot_m, glam::Mat4::IDENTITY, 1.5).unwrap();
        }

        // Lava flows down the sides
        let flow_data: &[([f32; 3], [f32; 3])] = &[
            ([-21.0, 4.0, 0.0], [0.8, 4.0, 2.5]),
            ([21.0, 3.8, 0.0], [0.8, 3.8, 2.5]),
            ([0.0, 4.2, -21.0], [2.5, 4.2, 0.8]),
            ([5.0, 3.5, 21.0], [2.5, 3.5, 0.8]),
        ];
        for (pos, size) in flow_data {
            let f = crate::common::spawn_mesh(world, box_mesh(*pos, *size));
            insert_object(world, f, lava_m, glam::Mat4::IDENTITY, size[0].max(size[2])).unwrap();
        }

        // Ash dunes in the distance
        let ash_dunes: &[([f32; 3], [f32; 3])] = &[
            ([-55.0, 2.5, 0.0], [8.0, 2.5, 25.0]),
            ([55.0, 3.0, 0.0], [8.0, 3.0, 25.0]),
            ([0.0, 2.0, -55.0], [25.0, 2.0, 8.0]),
            ([0.0, 3.5, 55.0], [25.0, 3.5, 8.0]),
        ];
        for (pos, size) in ash_dunes {
            let d = crate::common::spawn_mesh(world, box_mesh(*pos, *size));
            insert_object(world, d, ash_m, glam::Mat4::IDENTITY, size[0].max(size[2])).unwrap();
        }

        // Lava lights
        let mut lava_lights = Vec::new();
        for &pos in vent_positions {
            let id = crate::common::spawn_light(
                world,
                point_light([pos[0], pos[1] + 0.5, pos[2]], [1.0, 0.3, 0.02], 8.0, 10.0),
            );
            lava_lights.push(id);
        }
        // Central lava lake glow
        let central = crate::common::spawn_light(
            world,
            point_light([0.0, 2.0, 0.0], [1.0, 0.2, 0.0], 100.0, 40.0),
        );
        lava_lights.push(central);
        // Flow lights
        for (pos, _) in flow_data {
            let id =
                crate::common::spawn_light(world, point_light(*pos, [1.0, 0.25, 0.01], 15.0, 12.0));
            lava_lights.push(id);
        }

        // Night sky with red-orange glow from below
        let moon = Vec3::new(0.3, -0.9, 0.4).normalize();
        crate::common::spawn_light(
            world,
            directional_light([moon.x, moon.y, moon.z], [0.25, 0.3, 0.5], 0.002),
        );
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.15, 0.06, 0.02],
            intensity: 0.04,
        });
        renderer.set_clear_color([0.06, 0.02, 0.01, 1.0]);

        Self {
            lava_lights,
            cam_pos: Vec3::new(0.0, 30.0, 60.0),
            cam_yaw: std::f32::consts::PI,
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

        // Lava vent pulsing
        let vent_positions: &[[f32; 3]] = &[
            [-4.0, 0.4, -4.0],
            [4.0, 0.4, 4.0],
            [-4.0, 0.4, 4.0],
            [4.0, 0.4, -4.0],
            [0.0, 0.4, 7.0],
            [0.0, 0.4, -7.0],
            [7.0, 0.4, 0.0],
            [-7.0, 0.4, 0.0],
        ];
        for (i, (id, &pos)) in self.lava_lights[..8].iter().zip(vent_positions).enumerate() {
            let f = 1.0 + (elapsed * (3.0 + i as f32 * 0.4) + i as f32).sin() * 0.2;
            let _ = renderer.scene().update_light(
                *id,
                point_light(
                    [pos[0], pos[1] + 0.5, pos[2]],
                    [1.0, 0.3, 0.02],
                    8.0 * f,
                    10.0,
                ),
            );
        }
        // Central glow pulse
        let f_c = 1.0 + (elapsed * 0.8).sin() * 0.1;
        let _ = renderer.scene().update_light(
            self.lava_lights[8],
            point_light([0.0, 2.0, 0.0], [1.0, 0.2, 0.0], 100.0 * f_c, 40.0),
        );

        Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.5,
            500.0,
        )
    }
}
