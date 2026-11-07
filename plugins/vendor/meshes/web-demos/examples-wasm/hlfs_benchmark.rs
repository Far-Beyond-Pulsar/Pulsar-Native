//! Browser WebGPU benchmark for Helio's hierarchical light-field graph.
//!
//! A Cornell-box-like room, several occluders, and three coloured point lights
//! exercise the compute importance/injection/propagation path plus raster output.

use std::sync::Arc;

use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::{Entity, SceneDb};

use crate::common::{box_mesh, insert_object, make_material, point_light};

const LOOK_SENS: f32 = 0.0020;
const SPEED: f32 = 4.0;

const LIGHT_BASE: &[([f32; 3], [f32; 3], f32, f32)] = &[
    ([0.0, 4.8, 0.0], [1.0, 1.0, 1.0], 18.0, 6.0),
    ([-3.5, 3.0, -2.0], [1.0, 0.7, 0.4], 12.0, 5.0),
    ([3.5, 3.0, 2.0], [0.4, 0.7, 1.0], 12.0, 5.0),
];

pub struct Demo {
    light_ids: [Entity; 3],
    intensity: f32,
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — HLFS Compute Lighting"
    }

    fn init(
        renderer: &mut Renderer,
        scene_db: &mut SceneDb,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        _w: u32,
        _h: u32,
    ) -> Self {
        let world = &mut scene_db.world;
        let mat_white = crate::common::spawn_material(
            world,
            make_material([0.9, 0.9, 0.9, 1.0], 0.9, 0.0, [0.0; 3], 0.0),
        );
        let mat_red = crate::common::spawn_material(
            world,
            make_material([0.8, 0.1, 0.1, 1.0], 0.9, 0.0, [0.0; 3], 0.0),
        );
        let mat_green = crate::common::spawn_material(
            world,
            make_material([0.1, 0.7, 0.1, 1.0], 0.9, 0.0, [0.0; 3], 0.0),
        );
        let mat_cube = crate::common::spawn_material(
            world,
            make_material([0.8, 0.78, 0.72, 1.0], 0.85, 0.0, [0.0; 3], 0.0),
        );

        let mut add_box = |cx: f32, cy: f32, cz: f32, hx: f32, hy: f32, hz: f32, mat| {
            let mesh = crate::common::spawn_mesh(world, box_mesh([cx, cy, cz], [hx, hy, hz]));
            let _ = insert_object(
                world,
                mesh,
                mat,
                glam::Mat4::IDENTITY,
                (hx * hx + hy * hy + hz * hz).sqrt(),
            );
        };
        add_box(0.0, -0.05, 0.0, 5.0, 0.05, 5.0, mat_white);
        add_box(0.0, 5.05, 0.0, 5.0, 0.05, 5.0, mat_white);
        add_box(0.0, 2.5, -5.05, 5.0, 2.5, 0.05, mat_white);
        add_box(0.0, 2.5, 5.05, 5.0, 2.5, 0.05, mat_white);
        add_box(5.05, 2.5, 0.0, 0.05, 2.5, 5.0, mat_green);
        add_box(-5.05, 2.5, 0.0, 0.05, 2.5, 5.0, mat_red);
        add_box(-2.0, 0.5, -2.0, 0.5, 0.5, 0.5, mat_cube);
        add_box(2.0, 0.5, 2.0, 0.5, 0.5, 0.5, mat_cube);
        add_box(0.0, 0.7, 0.0, 0.7, 0.7, 0.7, mat_cube);
        add_box(-3.0, 1.0, 1.5, 1.0, 1.0, 1.0, mat_cube);
        add_box(3.0, 0.6, -1.5, 0.6, 0.6, 0.6, mat_cube);

        let mut ids: Vec<pulsar_scenedb::Entity> = LIGHT_BASE
            .iter()
            .map(|&(position, color, intensity, range)| {
                crate::common::spawn_light(world, point_light(position, color, intensity, range))
            })
            .collect();
        let light_ids = [ids.remove(0), ids.remove(0), ids.remove(0)];

        renderer.set_feature_live(helio::AmbientLight {
            color: [0.02, 0.02, 0.03],
            intensity: 1.0,
        });
        let graph = helio_mesh_host::presets::hlfs(helio::PassBuildContext {
            device: &device,
            queue: &queue,
            config: renderer.renderer_config(),
            debug_state: renderer.debug_state(),
            camera_buffer: renderer.camera_buf(),
            debug_camera_buffer: renderer.debug_camera_buf(),
            cull_stats_buffer: renderer.cull_stats_buf(),
            owns_device: true,
            scene_db: renderer.scene_db(),
        });
        renderer.set_graph(graph);

        Self {
            light_ids,
            intensity: 1.0,
            cam_pos: Vec3::new(0.0, 2.5, 8.0),
            cam_yaw: 0.0,
            cam_pitch: 0.0,
        }
    }

    fn update(
        &mut self,
        renderer: &mut Renderer,
        dt: f32,
        _elapsed: f32,
        input: &InputState,
    ) -> Camera {
        self.cam_yaw += input.mouse_delta.0 * LOOK_SENS;
        self.cam_pitch = (self.cam_pitch - input.mouse_delta.1 * LOOK_SENS).clamp(-1.5, 1.5);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let forward = Vec3::new(sy * cp, sp, -cy * cp);
        let right = Vec3::new(cy, 0.0, sy);

        if input.keys.contains(&helio_wasm::KeyCode::KeyW) {
            self.cam_pos += forward * SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyS) {
            self.cam_pos -= forward * SPEED * dt;
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
        if input.keys.contains(&helio_wasm::KeyCode::Equal) {
            self.intensity = (self.intensity + 0.5 * dt).min(5.0);
        }
        if input.keys.contains(&helio_wasm::KeyCode::Minus) {
            self.intensity = (self.intensity - 0.5 * dt).max(0.1);
        }

        for (id, &(position, color, intensity, range)) in
            self.light_ids.iter().zip(LIGHT_BASE.iter())
        {
            let _ = renderer.scene().update_light(
                *id,
                point_light(position, color, intensity * self.intensity, range),
            );
        }

        Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            renderer.output_width() as f32 / renderer.output_height().max(1) as f32,
            0.1,
            50.0,
        )
    }
}
