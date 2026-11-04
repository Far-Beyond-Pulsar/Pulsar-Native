//! WASM twin of `load_fbx` — FBX file-load showcase.
//!
//! On WASM, runtime file I/O is unavailable, so this shows a placeholder
//! scene with a note. Controls: WASD fly, mouse look.

use std::sync::Arc;

use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::SceneDb;

use crate::common::{
    box_mesh, directional_light, insert_object, make_material, plane_mesh, point_light, spot_light,
};

const LOOK_SENS: f32 = 0.002;
const SPEED: f32 = 8.0;

pub struct Demo {
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Load FBX (WASM Placeholder)"
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
        // Runtime file I/O is not available on WASM.
        // Build a simple showcase stage as a stand-in for the loaded FBX.
        let floor_m = crate::common::spawn_material(
            world,
            make_material([0.07, 0.08, 0.10, 1.0], 0.16, 0.02, [0.0; 3], 0.0),
        );
        let pedestal_m = crate::common::spawn_material(
            world,
            make_material([0.11, 0.12, 0.15, 1.0], 0.28, 0.04, [0.0; 3], 0.0),
        );
        let backdrop_m = crate::common::spawn_material(
            world,
            make_material([0.04, 0.05, 0.08, 1.0], 0.82, 0.0, [0.04, 0.06, 0.12], 0.03),
        );
        let cube_mat = crate::common::spawn_material(
            world,
            make_material([0.55, 0.52, 0.5, 1.0], 0.6, 0.1, [0.0; 3], 0.0),
        );
        let text_m = crate::common::spawn_material(
            world,
            make_material([0.9, 0.8, 0.2, 1.0], 0.1, 0.0, [0.9, 0.8, 0.1], 2.0),
        );

        // Floor
        let floor = crate::common::spawn_mesh(world, plane_mesh([0.0, 0.0, 0.0], 20.0));
        insert_object(world, floor, floor_m, glam::Mat4::IDENTITY, 20.0).unwrap();

        // Pedestal
        let ped = crate::common::spawn_mesh(world, box_mesh([0.0, 0.3, 0.0], [2.5, 0.3, 2.5]));
        insert_object(world, ped, pedestal_m, glam::Mat4::IDENTITY, 2.5).unwrap();

        // Placeholder "model" — stacked boxes suggesting a figure
        for (pos, half) in [
            ([0.0, 1.0, 0.0], [0.6, 0.7, 0.5]),
            ([0.0, 2.1, 0.0], [0.35, 0.35, 0.35]),
            ([0.0, 1.0, 0.0], [1.2, 0.1, 0.5]),
        ] {
            let m = crate::common::spawn_mesh(world, box_mesh(pos, half));
            insert_object(world, m, cube_mat, glam::Mat4::IDENTITY, 1.2).unwrap();
        }

        // "FBX N/A" sign (emissive slab)
        let sign = crate::common::spawn_mesh(world, box_mesh([0.0, 2.2, -4.5], [1.8, 0.4, 0.06]));
        insert_object(world, sign, text_m, glam::Mat4::IDENTITY, 1.8).unwrap();

        // Backdrop
        let back = crate::common::spawn_mesh(world, box_mesh([0.0, 4.0, -9.5], [10.0, 4.0, 0.1]));
        insert_object(world, back, backdrop_m, glam::Mat4::IDENTITY, 10.0).unwrap();

        // Three-point lighting (same as native showcase)
        let focus = Vec3::new(0.0, 1.2, 0.0);
        let r = 4.5_f32;
        let key = focus + Vec3::new(r * 0.22, r * 0.34, r * 0.24);
        let fill = focus + Vec3::new(-r * 0.26, r * 0.14, r * 0.28);
        let rim = focus + Vec3::new(-r * 0.30, r * 0.22, -r * 0.32);
        crate::common::spawn_light(
            world,
            spot_light(
                key.to_array(),
                (focus - key).normalize().to_array(),
                [1.0, 0.80, 0.62],
                18.0,
                r * 0.62,
                0.20,
                0.38,
            ),
        );
        crate::common::spawn_light(
            world,
            spot_light(
                fill.to_array(),
                (focus - fill).normalize().to_array(),
                [0.52, 0.66, 1.0],
                6.5,
                r * 0.59,
                0.28,
                0.46,
            ),
        );
        crate::common::spawn_light(
            world,
            spot_light(
                rim.to_array(),
                (focus - rim).normalize().to_array(),
                [0.36, 0.55, 1.0],
                14.0,
                r * 0.57,
                0.22,
                0.40,
            ),
        );
        crate::common::spawn_light(
            world,
            directional_light([0.15, -1.0, 0.1], [0.07, 0.09, 0.14], 0.3),
        );
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.0, 0.0, 0.0],
            intensity: 0.0,
        });
        renderer.set_clear_color([0.02, 0.02, 0.04, 1.0]);

        Self {
            cam_pos: Vec3::new(1.5 * 0.55, 1.5 * 0.28, 1.5 * 1.55) + focus,
            cam_yaw: std::f32::consts::PI + 0.1,
            cam_pitch: -0.1,
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
        self.cam_pitch = (self.cam_pitch - input.mouse_delta.1 * LOOK_SENS).clamp(-1.5, 1.5);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let fwd = Vec3::new(sy * cp, sp, -cy * cp);
        let right = Vec3::new(cy, 0.0, sy);

        if input.keys.contains(&helio_wasm::KeyCode::KeyW) {
            self.cam_pos += fwd * SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyS) {
            self.cam_pos -= fwd * SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyA) {
            self.cam_pos -= right * SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyD) {
            self.cam_pos += right * SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::Space) {
            self.cam_pos.y += SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::ShiftLeft) {
            self.cam_pos.y -= SPEED * dt;
        }

        Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.05,
            100.0,
        )
    }
}
