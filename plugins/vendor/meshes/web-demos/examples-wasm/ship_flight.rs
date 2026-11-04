//! WASM twin of `ship_flight` — 6DoF spaceship through an asteroid field.
//!
//! Controls: WASD/Space/Shift thrust, Q/E roll, mouse yaw/pitch.

use std::sync::Arc;

use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::{Camera, Renderer};
use helio_asset_compat::{load_scene_bytes_with_config, upload_scene_materials, LoadConfig};
use helio_wasm::{HelioWasmApp, InputState};
use pulsar_scenedb::{Entity, SceneDb};

use crate::common::{
    cube_mesh, directional_light, insert_object, make_material, point_light, spot_light,
};

const EMBEDDED_BYTES: &[u8] = include_bytes!("../../examples/assets/test.fbx");

const LOOK_SENS: f32 = 0.0024;
const ROLL_SPEED: f32 = 1.9;
const SHIP_POSITION_LAG: f32 = 12.0;
const SHIP_ROTATION_LAG: f32 = 14.0;
const CAMERA_POSITION_LAG: f32 = 8.5;
const CAMERA_TARGET_LAG: f32 = 9.5;
const CAMERA_UP_LAG: f32 = 10.5;
const YAW_THRUST: f32 = 9.0;
const PITCH_THRUST: f32 = 8.0;
const ROLL_THRUST: f32 = 2.6;
const ANGULAR_DAMPING: f32 = 9.0;
const FORWARD_THRUST_SCALE: f32 = 0.9;
const REVERSE_THRUST_SCALE: f32 = 0.5;
const STRAFE_THRUST_SCALE: f32 = 0.62;
const LIFT_THRUST_SCALE: f32 = 0.58;
const FORWARD_DRAG: f32 = 0.28;
const LATERAL_DRAG: f32 = 3.4;
const VERTICAL_DRAG: f32 = 2.9;

const ASTEROID_COUNT: usize = 10_000;
const LOCAL_ASTEROID_COUNT: usize = 620;

const MESH_BASE_ROT: Quat = Quat::from_xyzw(
    -std::f32::consts::FRAC_1_SQRT_2,
    0.0,
    0.0,
    std::f32::consts::FRAC_1_SQRT_2,
);

// ── Helpers ──────────────────────────────────────────────────────────────────

fn lcg(seed: &mut u64) -> f32 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    ((*seed >> 33) as f32) / (u32::MAX as f32)
}
fn rand_s(seed: &mut u64) -> f32 {
    lcg(seed) * 2.0 - 1.0
}
fn follow(strength: f32, dt: f32) -> f32 {
    1.0 - (-strength * dt).exp()
}

// ── Ship state ────────────────────────────────────────────────────────────────

struct ShipState {
    ids: Vec<Entity>,
    radius: f32,
    pos: Vec3,
    quat: Quat,
    render_pos: Vec3,
    render_quat: Quat,
    velocity: Vec3,
    angular_velocity: Vec3,
    thrusting: bool,
    thrust_accel: f32,
    max_speed: f32,
    engine_light: Entity,
    spotlight_left: Entity,
    spotlight_right: Entity,
    hull_port: Entity,
    hull_starboard: Entity,
    hull_top: Entity,
    hull_belly: Entity,
}

impl ShipState {
    fn forward(&self) -> Vec3 {
        self.quat * -Vec3::Z
    }
    fn render_forward(&self) -> Vec3 {
        self.render_quat * -Vec3::Z
    }
    fn render_right(&self) -> Vec3 {
        self.render_quat * Vec3::X
    }
    fn render_up(&self) -> Vec3 {
        self.render_quat * Vec3::Y
    }

    fn desired_cam_pos(&self) -> Vec3 {
        self.render_pos - self.render_forward() * self.radius * 3.2
            + self.render_up() * self.radius * 0.95
    }
    fn desired_cam_target(&self) -> Vec3 {
        self.render_pos
            + self.render_forward() * self.radius * 1.15
            + self.render_up() * self.radius * 0.18
    }

    fn update_visual(&mut self, dt: f32) {
        self.render_pos = self
            .render_pos
            .lerp(self.pos, follow(SHIP_POSITION_LAG, dt));
        self.render_quat = self
            .render_quat
            .slerp(self.quat, follow(SHIP_ROTATION_LAG, dt))
            .normalize();
    }

    fn push_transforms(&self, renderer: &mut Renderer) {
        let t = Mat4::from_rotation_translation(self.render_quat * MESH_BASE_ROT, self.render_pos);
        for &id in &self.ids {
            let _ = renderer.scene().update_object_transform(id, t);
        }
    }

    fn update_lights(&self, renderer: &mut Renderer) {
        let fwd = self.render_forward();
        let right = self.render_right();
        let up = self.render_up();
        let r = self.radius;

        let sl_r = r * 15.0;
        let sl_i = 35.0;
        let l_pos = self.render_pos + right * (-r * 0.4) + up * (r * 0.15) + fwd * (-r * 0.9);
        let rr_pos = self.render_pos + right * (r * 0.4) + up * (r * 0.15) + fwd * (-r * 0.9);
        let _ = renderer.scene().update_light(
            self.spotlight_left,
            spot_light(
                l_pos.to_array(),
                fwd.to_array(),
                [1.0, 1.0, 0.95],
                sl_i,
                sl_r,
                25_f32.to_radians(),
                35_f32.to_radians(),
            ),
        );
        let _ = renderer.scene().update_light(
            self.spotlight_right,
            spot_light(
                rr_pos.to_array(),
                fwd.to_array(),
                [1.0, 1.0, 0.95],
                sl_i,
                sl_r,
                25_f32.to_radians(),
                35_f32.to_radians(),
            ),
        );

        let hi = 12.0;
        let hr = r * 4.0;
        let _ = renderer.scene().update_light(
            self.hull_port,
            point_light(
                (self.render_pos + right * (-r * 0.75) + up * (r * 0.2)).to_array(),
                [1.0, 0.1, 0.1],
                hi,
                hr,
            ),
        );
        let _ = renderer.scene().update_light(
            self.hull_starboard,
            point_light(
                (self.render_pos + right * (r * 0.75) + up * (r * 0.2)).to_array(),
                [0.1, 1.0, 0.1],
                hi,
                hr,
            ),
        );
        let _ = renderer.scene().update_light(
            self.hull_top,
            point_light(
                (self.render_pos + up * (r * 0.5) + fwd * (r * 0.2)).to_array(),
                [1.0, 1.0, 1.0],
                hi * 0.8,
                hr,
            ),
        );
        let _ = renderer.scene().update_light(
            self.hull_belly,
            point_light(
                (self.render_pos - up * (r * 0.4) + fwd * (r * 0.2)).to_array(),
                [0.4, 0.6, 1.0],
                hi * 0.7,
                hr,
            ),
        );

        let glow = if self.thrusting { 9.0 } else { 1.8 };
        let _ = renderer.scene().update_light(
            self.engine_light,
            point_light(
                (self.render_pos - fwd * (r * 0.8)).to_array(),
                [0.35, 0.65, 1.0],
                glow,
                r * 3.5,
            ),
        );
    }
}

// ── Main demo ─────────────────────────────────────────────────────────────────

pub struct Demo {
    ship: ShipState,
    cam_pos: Vec3,
    cam_target: Vec3,
    cam_up: Vec3,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Ship Flight"
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
        let base_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");

        // Load ship mesh
        let (ship_ids, ship_radius, ship_thrust) = match load_scene_bytes_with_config(
            EMBEDDED_BYTES,
            "fbx",
            Some(base_dir.as_path()),
            LoadConfig::default().with_uv_flip(false),
        ) {
            Ok(scene) => {
                // Compute center
                let mut min = Vec3::splat(f32::INFINITY);
                let mut max = Vec3::splat(f32::NEG_INFINITY);
                for mesh in &scene.meshes {
                    for v in &mesh.vertices {
                        let p = Vec3::from(v.position);
                        min = min.min(p);
                        max = max.max(p);
                    }
                }
                let center = (min + max) * 0.5;
                let extents = (max - min).max(Vec3::splat(0.1));
                let radius = (extents.length() * 0.5).max(1.0);

                let mat_ids = upload_scene_materials(world, &scene);
                let ids: Vec<Entity> = scene
                    .meshes
                    .iter()
                    .map(|mesh| {
                        let mut vertices = mesh.vertices.clone();
                        for v in &mut vertices {
                            v.position = (Vec3::from(v.position) - center).to_array();
                        }
                        let mesh_id = crate::common::spawn_mesh(
                            world,
                            helio_mesh_api::MeshUpload {
                                vertices,
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
                                    make_material(
                                        [0.25, 0.40, 0.70, 1.0],
                                        0.25,
                                        0.85,
                                        [0.0; 3],
                                        0.0,
                                    ),
                                )
                            });
                        insert_object(world, mesh_id, mat_id, Mat4::IDENTITY, radius).unwrap()
                    })
                    .collect();
                let thrust = (radius * 120.0).clamp(60.0, 2400.0);
                (ids, radius, thrust)
            }
            Err(e) => {
                log::warn!("ship FBX load failed: {e:?}, using fallback cube");
                let mat = crate::common::spawn_material(
                    world,
                    make_material([0.25, 0.40, 0.70, 1.0], 0.25, 0.85, [0.0; 3], 0.0),
                );
                let mesh = crate::common::spawn_mesh(world, cube_mesh([0.0, 0.0, 0.0], 2.0));
                let id = insert_object(world, mesh, mat, Mat4::IDENTITY, 2.0).unwrap();
                (vec![id], 2.0, 240.0)
            }
        };

        // Asteroid field
        let rocky = crate::common::spawn_material(
            world,
            make_material([0.15, 0.12, 0.09, 1.0], 0.90, 0.0, [0.0; 3], 0.0),
        );
        let dark = crate::common::spawn_material(
            world,
            make_material([0.09, 0.09, 0.11, 1.0], 0.70, 0.25, [0.0; 3], 0.0),
        );
        let cube = crate::common::spawn_mesh(world, cube_mesh([0.0, 0.0, 0.0], 0.5));

        let field_radius = 12000.0_f32;
        let local_radius = (ship_radius * 40.0).clamp(120.0, 420.0);
        let min_size = ship_radius * 0.1;

        let mut seed: u64 = 0xCAFE_BABE_1234_5678;
        let mut spawn =
            |renderer: &mut Renderer, seed: &mut u64, i: usize, dist: f32, bias: f32| {
                let theta = lcg(seed) * std::f32::consts::TAU;
                let phi = rand_s(seed).asin();
                let pos = Vec3::new(
                    dist * phi.cos() * theta.cos(),
                    dist * phi.sin(),
                    dist * phi.cos() * theta.sin(),
                );
                let base = min_size * bias * (1.0 + lcg(seed) * 9.0);
                let scale = Vec3::new(
                    base * (0.6 + lcg(seed) * 0.8),
                    base * (0.5 + lcg(seed) * 0.7),
                    base * (0.6 + lcg(seed) * 0.8),
                );
                let rot = Quat::from_euler(
                    EulerRot::XYZ,
                    rand_s(seed) * std::f32::consts::PI,
                    rand_s(seed) * std::f32::consts::PI,
                    rand_s(seed) * std::f32::consts::PI,
                );
                let mat = if i % 3 == 0 { dark } else { rocky };
                let t = Mat4::from_scale_rotation_translation(scale, rot, pos);
                let _ = insert_object(world, cube, mat, t, base);
            };
        for i in 0..LOCAL_ASTEROID_COUNT {
            let d = ship_radius * 10.0 + lcg(&mut seed) * local_radius;
            spawn(renderer, &mut seed, i, d, 0.85);
        }
        for i in 0..ASTEROID_COUNT {
            let d = field_radius * (0.12 + lcg(&mut seed) * 0.88);
            spawn(renderer, &mut seed, i + LOCAL_ASTEROID_COUNT, d, 1.0);
        }

        // Ship lights
        let engine_light = crate::common::spawn_light(
            world,
            point_light([0.0; 3], [0.35, 0.65, 1.0], 1.8, ship_radius * 3.5),
        );
        let spotlight_left = crate::common::spawn_light(
            world,
            spot_light(
                [0.0; 3],
                [0.0, 0.0, -1.0],
                [1.0, 1.0, 0.95],
                35.0,
                ship_radius * 15.0,
                25_f32.to_radians(),
                35_f32.to_radians(),
            ),
        );
        let spotlight_right = crate::common::spawn_light(
            world,
            spot_light(
                [0.0; 3],
                [0.0, 0.0, -1.0],
                [1.0, 1.0, 0.95],
                35.0,
                ship_radius * 15.0,
                25_f32.to_radians(),
                35_f32.to_radians(),
            ),
        );
        let hull_port = crate::common::spawn_light(
            world,
            point_light([0.0; 3], [1.0, 0.1, 0.1], 12.0, ship_radius * 4.0),
        );
        let hull_starboard = crate::common::spawn_light(
            world,
            point_light([0.0; 3], [0.1, 1.0, 0.1], 12.0, ship_radius * 4.0),
        );
        let hull_top = crate::common::spawn_light(
            world,
            point_light([0.0; 3], [1.0, 1.0, 1.0], 9.6, ship_radius * 4.0),
        );
        let hull_belly = crate::common::spawn_light(
            world,
            point_light([0.0; 3], [0.4, 0.6, 1.0], 8.4, ship_radius * 4.0),
        );

        // Distant stars (directional)
        crate::common::spawn_light(
            world,
            directional_light([-0.5, -0.4, 0.8], [1.0, 0.98, 0.95], 0.8),
        );
        crate::common::spawn_light(
            world,
            directional_light([0.6, 0.2, -0.7], [0.2, 0.25, 0.4], 0.04),
        );
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.04, 0.05, 0.08],
            intensity: 0.003,
        });
        renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);

        let initial_pos = Vec3::new(0.0, 0.0, 0.0);
        let ship = ShipState {
            ids: ship_ids,
            radius: ship_radius,
            pos: initial_pos,
            quat: Quat::IDENTITY,
            render_pos: initial_pos,
            render_quat: Quat::IDENTITY,
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            thrusting: false,
            thrust_accel: ship_thrust,
            max_speed: ship_thrust * 0.6,
            engine_light,
            spotlight_left,
            spotlight_right,
            hull_port,
            hull_starboard,
            hull_top,
            hull_belly,
        };

        let cam_pos = initial_pos + Vec3::new(0.0, ship_radius * 0.95, ship_radius * 3.2);
        let cam_target = initial_pos;
        let cam_up = Vec3::Y;

        Self {
            ship,
            cam_pos,
            cam_target,
            cam_up,
        }
    }

    fn update(
        &mut self,
        renderer: &mut Renderer,
        dt: f32,
        _elapsed: f32,
        input: &InputState,
    ) -> Camera {
        // Ship physics
        let yaw = input.mouse_delta.0 * LOOK_SENS;
        let pitch = input.mouse_delta.1 * LOOK_SENS;

        let mut roll_input = 0.0_f32;
        if input.keys.contains(&helio_wasm::KeyCode::KeyQ) {
            roll_input += ROLL_SPEED;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyE) {
            roll_input -= ROLL_SPEED;
        }

        self.ship.angular_velocity += Vec3::new(
            -pitch * PITCH_THRUST,
            -yaw * YAW_THRUST,
            roll_input * ROLL_THRUST * dt,
        );
        self.ship.angular_velocity /= 1.0 + ANGULAR_DAMPING * dt;

        let local_rot = Quat::from_euler(
            EulerRot::XYZ,
            self.ship.angular_velocity.x * dt,
            self.ship.angular_velocity.y * dt,
            self.ship.angular_velocity.z * dt,
        );
        self.ship.quat = (self.ship.quat * local_rot).normalize();

        let mut local_vel = self.ship.quat.conjugate() * self.ship.velocity;
        let mut thrusting = false;
        let ta = self.ship.thrust_accel;

        if input.keys.contains(&helio_wasm::KeyCode::KeyW) {
            local_vel.z -= ta * FORWARD_THRUST_SCALE * dt;
            thrusting = true;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyS) {
            local_vel.z += ta * REVERSE_THRUST_SCALE * dt;
            thrusting = true;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyA) {
            local_vel.x -= ta * STRAFE_THRUST_SCALE * dt;
            thrusting = true;
        }
        if input.keys.contains(&helio_wasm::KeyCode::KeyD) {
            local_vel.x += ta * STRAFE_THRUST_SCALE * dt;
            thrusting = true;
        }
        if input.keys.contains(&helio_wasm::KeyCode::Space) {
            local_vel.y += ta * LIFT_THRUST_SCALE * dt;
            thrusting = true;
        }
        if input.keys.contains(&helio_wasm::KeyCode::ShiftLeft) {
            local_vel.y -= ta * LIFT_THRUST_SCALE * dt;
            thrusting = true;
        }

        local_vel.x /= 1.0 + LATERAL_DRAG * dt;
        local_vel.y /= 1.0 + VERTICAL_DRAG * dt;
        local_vel.z /= 1.0 + FORWARD_DRAG * dt;

        self.ship.velocity = self.ship.quat * local_vel;
        self.ship.thrusting = thrusting;
        let spd = self.ship.velocity.length();
        if spd > self.ship.max_speed {
            self.ship.velocity *= self.ship.max_speed / spd;
        }
        self.ship.pos += self.ship.velocity * dt;

        self.ship.update_visual(dt);
        self.ship.push_transforms(renderer);
        self.ship.update_lights(renderer);

        // Camera lag
        self.cam_pos = self
            .cam_pos
            .lerp(self.ship.desired_cam_pos(), follow(CAMERA_POSITION_LAG, dt));
        self.cam_target = self.cam_target.lerp(
            self.ship.desired_cam_target(),
            follow(CAMERA_TARGET_LAG, dt),
        );
        let desired_up = self.ship.render_up();
        let f = follow(CAMERA_UP_LAG, dt);
        self.cam_up = self.cam_up.lerp(desired_up, f).normalize_or(Vec3::Y);

        Camera::perspective_look_at(
            self.cam_pos,
            self.cam_target,
            self.cam_up,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.5,
            30_000.0,
        )
    }
}
