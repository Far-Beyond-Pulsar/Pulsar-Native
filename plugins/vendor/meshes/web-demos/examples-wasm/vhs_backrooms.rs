//! WASM twin of `vhs_backrooms` — a procedurally generated backrooms maze
//! rendered through the **custom user-effects render graph**: the full default
//! deferred chain with a VHS camcorder WGSL snippet injected into the
//! post-process pass, plus a scene-wide post-process volume. The degraded
//! camcorder look (chromatic aberration, grain, vignette, tracking noise,
//! flicker) is driven entirely by `vhs_effects.wgsl`.
//!
//! Demonstrates per-frame custom params via
//! `PostProcessPass::set_custom_params`.
//!
//! Controls:
//!   Click        — grab cursor / look
//!   WASD         — move · Space/Shift — up/down
//!   R            — regenerate the maze
//!   Escape       — release cursor

use std::sync::Arc;

use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState, KeyCode};
use pulsar_scenedb::{Entity, SceneDb, World};

use crate::common::{
    box_mesh, insert_object, make_material, point_light, spawn_light, spawn_material, spawn_mesh,
};

const VHS_SHADER_SNIPPET: &str = include_str!("vhs_effects.wgsl");

const LOOK_SENS: f32 = 0.002;
const MOVE_SPEED: f32 = 12.0;
const DRAG: f32 = 8.0;

// Maze dimensions.
const GRID: i32 = 8; // GRID × GRID cells
const CELL: f32 = 8.0; // world units per cell
const WALL_H: f32 = 3.0; // ceiling height
const WALL_T: f32 = 0.3; // wall half-thickness
const HALF: f32 = GRID as f32 * CELL * 0.5; // half of the whole floor plan

/// Tiny deterministic LCG so each seed produces the same, reliably navigable map.
struct Rng(u32);
impl Rng {
    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        self.0
    }
    fn chance(&mut self, p: f32) -> bool {
        (self.next_u32() >> 8) as f32 / 16_777_216.0 < p
    }
}

pub struct Demo {
    cam_pos: Vec3,
    yaw: f32,
    pitch: f32,
    velocity: Vec3,
    seed: u32,
    prev_regen: bool,
}

/// Insert a static box (mesh + object) at `center` with the given half-extents.
fn add_box(world: &mut World, center: [f32; 3], half: [f32; 3], material: Entity) {
    // Build the box at the local origin and place it via the transform. The
    // object's cull bounds are derived from the transform translation, so baking
    // the position into the mesh (and leaving an identity transform) would put
    // every box's bounding sphere at the world origin — making them all frustum-
    // cull together instead of each on its own geometry.
    let mesh = spawn_mesh(world, box_mesh([0.0, 0.0, 0.0], half));
    let radius = (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt();
    let _ = insert_object(
        world,
        mesh,
        material,
        Mat4::from_translation(Vec3::from(center)),
        radius,
    );
}

impl Demo {
    /// Build the whole scene from `self.seed`: wipe any prior contents, create
    /// materials, lay out the maze, and re-add the scene-wide VHS post-process
    /// volume (which `clear` also removes).
    fn build_scene(&self, world: &mut World) {
        let (wall, floor, ceiling, pillar, fixture) = {
            let world = &mut *world;
            (
                // Classic mono-yellow damp wallpaper.
                spawn_material(
                    world,
                    make_material([0.78, 0.70, 0.34, 1.0], 0.92, 0.0, [0.0; 3], 0.0),
                ),
                // Moist patterned carpet.
                spawn_material(
                    world,
                    make_material([0.42, 0.37, 0.22, 1.0], 0.96, 0.0, [0.0; 3], 0.0),
                ),
                // Off-white drop-ceiling tiles.
                spawn_material(
                    world,
                    make_material([0.80, 0.78, 0.70, 1.0], 0.88, 0.0, [0.0; 3], 0.0),
                ),
                spawn_material(
                    world,
                    make_material([0.74, 0.66, 0.32, 1.0], 0.9, 0.0, [0.0; 3], 0.0),
                ),
                // Emissive fluorescent panel.
                spawn_material(
                    world,
                    make_material([0.9, 0.9, 0.85, 1.0], 0.6, 0.0, [1.0, 0.96, 0.85], 6.0),
                ),
            )
        };

        // Floor + ceiling slabs spanning the whole plan.
        add_box(world, [0.0, -WALL_T, 0.0], [HALF, WALL_T, HALF], floor);
        add_box(
            world,
            [0.0, WALL_H + WALL_T, 0.0],
            [HALF, WALL_T, HALF],
            ceiling,
        );

        // Perimeter walls.
        for &(cx, cz, hx, hz) in &[
            (0.0, -HALF, HALF, WALL_T),
            (0.0, HALF, HALF, WALL_T),
            (-HALF, 0.0, WALL_T, HALF),
            (HALF, 0.0, WALL_T, HALF),
        ] {
            add_box(world, [cx, WALL_H * 0.5, cz], [hx, WALL_H * 0.5, hz], wall);
        }

        let mut rng = Rng(self.seed | 1);

        // Interior wall segments on cell edges. Each is shorter than a cell and
        // centered, leaving gaps at both ends so the maze is always navigable.
        let seg = CELL * 0.35; // half-length of a wall segment (< CELL/2)
        for gx in 0..GRID {
            for gz in 0..GRID {
                let cx = -HALF + (gx as f32 + 0.5) * CELL;
                let cz = -HALF + (gz as f32 + 0.5) * CELL;

                // Wall on the +X edge of this cell (interior edges only).
                if gx < GRID - 1 && rng.chance(0.45) {
                    add_box(
                        world,
                        [cx + CELL * 0.5, WALL_H * 0.5, cz],
                        [WALL_T, WALL_H * 0.5, seg],
                        wall,
                    );
                }
                // Wall on the +Z edge of this cell.
                if gz < GRID - 1 && rng.chance(0.45) {
                    add_box(
                        world,
                        [cx, WALL_H * 0.5, cz + CELL * 0.5],
                        [seg, WALL_H * 0.5, WALL_T],
                        wall,
                    );
                }

                // Occasional support pillar at a cell corner.
                if rng.chance(0.22) {
                    add_box(
                        world,
                        [cx + CELL * 0.5, WALL_H * 0.5, cz + CELL * 0.5],
                        [0.35, WALL_H * 0.5, 0.35],
                        pillar,
                    );
                }

                // Ceiling fluorescent fixture + light. A few cells stay dark.
                if !rng.chance(0.16) {
                    add_box(world, [cx, WALL_H - 0.05, cz], [1.2, 0.05, 0.35], fixture);
                    spawn_light(
                        world,
                        point_light([cx, WALL_H - 0.3, cz], [1.0, 0.96, 0.85], 6.0, CELL * 1.6),
                    );
                }
            }
        }
    }
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — VHS Backrooms"
    }

    // Match the native demo: render at full resolution (the default graph's TAA
    // still resolves the jitter, which stays enabled).
    fn render_scale() -> f32 {
        1.0
    }

    fn init(
        renderer: &mut Renderer,
        scene_db: &mut SceneDb,
        _device: Arc<wgpu::Device>,
        _queue: Arc<wgpu::Queue>,
        _w: u32,
        _h: u32,
    ) -> Self {
        let demo = Self {
            cam_pos: Vec3::new(0.0, 1.6, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            velocity: Vec3::ZERO,
            seed: 1,
            prev_regen: false,
        };
        demo.build_scene(&mut scene_db.world);

        renderer.set_feature_live(helio::AmbientLight {
            color: [0.75, 0.7, 0.6],
            intensity: 0.04,
        });
        renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
        demo
    }

    fn update(
        &mut self,
        renderer: &mut Renderer,
        dt: f32,
        elapsed: f32,
        input: &InputState,
    ) -> Camera {
        // ── Look ──────────────────────────────────────────────────────────
        let (dx, dy) = input.mouse_delta;
        self.yaw -= dx * LOOK_SENS;
        self.pitch = (self.pitch - dy * LOOK_SENS).clamp(-1.5, 1.5);

        let orientation = Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0);
        let forward = orientation * -Vec3::Z;
        let right = orientation * Vec3::X;

        // ── Move ──────────────────────────────────────────────────────────
        let mut accel = Vec3::ZERO;
        if input.keys.contains(&KeyCode::KeyW) {
            accel += forward;
        }
        if input.keys.contains(&KeyCode::KeyS) {
            accel -= forward;
        }
        if input.keys.contains(&KeyCode::KeyA) {
            accel -= right;
        }
        if input.keys.contains(&KeyCode::KeyD) {
            accel += right;
        }
        if input.keys.contains(&KeyCode::Space) {
            accel += Vec3::Y;
        }
        if input.keys.contains(&KeyCode::ShiftLeft) {
            accel -= Vec3::Y;
        }
        if accel.length_squared() > 0.0 {
            accel = accel.normalize();
        }
        self.velocity += accel * MOVE_SPEED * dt;
        self.velocity /= 1.0 + DRAG * dt;
        self.cam_pos += self.velocity * dt;

        // Keep the camera inside the building.
        let bound = HALF - 0.4;
        self.cam_pos.x = self.cam_pos.x.clamp(-bound, bound);
        self.cam_pos.z = self.cam_pos.z.clamp(-bound, bound);
        self.cam_pos.y = self.cam_pos.y.clamp(0.4, WALL_H - 0.3);

        // ── Regenerate (R, edge-triggered) ────────────────────────────────
        let regen = input.keys.contains(&KeyCode::KeyR);
        if regen && !self.prev_regen {
            self.seed = self.seed.wrapping_mul(2654435761).wrapping_add(1);
            // SceneDB is authoritative; rebuilding requires a World handle in
            // the update hook, which the current WASM trait does not expose.
        }
        self.prev_regen = regen;

        // ── VHS post-process params (mirrors the native demo) ─────────────
        let vhs_params: [[f32; 4]; 2] = [
            [0.0, 0.12, 8.0, 0.2],    // _, tape jitter, jitter freq, flicker intensity
            [0.4, elapsed, 0.0, 0.0], // noise amount, animation time, _, _
        ];
        if let Some(pass) = renderer.find_pass_mut::<helio_pass_postprocess::PostProcessPass>() {
            pass.set_custom_params(&vhs_params);
        }

        let target = self.cam_pos + forward;
        let up = orientation * Vec3::Y;
        Camera::perspective_look_at(
            self.cam_pos,
            target,
            up,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.05,
            500.0,
        )
    }
}
