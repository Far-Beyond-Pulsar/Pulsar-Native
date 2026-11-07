//! WASM twin of `outdoor_rocks` — procedural rock clusters with embedded ship.
//!
//! The native version loads three rock FBX meshes from `3d/` and scatters them
//! as classic instanced meshes. On WASM, file I/O is unavailable, so rocks are
//! replaced with randomised stretched cubes and the ship is loaded from
//! `include_bytes!("../../examples/assets/test.fbx")`.
//!
//! Controls:
//!   WASD / Space / Shift — fly
//!   Mouse                — look (click to grab cursor)
//!   Q / E                — rotate sun

use std::sync::Arc;

use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::{Camera, Renderer};
use helio_asset_compat::{load_scene_bytes_with_config, upload_scene_materials, LoadConfig};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::{Entity, SceneDb};

use crate::common::{cube_mesh, directional_light, insert_object, make_material, point_light};

const SHIP_BYTES: &[u8] = include_bytes!("../../examples/assets/test.fbx");

const ROCK_COUNT: usize = 90;
const FIELD_RADIUS: f32 = 80.0;
const LOOK_SENS: f32 = 0.002;
const FLY_SPEED: f32 = 28.0;

fn lcg(seed: &mut u64) -> f32 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    ((*seed >> 33) as f32) / (u32::MAX as f32)
}
fn rand_s(seed: &mut u64) -> f32 {
    lcg(seed) * 2.0 - 1.0
}

pub struct Demo {
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    sun_light_id: Entity,
    sun_angle: f32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Outdoor Rocks"
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
        // Ground plane
        let ground_mat = crate::common::spawn_material(
            world,
            make_material([0.30, 0.27, 0.22, 1.0], 0.85, 0.0, [0.0; 3], 0.0),
        );
        let ground = crate::common::spawn_mesh(
            world,
            crate::common::box_mesh([0.0, -1.0, 0.0], [200.0, 0.4, 200.0]),
        );
        let _ = insert_object(world, ground, ground_mat, Mat4::IDENTITY, 10.0);

        // Rock materials
        let mats = [
            crate::common::spawn_material(
                world,
                make_material([0.20, 0.18, 0.14, 1.0], 0.90, 0.0, [0.0; 3], 0.0),
            ),
            crate::common::spawn_material(
                world,
                make_material([0.28, 0.24, 0.20, 1.0], 0.80, 0.05, [0.0; 3], 0.0),
            ),
            crate::common::spawn_material(
                world,
                make_material([0.15, 0.14, 0.12, 1.0], 0.95, 0.0, [0.0; 3], 0.0),
            ),
        ];
        let rock_mesh = crate::common::spawn_mesh(world, cube_mesh([0.0, 0.0, 0.0], 0.5));

        let mut seed: u64 = 0xB00B1E5_CAFEBABE;
        for i in 0..ROCK_COUNT {
            let angle = lcg(&mut seed) * std::f32::consts::TAU;
            let dist = 4.0 + lcg(&mut seed) * FIELD_RADIUS;
            let pos = Vec3::new(angle.cos() * dist, 0.0, angle.sin() * dist);
            let w = 0.4 + lcg(&mut seed) * 2.8;
            let h = 0.3 + lcg(&mut seed) * 1.6;
            let d = 0.4 + lcg(&mut seed) * 2.4;
            let rot = Quat::from_euler(
                EulerRot::XYZ,
                rand_s(&mut seed) * 0.6,
                rand_s(&mut seed) * std::f32::consts::PI,
                rand_s(&mut seed) * 0.5,
            );
            let t = Mat4::from_scale_rotation_translation(
                Vec3::new(w, h, d),
                rot,
                pos + Vec3::Y * (h * 0.5 - 0.8),
            );
            let mat = mats[i % mats.len()];
            let _ = insert_object(world, rock_mesh, mat, t, w.max(h).max(d));
        }

        // Embedded ship parked near origin
        let base_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        match load_scene_bytes_with_config(
            SHIP_BYTES,
            "fbx",
            Some(base_dir.as_path()),
            LoadConfig::default().with_uv_flip(false),
        ) {
            Ok(scene) => {
                let mat_ids = upload_scene_materials(world, &scene);
                for mesh in &scene.meshes {
                    let mesh_id = crate::common::spawn_mesh(
                        world,
                        helio_mesh_api::MeshUpload {
                            vertices: mesh.vertices.clone(),
                            indices: mesh.indices.clone(),
                        },
                    );
                    let mat_id = mesh
                        .material_index
                        .and_then(|i| mat_ids.get(i).copied())
                        .or_else(|| mat_ids.first().copied())
                        .unwrap_or_else(|| {
                            crate::common::spawn_material(
                                world,
                                make_material([0.40, 0.40, 0.48, 1.0], 0.3, 0.7, [0.0; 3], 0.0),
                            )
                        });
                    let t = Mat4::from_scale_rotation_translation(
                        Vec3::ONE,
                        Quat::IDENTITY,
                        Vec3::new(8.0, 2.5, 0.0),
                    );
                    let _ = insert_object(world, mesh_id, mat_id, t, 3.0);
                }
            }
            Err(e) => log::warn!("ship FBX unavailable: {e:?}"),
        }

        // Lighting
        let sun_light_id = crate::common::spawn_light(
            world,
            directional_light([-0.5, -0.8, 0.3], [1.0, 0.97, 0.88], 2.2),
        );
        crate::common::spawn_light(
            world,
            directional_light([0.3, 0.6, -0.8], [0.3, 0.4, 0.6], 0.05),
        );
        // Scatter a few warm rock pool lights
        let mut light_seed: u64 = 0xFEEDFACE;
        for _ in 0..6 {
            let a = lcg(&mut light_seed) * std::f32::consts::TAU;
            let d = 5.0 + lcg(&mut light_seed) * 25.0;
            let p = Vec3::new(a.cos() * d, 1.5, a.sin() * d);
            crate::common::spawn_light(
                world,
                point_light(p.to_array(), [1.0, 0.85, 0.60], 4.0, 18.0),
            );
        }
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.4, 0.42, 0.48],
            intensity: 0.12,
        });

        Self {
            cam_pos: Vec3::new(0.0, 6.0, 20.0),
            cam_yaw: 0.0,
            cam_pitch: -0.25,
            sun_light_id,
            sun_angle: 0.8,
        }
    }

    fn update(
        &mut self,
        renderer: &mut Renderer,
        dt: f32,
        _elapsed: f32,
        input: &InputState,
    ) -> Camera {
        // Sun rotation
        if input.keys.contains(&helio_wasm::KeyCode::KeyQ) {
            self.sun_angle -= dt * 0.7;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyE) {
            self.sun_angle += dt * 0.7;
        }
        let (s, c) = self.sun_angle.sin_cos();
        let sun_dir = Vec3::new(s * 0.7, -c.abs().max(0.15), s.abs() * 0.3 - 0.5).normalize();
        let sun_col = Vec3::new(
            1.0,
            (c * 0.5 + 0.5) * 0.97 + 0.03,
            (c * 0.5 + 0.5) * 0.85 + 0.03,
        );
        let _ = renderer.scene().update_light(
            self.sun_light_id,
            directional_light(sun_dir.to_array(), sun_col.to_array(), 2.2),
        );

        // Fly camera
        let (dx, dy) = input.mouse_delta;
        self.cam_yaw -= dx * LOOK_SENS;
        self.cam_pitch = (self.cam_pitch - dy * LOOK_SENS).clamp(-1.48, 1.48);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let forward = Vec3::new(sy * cp, sp, -cy * cp);
        let right = Vec3::new(cy, 0.0, sy);

        if input.keys.contains(&helio_wasm::KeyCode::KeyW) {
            self.cam_pos += forward * FLY_SPEED * dt;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyS) {
            self.cam_pos -= forward * FLY_SPEED * dt;
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
            self.cam_pos + forward,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.2,
            2000.0,
        )
    }
}
