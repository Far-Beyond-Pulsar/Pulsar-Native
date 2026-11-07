//! Rapier Collision Groups Demo — helio v3
//!
//! Two sets of spheres: group A and group B. A collides with B, but not with same group.
//! Controls:
//!   C — toggle physics
//!   R — respawn objects
//!   WASD / Space / Shift — fly camera
//!   Escape — release cursor / exit

mod v3_demo_common;
use pulsar_scenedb::{Entity, SceneDb};
use v3_demo_common::{
    box_mesh, despawn_object, make_material, new_scene_db_with_gpu_mirror, plane_mesh, point_light,
    scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object,
    spawn_object_with_movability, sphere_mesh, update_object_transform,
};

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use rapier3d::prelude::*;
use std::collections::HashSet;
use std::sync::Arc;
use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

struct GroupObject {
    id: Entity,
    body_handle: RigidBodyHandle,
    collider_handle: ColliderHandle,
}

struct AppState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,
    renderer: Renderer,
    last_frame: std::time::Instant,
    frame_count: u64,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    scene_db: SceneDb,
    group_a: Vec<GroupObject>,
    group_b: Vec<GroupObject>,
    group_a_mat: Entity,
    group_b_mat: Entity,
    physics_enabled: bool,

    physics_integration: IntegrationParameters,
    physics_bodies: RigidBodySet,
    physics_colliders: ColliderSet,
    physics_forces: IslandManager,
    physics_broad_phase: DefaultBroadPhase,
    physics_narrow_phase: NarrowPhase,
    physics_impulse_joints: ImpulseJointSet,
    physics_multibody_joint_set: MultibodyJointSet,
    physics_ccd_solver: CCDSolver,

    time_render_end: Option<std::time::Instant>,
    time_about_to_wait_start: Option<std::time::Instant>,
    time_redraw_requested: Option<std::time::Instant>,
}

struct App {
    state: Option<AppState>,
}

fn main() {
    env_logger::init();
    log::info!("Starting Rapier Collision Groups Demo");
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
                        .with_title("Helio — Rapier Collision Groups Demo")
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
                application_name: "Rapier Collision Groups".into(),
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
        let graph_scene_db = scene_db_handle(&scene_db);
        let mut renderer = RendererBuilder::new(config, graph_scene_db.clone())
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.05, 0.05, 0.07],
            intensity: 1.0,
        });

        let floor_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.22, 0.22, 0.28, 1.0], 0.88, 0.02, [0.0, 0.0, 0.0], 0.0),
        );
        let a_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.8, 0.18, 0.18, 1.0], 0.46, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let b_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.18, 0.7, 0.86, 1.0], 0.46, 0.0, [0.0, 0.0, 0.0], 0.0),
        );

        let floor_mesh = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 70.0));
        let _ = spawn_object(
            &mut scene_db.world,
            floor_mesh,
            floor_mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, -0.01, 0.0)),
            70.0,
        );

        spawn_light(
            &mut scene_db.world,
            point_light([18.0, 26.0, 18.0], [1.0, 0.88, 0.77], 10.0, 50.0),
        );
        spawn_light(
            &mut scene_db.world,
            point_light([-18.0, 19.0, -21.0], [0.6, 0.8, 1.0], 10.0, 50.0),
        );

        let mut state = AppState {
            window,
            surface,
            device,
            queue,
            surface_format: fmt,
            renderer,
            last_frame: std::time::Instant::now(),
            frame_count: 0,
            cam_pos: glam::Vec3::new(0.0, 25.0, 30.0),
            cam_yaw: 0.0,
            cam_pitch: -0.35,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            scene_db,
            group_a: Vec::new(),
            group_b: Vec::new(),
            group_a_mat: a_mat,
            group_b_mat: b_mat,
            physics_enabled: true,
            physics_integration: IntegrationParameters::default(),
            physics_bodies: RigidBodySet::new(),
            physics_colliders: ColliderSet::new(),
            physics_forces: IslandManager::new(),
            physics_broad_phase: DefaultBroadPhase::new(),
            physics_narrow_phase: NarrowPhase::new(),
            physics_impulse_joints: ImpulseJointSet::new(),
            physics_multibody_joint_set: MultibodyJointSet::new(),
            physics_ccd_solver: CCDSolver::new(),
            time_render_end: None,
            time_about_to_wait_start: None,
            time_redraw_requested: None,
        };

        state.spawn_groups(a_mat, b_mat);

        self.state = Some(state);
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
                        physical_key: PhysicalKey::Code(KeyCode::KeyC),
                        ..
                    },
                ..
            } => {
                state.physics_enabled = !state.physics_enabled;
                eprintln!("physics={}", state.physics_enabled);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::KeyR),
                        ..
                    },
                ..
            } => {
                state.clear_groups();
                state.spawn_groups(state.group_a_mat, state.group_b_mat);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ks,
                        physical_key: PhysicalKey::Code(key),
                        ..
                    },
                ..
            } => {
                match ks {
                    ElementState::Pressed => state.keys.insert(key),
                    ElementState::Released => state.keys.remove(&key),
                };
            }
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
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
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
                if let Some(last) = state.time_render_end {
                    let full_cycle_ms = last.elapsed().as_secs_f32() * 1000.0;
                    if state.frame_count % 60 == 0 {
                        eprintln!("render_end -> next: {:.2}ms", full_cycle_ms);
                    }
                }
                if let Some(about) = state.time_about_to_wait_start {
                    let gap_ms = about.elapsed().as_secs_f32() * 1000.0;
                    if gap_ms > 2.0 {
                        eprintln!("about_to_wait -> redraw: {:.2}ms", gap_ms);
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
            if let Some(end) = s.time_render_end {
                let gap_ms = end.elapsed().as_secs_f32() * 1000.0;
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
    fn clear_groups(&mut self) {
        for obj in self.group_a.drain(..).chain(self.group_b.drain(..)) {
            let _ = despawn_object(&mut self.scene_db.world, &mut self.renderer, obj.id);
            self.physics_colliders.remove(
                obj.collider_handle,
                &mut self.physics_forces,
                &mut self.physics_bodies,
                false,
            );
            self.physics_bodies.remove(
                obj.body_handle,
                &mut self.physics_forces,
                &mut self.physics_colliders,
                &mut self.physics_impulse_joints,
                &mut self.physics_multibody_joint_set,
                true,
            );
        }
    }

    fn spawn_groups(&mut self, a_mat: Entity, b_mat: Entity) {
        // Group A collides with group B and ground.
        let g_a = InteractionGroups::new(0b0001u32.into(), (0b0010u32 | 0b0100u32).into());
        // Group B collides with group A only, so it will fall through the ground.
        let g_b = InteractionGroups::new(0b0010u32.into(), 0b0001u32.into());
        // Ground is group 0b0100 and only collides with group A.
        let g_ground = InteractionGroups::new(0b0100u32.into(), 0b0001u32.into());

        let ball_mesh = sphere_mesh([0.0, 0.0, 0.0], 0.6);
        for i in 0..64 {
            let x = (i % 8) as f32 * 2.0 - 7.0;
            let z = (i / 8) as f32 * 2.0 - 7.0;

            let mat = if (i % 2) == 0 { a_mat } else { b_mat };
            let group = if (i % 2) == 0 { g_a } else { g_b };
            let y = 6.0 + (i / 16) as f32 * 1.5;

            let id = spawn_mesh(
                &mut self.scene_db.world,
                box_mesh([0.0, 0.0, 0.0], [0.6, 0.6, 0.6]),
            );
            let obj = spawn_object_with_movability(
                &mut self.scene_db.world,
                id,
                mat,
                glam::Mat4::from_translation(glam::Vec3::new(x, y, z)),
                1.0,
                Some(helio::Movability::Movable),
            )
            .expect("insert");

            let body = RigidBodyBuilder::dynamic()
                .translation([x, y, z].into())
                .linvel(Vector::new(0.0, -1.0, 0.0))
                .build();
            let body_handle = self.physics_bodies.insert(body);
            let collider = ColliderBuilder::ball(0.6)
                .restitution(0.5)
                .friction(0.2)
                .collision_groups(group)
                .build();
            let collider_handle = self.physics_colliders.insert_with_parent(
                collider,
                body_handle,
                &mut self.physics_bodies,
            );
            if (i % 2) == 0 {
                self.group_a.push(GroupObject {
                    id: obj,
                    body_handle,
                    collider_handle,
                });
            } else {
                self.group_b.push(GroupObject {
                    id: obj,
                    body_handle,
                    collider_handle,
                });
            }
        }

        let ground_body = RigidBodyBuilder::fixed()
            .translation([0.0, -0.2, 0.0].into())
            .build();
        let ground_handle = self.physics_bodies.insert(ground_body);
        let ground_collider = ColliderBuilder::cuboid(30.0, 0.5, 30.0)
            .friction(0.9)
            .collision_groups(g_ground)
            .build();
        self.physics_colliders.insert_with_parent(
            ground_collider,
            ground_handle,
            &mut self.physics_bodies,
        );
    }

    fn step_physics(&mut self, dt: f32) {
        self.physics_integration.dt = dt;
        let gravity = Vector::new(0.0, -9.81, 0.0);
        PhysicsPipeline::new().step(
            &gravity,
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
        for obj in self.group_a.iter().chain(self.group_b.iter()) {
            if let Some(body) = self.physics_bodies.get(obj.body_handle) {
                let pos = body.position();
                let translation =
                    glam::Vec3::new(pos.translation.x, pos.translation.y, pos.translation.z);
                let rotation = glam::Quat::from_xyzw(
                    pos.rotation.i,
                    pos.rotation.j,
                    pos.rotation.k,
                    pos.rotation.w,
                );
                let transform =
                    glam::Mat4::from_translation(translation) * glam::Mat4::from_quat(rotation);
                let _ = update_object_transform(
                    &mut self.scene_db.world,
                    &mut self.renderer,
                    obj.id,
                    transform,
                );
            }
        }
    }

    fn render(&mut self, dt: f32) {
        const SPEED: f32 = 12.0;
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

        if self.physics_enabled {
            self.step_physics(dt);
            self.sync_transforms_from_physics();
        }

        v3_demo_common::flush_scene_db(&self.scene_db, &self.queue);
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
        self.frame_count += 1;
        self.time_render_end = Some(std::time::Instant::now());

        if self.frame_count % 60 == 0 {
            eprintln!(
                "Rapier Collision Groups: frame {} sim={}",
                self.frame_count, self.physics_enabled
            );
        }
    }
}
