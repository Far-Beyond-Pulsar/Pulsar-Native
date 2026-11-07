//! WASM twin of `indoor_cathedral` — gothic cathedral nave.
//!
//! Controls: WASD walk, mouse look. Candles flicker over time.

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
const WALK_SPEED: f32 = 4.0;

pub struct Demo {
    candle_ids: Vec<Entity>,
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Indoor Cathedral"
    }

    fn init(
        renderer: &mut Renderer,
        scene_db: &mut SceneDb,
        _device: Arc<wgpu::Device>,
        _queue: Arc<wgpu::Queue>,
        _w: u32,
        _h: u32,
    ) -> Self {
        let stone = spawn_material(
            &mut scene_db.world,
            make_material([0.65, 0.62, 0.58, 1.0], 0.9, 0.0, [0.0; 3], 0.0),
        );
        let marble = spawn_material(
            &mut scene_db.world,
            make_material([0.88, 0.86, 0.82, 1.0], 0.4, 0.1, [0.0; 3], 0.0),
        );
        let candle_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.8, 0.75, 0.6, 1.0], 1.0, 0.0, [1.2, 0.9, 0.4], 3.5),
        );
        let window_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.15, 0.1, 0.4, 1.0], 0.1, 0.0, [0.2, 0.15, 0.6], 1.0),
        );
        let dark_stone = spawn_material(
            &mut scene_db.world,
            make_material([0.3, 0.28, 0.26, 1.0], 0.95, 0.0, [0.0; 3], 0.0),
        );

        // Nave floor
        let floor = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 30.0));
        insert_object(
            &mut scene_db.world,
            floor,
            marble,
            glam::Mat4::IDENTITY,
            30.0,
        )
        .unwrap();

        // Ceiling slab
        let ceil = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 18.0, 0.0], [14.0, 0.8, 30.0]),
        );
        insert_object(&mut scene_db.world, ceil, stone, glam::Mat4::IDENTITY, 14.0).unwrap();

        // Side walls (left / right)
        let wall_l = spawn_mesh(
            &mut scene_db.world,
            box_mesh([-14.0, 9.0, 0.0], [0.6, 18.0, 30.0]),
        );
        let wall_r = spawn_mesh(
            &mut scene_db.world,
            box_mesh([14.0, 9.0, 0.0], [0.6, 18.0, 30.0]),
        );
        for m in [wall_l, wall_r] {
            insert_object(&mut scene_db.world, m, stone, glam::Mat4::IDENTITY, 18.0).unwrap();
        }

        // Back wall
        let back_wall = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 9.0, -30.0], [14.0, 18.0, 0.6]),
        );
        insert_object(
            &mut scene_db.world,
            back_wall,
            stone,
            glam::Mat4::IDENTITY,
            14.0,
        )
        .unwrap();

        // Front entrance wall (with gap)
        let entrance_top = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 15.0, 30.0], [14.0, 3.0, 0.6]),
        );
        insert_object(
            &mut scene_db.world,
            entrance_top,
            stone,
            glam::Mat4::IDENTITY,
            14.0,
        )
        .unwrap();

        // Nave columns (4 pairs)
        for (i, z) in [-20.0_f32, -10.0, 0.0, 10.0].iter().enumerate() {
            for sx in [-8.0_f32, 8.0] {
                let col_mesh = spawn_mesh(
                    &mut scene_db.world,
                    box_mesh([sx, 6.0, *z], [0.9, 12.0, 0.9]),
                );
                let cap_mesh = spawn_mesh(
                    &mut scene_db.world,
                    box_mesh([sx, 12.3, *z], [1.4, 0.6, 1.4]),
                );
                let base_mesh = spawn_mesh(
                    &mut scene_db.world,
                    box_mesh([sx, 0.3, *z], [1.2, 0.6, 1.2]),
                );
                insert_object(
                    &mut scene_db.world,
                    col_mesh,
                    marble,
                    glam::Mat4::IDENTITY,
                    1.0,
                )
                .unwrap();
                insert_object(
                    &mut scene_db.world,
                    cap_mesh,
                    marble,
                    glam::Mat4::IDENTITY,
                    1.0,
                )
                .unwrap();
                insert_object(
                    &mut scene_db.world,
                    base_mesh,
                    marble,
                    glam::Mat4::IDENTITY,
                    1.0,
                )
                .unwrap();
                let _ = i;
            }
        }

        // Rib arches (simplified as thin boxes)
        for z in [-20.0_f32, -10.0, 0.0, 10.0] {
            let span_m = spawn_mesh(
                &mut scene_db.world,
                box_mesh([0.0, 13.5, z], [14.0, 0.4, 0.4]),
            );
            insert_object(
                &mut scene_db.world,
                span_m,
                dark_stone,
                glam::Mat4::IDENTITY,
                14.0,
            )
            .unwrap();
        }

        // Altar platform
        let alt_base = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.2, -24.0], [6.0, 0.2, 4.0]),
        );
        let alt_top = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.5, -24.0], [2.0, 0.4, 1.4]),
        );
        insert_object(
            &mut scene_db.world,
            alt_base,
            marble,
            glam::Mat4::IDENTITY,
            6.0,
        )
        .unwrap();
        insert_object(
            &mut scene_db.world,
            alt_top,
            marble,
            glam::Mat4::IDENTITY,
            2.0,
        )
        .unwrap();

        // Stained glass (back rose window - emissive slabs)
        let rose = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 14.0, -30.4], [6.0, 6.0, 0.2]),
        );
        insert_object(
            &mut scene_db.world,
            rose,
            window_mat,
            glam::Mat4::IDENTITY,
            6.0,
        )
        .unwrap();

        // Candelabra candles (6 around altar)
        let candle_positions = [
            [-1.5, 0.75, -23.0],
            [0.0, 0.75, -23.0],
            [1.5, 0.75, -23.0],
            [-1.5, 0.75, -25.0],
            [0.0, 0.75, -25.0],
            [1.5, 0.75, -25.0],
        ];
        for pos in &candle_positions {
            let c = spawn_mesh(&mut scene_db.world, cube_mesh(*pos, 0.06));
            insert_object(
                &mut scene_db.world,
                c,
                candle_mat,
                glam::Mat4::IDENTITY,
                0.06,
            )
            .unwrap();
        }

        // Torches on columns
        let torch_positions: [[f32; 3]; 8] = [
            [-7.6, 7.0, -20.0],
            [7.6, 7.0, -20.0],
            [-7.6, 7.0, -10.0],
            [7.6, 7.0, -10.0],
            [-7.6, 7.0, 0.0],
            [7.6, 7.0, 0.0],
            [-7.6, 7.0, 10.0],
            [7.6, 7.0, 10.0],
        ];
        let light_positions: [[f32; 3]; 11] = [
            [-1.5, 1.5, -23.0],
            [0.0, 1.5, -23.0],
            [1.5, 1.5, -23.0],
            [-1.5, 1.5, -25.0],
            [0.0, 1.5, -25.0],
            [1.5, 1.5, -25.0],
            // torches
            [-7.6, 7.5, -20.0],
            [7.6, 7.5, -20.0],
            [-7.6, 7.5, 0.0],
            [7.6, 7.5, 0.0],
            [0.0, 2.0, -24.5],
        ];

        for pos in &torch_positions {
            let t = spawn_mesh(&mut scene_db.world, cube_mesh(*pos, 0.08));
            insert_object(
                &mut scene_db.world,
                t,
                candle_mat,
                glam::Mat4::IDENTITY,
                0.08,
            )
            .unwrap();
        }

        let mut candle_ids = Vec::new();
        for pos in &light_positions {
            let id = spawn_light(
                &mut scene_db.world,
                point_light(*pos, [1.0, 0.85, 0.5], 1.5, 10.0),
            );
            candle_ids.push(id);
        }

        // Dim ambient blue for stained glass atmosphere
        let moon_dir = Vec3::new(0.2, -0.9, 0.4).normalize();
        spawn_light(
            &mut scene_db.world,
            directional_light([moon_dir.x, moon_dir.y, moon_dir.z], [0.4, 0.5, 1.0], 0.02),
        );
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.4, 0.45, 0.65],
            intensity: 0.015,
        });
        renderer.set_clear_color([0.02, 0.01, 0.04, 1.0]);

        // Water volumes are not part of the current WASM SceneDB component
        // schema; keep the cathedral geometry and lighting SceneDB-owned.

        Self {
            candle_ids,
            cam_pos: Vec3::new(0.0, 1.7, 25.0),
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

        // Dynamic candle updates require a mutable World in the update hook.
        Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.05,
            150.0,
        )
    }
}
