//! Churn Benchmark — helio v3
//!
//! Stress test with objects continuously spawning, despawning, and moving.
//!
//! Controls:
//!   WASD / Space / Shift — fly
//!   +/-                  — open/close spawn pressure (number of objects added each frame)
//!   C                    — toggle object-object collisions on/off
//!   Escape               — release cursor / exit

mod v3_demo_common;
use v3_demo_common::{
    box_mesh, cube_mesh, despawn_object, make_material, new_scene_db_with_gpu_mirror, plane_mesh,
    point_light, scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object,
    spawn_object_with_movability, update_object_transform,
};

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use pulsar_scenedb::{Entity, SceneDb};
use rapier3d::prelude::*;

use crate::nalgebra::UnitQuaternion;
use std::collections::HashSet;
use std::sync::Arc;
use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

const MAX_DYNAMIC_OBJECTS: usize = 2200;
const START_SPAWN_RATE: usize = 8;
const MIN_SPAWN_RATE: usize = 1;
const MAX_SPAWN_RATE: usize = 64;

struct SpawnedObject {
    id: Entity,
    seed: f32,
    speed: f32,
    scale: f32,
    mesh: Entity,
    material: Entity,
    body_handle: RigidBodyHandle,
    collider_handle: ColliderHandle,
}

struct SimpleRng(u64);
impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }
    fn next_f32(&mut self, min: f32, max: f32) -> f32 {
        let r = self.next_u32() as f32 / 4294967295.0;
        min + (max - min) * r
    }
    fn next_usize(&mut self, max: usize) -> usize {
        (self.next_u32() as usize) % max
    }
}

struct App {
    state: Option<AppState>,
}

struct AppState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,
    renderer: Renderer,
    scene_db: SceneDb,
    last_frame: std::time::Instant,
    frame_count: u64,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    spawn_rate: usize,
    dynamic_objects: Vec<SpawnedObject>,
    meshes: Vec<Entity>,
    materials: Vec<Entity>,
    rng: SimpleRng,
    collisions_enabled: bool,

    time_render_end: Option<std::time::Instant>,
    time_about_to_wait_start: Option<std::time::Instant>,

    physics_integration: IntegrationParameters,
    physics_bodies: RigidBodySet,
    physics_colliders: ColliderSet,
    physics_forces: IslandManager,
    physics_broad_phase: DefaultBroadPhase,
    physics_narrow_phase: NarrowPhase,
    physics_impulse_joints: ImpulseJointSet,
    physics_multibody_joint_set: MultibodyJointSet,
    physics_ccd_solver: CCDSolver,

    time_redraw_requested: Option<std::time::Instant>,

    inspector_agent: Option<scenedb_inspector_agent::InlineAgent>,
}

fn main() {
    env_logger::init();
    log::info!("Starting Churn Benchmark");
    EventLoop::new()
        .expect("event loop")
        .run_app(&mut App { state: None })
        .expect("run");
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Helio — Churn Benchmark")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 720u32)),
                )
                .expect("window"),
        );

        let size = window.inner_size();
        let mut graphics = pollster::block_on(helio::HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            helio::HelioGraphicsConfig {
                vr_support: true,
                application_name: "Churn Benchmark".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let surface_format = format;
        let fmt = format;

        let config = RendererConfig::new(size.width, size.height, fmt);
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let mut renderer = RendererBuilder::new(config, scene_db_handle(&scene_db))
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.04, 0.04, 0.05],
            intensity: 1.0,
        });

        let mat_floor = spawn_material(
            &mut scene_db.world,
            make_material([0.25, 0.25, 0.30, 1.0], 0.85, 0.03, [0.0, 0.0, 0.0], 0.0),
        );
        let mat_red = spawn_material(
            &mut scene_db.world,
            make_material([0.85, 0.12, 0.12, 1.0], 0.65, 0.00, [0.0, 0.0, 0.0], 0.0),
        );
        let mat_green = spawn_material(
            &mut scene_db.world,
            make_material([0.17, 0.82, 0.28, 1.0], 0.60, 0.00, [0.0, 0.0, 0.0], 0.0),
        );
        let mat_blue = spawn_material(
            &mut scene_db.world,
            make_material([0.16, 0.40, 0.90, 1.0], 0.70, 0.00, [0.0, 0.0, 0.0], 0.0),
        );
        let mat_steel = spawn_material(
            &mut scene_db.world,
            make_material([0.7, 0.7, 0.75, 1.0], 0.15, 0.80, [0.0, 0.0, 0.0], 0.0),
        );

        let floor_mesh = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 40.0));
        let _ = spawn_object(
            &mut scene_db.world,
            floor_mesh,
            mat_floor,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, -0.01, 0.0)),
            40.0,
        );

        let mut mesh_list = Vec::new();
        mesh_list.push(spawn_mesh(
            &mut scene_db.world,
            cube_mesh([0.0, 0.0, 0.0], 0.35),
        ));
        mesh_list.push(spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.15, 0.65, 0.15]),
        ));
        mesh_list.push(spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.60, 0.20, 0.20]),
        ));

        let offset = 20.0;
        spawn_light(
            &mut scene_db.world,
            point_light([-offset, 5.0, -offset], [0.8, 0.7, 0.55], 7.0, 40.0),
        );
        spawn_light(
            &mut scene_db.world,
            point_light([offset, 5.0, offset], [0.5, 0.7, 1.0], 7.0, 40.0),
        );

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: fmt,
            renderer,
            scene_db,
            last_frame: std::time::Instant::now(),
            frame_count: 0,
            cam_pos: glam::Vec3::new(0.0, 7.0, 25.0),
            cam_yaw: 0.0,
            cam_pitch: -0.26,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            spawn_rate: START_SPAWN_RATE,
            dynamic_objects: Vec::new(),
            meshes: mesh_list,
            materials: vec![mat_red, mat_green, mat_blue, mat_steel],
            rng: SimpleRng::new(0x59A0_D3E4_B2CA_1897),
            collisions_enabled: false,
            time_render_end: None,
            time_about_to_wait_start: None,
            time_redraw_requested: None,
            physics_integration: IntegrationParameters::default(),
            physics_bodies: RigidBodySet::new(),
            physics_colliders: ColliderSet::new(),
            physics_forces: IslandManager::new(),
            physics_broad_phase: DefaultBroadPhase::new(),
            physics_narrow_phase: NarrowPhase::new(),
            physics_impulse_joints: ImpulseJointSet::new(),
            physics_multibody_joint_set: MultibodyJointSet::new(),
            physics_ccd_solver: CCDSolver::new(),
            inspector_agent: scenedb_inspector_agent::InlineAgent::maybe_start(),
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::Escape),
                        ..
                    },
                ..
            } => {
                if state.cursor_grabbed {
                    state.cursor_grabbed = false;
                    let _ = state.window.set_cursor_grab(CursorGrabMode::None);
                    state.window.set_cursor_visible(true);
                } else {
                    event_loop.exit();
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::Equal),
                        ..
                    },
                ..
            } => {
                state.spawn_rate = (state.spawn_rate + 1).min(MAX_SPAWN_RATE);
                eprintln!("spawn_rate={}", state.spawn_rate);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::Minus),
                        ..
                    },
                ..
            } => {
                state.spawn_rate = state.spawn_rate.saturating_sub(1).max(MIN_SPAWN_RATE);
                eprintln!("spawn_rate={}", state.spawn_rate);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::KeyC),
                        ..
                    },
                ..
            } => {
                state.collisions_enabled = !state.collisions_enabled;
                eprintln!("collisions={}", state.collisions_enabled);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ks,
                        physical_key: PhysicalKey::Code(key),
                        ..
                    },
                ..
            } => match ks {
                ElementState::Pressed => {
                    state.keys.insert(key);
                }
                ElementState::Released => {
                    state.keys.remove(&key);
                }
            },
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !state.cursor_grabbed {
                    let ok = state
                        .window
                        .set_cursor_grab(CursorGrabMode::Confined)
                        .or_else(|_| state.window.set_cursor_grab(CursorGrabMode::Locked))
                        .is_ok();
                    if ok {
                        state.window.set_cursor_visible(false);
                        state.cursor_grabbed = true;
                    }
                }
            }
            WindowEvent::Resized(s) if s.width > 0 && s.height > 0 => {
                state.surface.configure(
                    &state.device,
                    &wgpu::SurfaceConfiguration {
                        // COPY_SRC so the debug frame-dump (see dump_frame_png) can
                        // read the actual presented swapchain texture back.
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::COPY_SRC,
                        format: state.surface_format,
                        width: s.width,
                        height: s.height,
                        present_mode: wgpu::PresentMode::Fifo,
                        alpha_mode: wgpu::CompositeAlphaMode::Auto,
                        view_formats: vec![],
                        desired_maximum_frame_latency: 1,
                        color_space: wgpu::SurfaceColorSpace::Auto,
                    },
                );
                state.renderer.set_render_size(s.width, s.height);
            }
            WindowEvent::RedrawRequested => {
                let now = std::time::Instant::now();

                if let Some(last_render_end) = state.time_render_end {
                    let full_cycle_ms = last_render_end.elapsed().as_secs_f32() * 1000.0;
                    if state.frame_count % 60 == 0 {
                        eprintln!("render_end -> next RedrawRequested: {:.2}ms", full_cycle_ms);
                    }
                }

                if let Some(about_to_wait_start) = state.time_about_to_wait_start {
                    let gap_ms = about_to_wait_start.elapsed().as_secs_f32() * 1000.0;
                    if gap_ms > 2.0 {
                        eprintln!("about_to_wait -> RedrawRequested: {:.2}ms", gap_ms);
                    }
                }

                state.time_redraw_requested = Some(now);
                let dt = (now - state.last_frame).as_secs_f32();
                state.last_frame = now;
                state.render(dt);
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        let Some(state) = &mut self.state else { return };
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if state.cursor_grabbed {
                state.mouse_delta.0 += dx as f32;
                state.mouse_delta.1 += dy as f32;
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(s) = &mut self.state {
            let now = std::time::Instant::now();
            if let Some(render_end) = s.time_render_end {
                let gap_ms = render_end.elapsed().as_secs_f32() * 1000.0;
                if gap_ms > 2.0 {
                    eprintln!("render_end -> about_to_wait: {:.2}ms", gap_ms);
                }
            }
            s.time_about_to_wait_start = Some(now);
            s.window.request_redraw();
        }
    }
}

impl AppState {
    fn spawn_objects(&mut self) {
        let max_count = MAX_DYNAMIC_OBJECTS;
        for _ in 0..self.spawn_rate {
            let mesh = self.meshes[self.rng.next_usize(self.meshes.len())];
            let material = self.materials[self.rng.next_usize(self.materials.len())];
            let radius = self.rng.next_f32(5.0, 20.0);
            let angle = self.rng.next_f32(0.0, std::f32::consts::TAU);
            let height = self.rng.next_f32(0.35, 2.1);
            let scale = self.rng.next_f32(0.25, 1.0);
            let pos = glam::Vec3::new(angle.cos() * radius, height, angle.sin() * radius);
            let transform = glam::Mat4::from_translation(pos)
                * glam::Mat4::from_scale(glam::Vec3::splat(scale));

            let body = RigidBodyBuilder::dynamic()
                .translation([pos.x, pos.y, pos.z].into())
                .linvel(Vector::new(
                    self.rng.next_f32(-4.0, 4.0),
                    self.rng.next_f32(-1.0, 1.0),
                    self.rng.next_f32(-4.0, 4.0),
                ))
                .angvel(Vector::new(
                    self.rng.next_f32(-2.0, 2.0),
                    self.rng.next_f32(-2.0, 2.0),
                    self.rng.next_f32(-2.0, 2.0),
                ))
                .build();
            let body_handle = self.physics_bodies.insert(body);

            let collider = ColliderBuilder::ball((scale * 0.35).max(0.2))
                .restitution(0.3)
                .friction(0.2)
                .build();
            let collider_handle = self.physics_colliders.insert_with_parent(
                collider,
                body_handle,
                &mut self.physics_bodies,
            );

            if let Ok(obj_id) = spawn_object_with_movability(
                &mut self.scene_db.world,
                mesh,
                material,
                transform,
                (scale * 1.3).max(0.15),
                Some(helio::Movability::Movable),
            ) {
                self.dynamic_objects.push(SpawnedObject {
                    id: obj_id,
                    seed: self.rng.next_f32(0.0, std::f32::consts::TAU),
                    speed: self.rng.next_f32(0.4, 1.6),
                    scale,
                    mesh,
                    material,
                    body_handle,
                    collider_handle,
                });
            } else {
                self.physics_colliders.remove(
                    collider_handle,
                    &mut self.physics_forces,
                    &mut self.physics_bodies,
                    false,
                );
                self.physics_bodies.remove(
                    body_handle,
                    &mut self.physics_forces,
                    &mut self.physics_colliders,
                    &mut self.physics_impulse_joints,
                    &mut self.physics_multibody_joint_set,
                    true,
                );
            }

            if self.dynamic_objects.len() > max_count {
                if let Some(dead) = self.dynamic_objects.first() {
                    let _ = despawn_object(&mut self.scene_db.world, &mut self.renderer, dead.id);
                    self.physics_colliders.remove(
                        dead.collider_handle,
                        &mut self.physics_forces,
                        &mut self.physics_bodies,
                        false,
                    );
                    self.physics_bodies.remove(
                        dead.body_handle,
                        &mut self.physics_forces,
                        &mut self.physics_colliders,
                        &mut self.physics_impulse_joints,
                        &mut self.physics_multibody_joint_set,
                        true,
                    );
                }
                self.dynamic_objects.remove(0);
            }
        }
    }

    fn animate_objects(&mut self) {
        let t = (self.frame_count as f32) * 0.01;

        for variant in &mut self.dynamic_objects {
            let phase = variant.seed + t * variant.speed;
            let radius = 8.0 + (phase * 0.25).sin() * 2.0;
            let x = phase.cos() * radius;
            let z = phase.sin() * radius;
            let y = 0.5 + (phase * 1.3).sin() * 0.8;
            let pos = glam::Vec3::new(x, y, z);
            let transform = glam::Mat4::from_translation(pos)
                * glam::Mat4::from_rotation_y(phase * 1.37)
                * glam::Mat4::from_scale(glam::Vec3::splat(variant.scale));
            let _ = update_object_transform(
                &mut self.scene_db.world,
                &mut self.renderer,
                variant.id,
                transform,
            );

            if let Some(body) = self.physics_bodies.get_mut(variant.body_handle) {
                body.set_position(
                    Isometry::from_parts(
                        Translation::from(Vector::new(pos.x, pos.y, pos.z)),
                        UnitQuaternion::from_euler_angles(0.0, phase * 1.37, 0.0),
                    ),
                    true,
                );
                body.set_linvel(Vector::zeros(), true);
                body.set_angvel(Vector::zeros(), true);
            }
        }
    }

    fn step_physics(&mut self, dt: f32) {
        self.physics_integration.dt = dt;
        PhysicsPipeline::new().step(
            &Vector::y_axis(),
            &self.physics_integration,
            &mut self.physics_forces,
            &mut self.physics_broad_phase,
            &mut self.physics_narrow_phase,
            &mut self.physics_bodies,
            &mut self.physics_colliders,
            &mut self.physics_impulse_joints,
            &mut self.physics_multibody_joint_set,
            &mut self.physics_ccd_solver,
            None,
            &(),
            &(),
        );
    }

    fn sync_transforms_from_physics(&mut self) {
        for variant in &self.dynamic_objects {
            if let Some(body) = self.physics_bodies.get(variant.body_handle) {
                let pos = body.position();
                let translation = glam::Vec3::new(
                    pos.translation.vector.x,
                    pos.translation.vector.y,
                    pos.translation.vector.z,
                );
                let rotation = glam::Quat::from_xyzw(
                    pos.rotation.i,
                    pos.rotation.j,
                    pos.rotation.k,
                    pos.rotation.w,
                );
                let transform = glam::Mat4::from_translation(translation)
                    * glam::Mat4::from_quat(rotation)
                    * glam::Mat4::from_scale(glam::Vec3::splat(variant.scale));
                let _ = update_object_transform(
                    &mut self.scene_db.world,
                    &mut self.renderer,
                    variant.id,
                    transform,
                );
            }
        }
    }

    fn render(&mut self, dt: f32) {
        // A churn demo is exactly the case worth polling faster than
        // cloud_engine's ~10 Hz: the point is watching entities spawn/despawn
        // live, so publish every 3rd frame (~20 Hz at 60 fps) instead.
        if let Some(agent) = &mut self.inspector_agent {
            if self.frame_count % 3 == 0 {
                agent.publish(&self.scene_db.world.telemetry_snapshot());
            }
        }

        const SPEED: f32 = 8.0;
        const SENS: f32 = 0.002;
        self.cam_yaw += self.mouse_delta.0 * SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * SENS).clamp(-1.5, 1.5);
        self.mouse_delta = (0.0, 0.0);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let forward = glam::Vec3::new(sy * cp, sp, -cy * cp);
        let right = glam::Vec3::new(cy, 0.0, sy);

        if self.keys.contains(&KeyCode::KeyW) {
            self.cam_pos += forward * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyS) {
            self.cam_pos -= forward * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyA) {
            self.cam_pos -= right * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyD) {
            self.cam_pos += right * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::Space) {
            self.cam_pos.y += SPEED * dt;
        }
        if self.keys.contains(&KeyCode::ShiftLeft) {
            self.cam_pos.y -= SPEED * dt;
        }

        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;
        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            300.0,
        );

        if self.frame_count % 2 == 0 {
            self.spawn_objects();
        }

        if self.collisions_enabled {
            self.step_physics(dt);
            self.sync_transforms_from_physics();
        } else {
            self.animate_objects();
        }

        // Uploads every row queued since last frame (spawns/inserts/
        // updates/despawns) into the GPU-mirrored buffers the renderer
        // actually reads. Without this, CPU-side SceneDB writes are
        // authoritative but invisible to the GPU forever -- this is the
        // root cause of a fully black render despite correct scene data
        // (confirmed via ObjectBatchPass reporting instance_count=0/
        // draw_count=0 even with valid StaticObjectComponent rows present).
        v3_demo_common::flush_scene_db(&self.scene_db, &self.queue);

        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }

        // Debug: dump depth once after the scene has had a chance to settle.
        // The window surface is acquired inside Helio after XR frame pacing,
        // so this diagnostic intentionally captures the renderer's depth
        // target rather than acquiring the surface early just for a screenshot.
        if self.frame_count == 100 {
            dump_depth_png(
                &self.device,
                &self.queue,
                self.renderer.debug_depth_texture(),
                std::path::Path::new("frame_100_depth.png"),
            );
        }

        self.frame_count += 1;
        self.time_render_end = Some(std::time::Instant::now());

        if self.frame_count % 60 == 0 {
            eprintln!(
                "Churn: frame {} objects={} spawn_rate={} dt={:.3}ms",
                self.frame_count,
                self.dynamic_objects.len(),
                self.spawn_rate,
                dt * 1000.0
            );
        }
    }
}

/// Debug capture: read the actual presented swapchain texture back to CPU
/// and save it as a PNG. Deliberately reads the *final* output rather than
/// an intermediate render-graph buffer (G-buffer albedo, depth, etc.):
/// those are privately owned inside each pass crate (`Renderer` itself only
/// holds `depth_texture` as a raw `wgpu::Texture`; everything else is a
/// `wgpu::TextureView` handed out per-frame via `ResourceRegistry`, with no
/// path back to the owning `Texture` `copy_texture_to_buffer` needs), so
/// capturing them would mean adding a new debug-only trait method to
/// `RenderPass` and implementing it pass-by-pass -- worth doing later if a
/// specific intermediate stage needs inspecting, but the final frame
/// already answers "does anything render at all" with zero engine changes
/// beyond adding `COPY_SRC` to the surface's usage flags.
///
/// Blocks the calling thread until the GPU readback completes (a one-time
/// debug capture, not a hot-path concern).
fn dump_frame_png(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    path: &std::path::Path,
) {
    let Some(bytes_per_pixel) = format.block_copy_size(None) else {
        log::error!("dump_frame_png: unsupported (non-color) format {format:?}");
        return;
    };

    let unpadded_bytes_per_row = width * bytes_per_pixel;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;
    let buffer_size = (padded_bytes_per_row as u64) * (height as u64);

    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Debug Frame Dump Buffer"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Debug Frame Dump Encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = readback_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    if let Err(e) = device.poll(wgpu::PollType::wait_indefinitely()) {
        log::error!("dump_frame_png: device.poll failed: {e:?}");
        return;
    }
    match rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log::error!("dump_frame_png: buffer map failed: {e:?}");
            return;
        }
        Err(e) => {
            log::error!("dump_frame_png: map_async never signaled: {e:?}");
            return;
        }
    }

    let mapped = match slice.get_mapped_range() {
        Ok(m) => m,
        Err(e) => {
            log::error!("dump_frame_png: get_mapped_range failed: {e:?}");
            return;
        }
    };
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let is_bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    for y in 0..height {
        let row_start = (y * padded_bytes_per_row) as usize;
        let src_row = &mapped[row_start..row_start + unpadded_bytes_per_row as usize];
        let dst_row = &mut rgba[(y * width * 4) as usize..((y + 1) * width * 4) as usize];
        if is_bgra {
            for (src_px, dst_px) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
                dst_px[0] = src_px[2];
                dst_px[1] = src_px[1];
                dst_px[2] = src_px[0];
                dst_px[3] = src_px[3];
            }
        } else {
            dst_row.copy_from_slice(&src_row[..dst_row.len().min(src_row.len())]);
        }
    }
    drop(mapped);
    readback_buffer.unmap();

    match image::save_buffer(path, &rgba, width, height, image::ColorType::Rgba8) {
        Ok(()) => log::info!("dump_frame_png: wrote {}", path.display()),
        Err(e) => log::error!("dump_frame_png: failed to save {}: {e:?}", path.display()),
    }
}

/// Debug capture: read a `Depth32Float` texture back and save an
/// autocontrast-normalized grayscale visualization (actual min/max depth
/// found in the buffer map to black/white, since real depth values cluster
/// tightly near the far plane and a raw 0..1 mapping would look uniformly
/// white) -- answers "is anything being rasterized in front of the camera
/// at all" independent of shading/lighting/post-process, which the final
/// composited frame alone can't distinguish.
fn dump_depth_png(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    path: &std::path::Path,
) {
    let width = texture.width();
    let height = texture.height();
    let unpadded_bytes_per_row = width * 4; // Depth32Float = 4 bytes/texel
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;
    let buffer_size = (padded_bytes_per_row as u64) * (height as u64);

    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Debug Depth Dump Buffer"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Debug Depth Dump Encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::DepthOnly,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = readback_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    if let Err(e) = device.poll(wgpu::PollType::wait_indefinitely()) {
        log::error!("dump_depth_png: device.poll failed: {e:?}");
        return;
    }
    match rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log::error!("dump_depth_png: buffer map failed: {e:?}");
            return;
        }
        Err(e) => {
            log::error!("dump_depth_png: map_async never signaled: {e:?}");
            return;
        }
    }

    let mapped = match slice.get_mapped_range() {
        Ok(m) => m,
        Err(e) => {
            log::error!("dump_depth_png: get_mapped_range failed: {e:?}");
            return;
        }
    };

    let mut values = vec![0f32; (width * height) as usize];
    for y in 0..height {
        let row_start = (y * padded_bytes_per_row) as usize;
        let row_bytes = &mapped[row_start..row_start + unpadded_bytes_per_row as usize];
        let dst_row = &mut values[(y * width) as usize..((y + 1) * width) as usize];
        for (px, dst) in row_bytes.chunks_exact(4).zip(dst_row.iter_mut()) {
            *dst = f32::from_le_bytes([px[0], px[1], px[2], px[3]]);
        }
    }
    drop(mapped);
    readback_buffer.unmap();

    let min = values.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = (max - min).max(1e-8);
    log::info!("dump_depth_png: depth range [{min}, {max}]");

    let mut gray = vec![0u8; (width * height * 4) as usize];
    for (i, &v) in values.iter().enumerate() {
        // Invert: near (small depth) -> bright, far/cleared (1.0) -> dark,
        // matching the usual "closer = whiter" depth-visualization convention.
        let normalized = 1.0 - ((v - min) / range);
        let byte = (normalized.clamp(0.0, 1.0) * 255.0) as u8;
        gray[i * 4] = byte;
        gray[i * 4 + 1] = byte;
        gray[i * 4 + 2] = byte;
        gray[i * 4 + 3] = 255;
    }

    match image::save_buffer(path, &gray, width, height, image::ColorType::Rgba8) {
        Ok(()) => log::info!("dump_depth_png: wrote {}", path.display()),
        Err(e) => log::error!("dump_depth_png: failed to save {}: {e:?}", path.display()),
    }
}
