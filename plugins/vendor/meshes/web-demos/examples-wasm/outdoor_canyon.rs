//! WASM twin of `outdoor_canyon` — desert canyon at golden hour.
//!
//! Controls: WASD + fly, Q/E to rotate the sun.

use std::sync::Arc;

use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::{Entity, SceneDb};

use crate::common::{
    box_mesh, cube_mesh, directional_light, insert_object, make_material, plane_mesh, point_light,
};

const LOOK_SENS: f32 = 0.0024;
const FLY_SPEED: f32 = 8.0;

pub struct Demo {
    sun_light: Entity,
    fire_light: Entity,

    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    sun_angle: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Outdoor Canyon"
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
        let mat = crate::common::spawn_material(
            world,
            make_material([0.72, 0.58, 0.42, 1.0], 0.85, 0.0, [0.0; 3], 0.0),
        );
        let fire_mat = crate::common::spawn_material(
            world,
            make_material([0.3, 0.1, 0.05, 1.0], 0.9, 0.0, [1.0, 0.4, 0.05], 4.0),
        );

        let valley_floor = crate::common::spawn_mesh(world, plane_mesh([0.0, 0.0, 0.0], 35.0));
        let wall_l1 =
            crate::common::spawn_mesh(world, box_mesh([-12.0, 4.0, 0.0], [3.0, 4.0, 30.0]));
        let wall_l2 =
            crate::common::spawn_mesh(world, box_mesh([-18.0, 8.0, 0.0], [3.0, 8.0, 25.0]));
        let wall_l3 =
            crate::common::spawn_mesh(world, box_mesh([-24.0, 14.0, 0.0], [3.0, 14.0, 20.0]));
        let wall_r1 =
            crate::common::spawn_mesh(world, box_mesh([12.0, 4.0, 0.0], [3.0, 4.0, 30.0]));
        let wall_r2 =
            crate::common::spawn_mesh(world, box_mesh([18.0, 8.0, 0.0], [3.0, 8.0, 25.0]));
        let wall_r3 =
            crate::common::spawn_mesh(world, box_mesh([24.0, 14.0, 0.0], [3.0, 14.0, 20.0]));
        let terrace_l1 =
            crate::common::spawn_mesh(world, box_mesh([-13.5, 8.1, -2.0], [1.5, 0.2, 12.0]));
        let terrace_l2 =
            crate::common::spawn_mesh(world, box_mesh([-19.5, 16.1, -4.0], [1.5, 0.2, 8.0]));
        let terrace_r1 =
            crate::common::spawn_mesh(world, box_mesh([13.5, 8.1, -2.0], [1.5, 0.2, 12.0]));
        let terrace_r2 =
            crate::common::spawn_mesh(world, box_mesh([19.5, 16.1, -4.0], [1.5, 0.2, 8.0]));
        let mesa =
            crate::common::spawn_mesh(world, box_mesh([3.0, 12.0, -38.0], [10.0, 12.0, 8.0]));
        let tent_a = crate::common::spawn_mesh(world, box_mesh([-2.5, 0.6, 8.0], [0.8, 0.6, 1.2]));
        let tent_b = crate::common::spawn_mesh(world, box_mesh([0.0, 0.7, 7.5], [0.9, 0.7, 1.3]));
        let tent_c = crate::common::spawn_mesh(world, box_mesh([2.8, 0.55, 8.5], [0.7, 0.55, 1.1]));
        let firepit = crate::common::spawn_mesh(world, cube_mesh([0.0, 0.15, 9.5], 0.2));

        for (m, mmat, r) in [
            (valley_floor, mat, 35.0),
            (wall_l1, mat, 20.0),
            (wall_l2, mat, 20.0),
            (wall_l3, mat, 20.0),
            (wall_r1, mat, 20.0),
            (wall_r2, mat, 20.0),
            (wall_r3, mat, 20.0),
            (terrace_l1, mat, 10.0),
            (terrace_l2, mat, 8.0),
            (terrace_r1, mat, 10.0),
            (terrace_r2, mat, 8.0),
            (mesa, mat, 14.0),
            (tent_a, mat, 1.0),
            (tent_b, mat, 1.0),
            (tent_c, mat, 1.0),
            (firepit, fire_mat, 0.3),
        ] {
            let _ = insert_object(world, m, mmat, glam::Mat4::IDENTITY, r);
        }

        let sun_light = crate::common::spawn_light(
            world,
            directional_light([-0.0, -1.0, -0.5], [1.0, 0.9, 0.7], 0.005),
        );
        let fire_light = crate::common::spawn_light(
            world,
            point_light([0.0, 0.5, 9.5], [1.0, 0.45, 0.1], 5.0, 12.0),
        );
        crate::common::spawn_light(
            world,
            point_light([-0.4, 0.4, 9.2], [1.0, 0.35, 0.05], 1.5, 5.0),
        );
        crate::common::spawn_light(
            world,
            point_light([0.4, 0.4, 9.8], [1.0, 0.35, 0.05], 1.5, 5.0),
        );
        let moon_dir = Vec3::new(0.4, -0.7, 0.3).normalize();
        crate::common::spawn_light(
            world,
            directional_light([moon_dir.x, moon_dir.y, moon_dir.z], [0.5, 0.65, 1.0], 0.05),
        );
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.6, 0.55, 0.45],
            intensity: 0.08,
        });
        renderer.set_clear_color([0.45, 0.6, 0.85, 1.0]);

        Self {
            sun_light,
            fire_light,
            cam_pos: Vec3::new(0.0, 4.0, 25.0),
            cam_yaw: 0.0,
            cam_pitch: -0.15,
            sun_angle: 0.45,
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
        if input.keys.contains(&helio_wasm::KeyCode::KeyQ) {
            self.sun_angle -= 0.8 * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyE) {
            self.sun_angle += 0.8 * dt;
        }

        // Sun
        let light_dir = Vec3::new(
            self.sun_angle.cos() * 0.6,
            -(self.sun_angle.sin() * 0.8 + 0.1),
            0.5,
        )
        .normalize();
        let sun_elev = (-light_dir.y).clamp(0.0, 1.0);
        let sun_color = [
            (0.95 + sun_elev * 0.05).min(1.0),
            0.85 + sun_elev * 0.05,
            0.6 + sun_elev * 0.3,
        ];
        let sun_lux = (sun_elev * 3.0).clamp(0.0, 1.0);
        let _ = renderer.scene().update_light(
            self.sun_light,
            directional_light(
                [light_dir.x, light_dir.y, light_dir.z],
                sun_color,
                (sun_lux * 0.4).max(0.005),
            ),
        );

        // Fire flicker
        let flicker = 1.0 + (elapsed * 8.3).sin() * 0.15 + (elapsed * 13.7).cos() * 0.08;
        let _ = renderer.scene().update_light(
            self.fire_light,
            point_light([0.0, 0.5, 9.5], [1.0, 0.45, 0.1], 5.0 * flicker, 12.0),
        );

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
