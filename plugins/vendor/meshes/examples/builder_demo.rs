//! Impressive demo using the new [`RendererBuilder`] API.
//!
//! A colonnade with metallic spheres, orbiting coloured lights,
//! and a rotating central crystal — all in ~100 lines of scene code.
//!
//! Controls:
//!   WASD        — fly around
//!   Space/Shift — move up/down
//!   Left click  — grab cursor, drag to look
//!   Escape      — release cursor / exit
//!
//! The sky (the atmosphere and its clouds) and GI left Helio with their
//! plugins (Pulsar `plugins/vendor/sky`, `plugins/vendor/gi`): this scene
//! has no sky and no GI probes; the background colour and the ambient
//! light stand in.

mod v3_demo_common;

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    Movability, RendererBuilder, RendererConfig,
};
use pulsar_scenedb::{Entity, SceneDb};
use v3_demo_common::{
    box_mesh, cube_mesh, make_material, new_scene_db_with_gpu_mirror, plane_mesh, point_light,
    scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object_with_movability,
    sphere_mesh, update_object_transform, update_point_light,
};

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

use std::collections::HashSet;
use std::sync::Arc;

fn main() {
    env_logger::init();

    let event_loop = EventLoop::new().expect("Failed to create event loop");
    event_loop
        .run_app(&mut App::new())
        .expect("Event loop error");
}

struct App {
    state: Option<State>,
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,
    renderer: helio::Renderer,
    last_frame: std::time::Instant,
    start_time: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    scene_db: SceneDb,

    orbit_lights: [Entity; 4],
    spin_crystal: Entity,
}

impl App {
    fn new() -> Self {
        Self { state: None }
    }
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
                        .with_title("Helio Builder Demo")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280, 720)),
                )
                .expect("create window"),
        );

        let size = window.inner_size();
        let mut graphics = pollster::block_on(helio::HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            helio::HelioGraphicsConfig {
                vr_support: true,
                application_name: "Builder Demo".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let w = size.width;
        let h = size.height;
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let surface_format = format;

        let config = RendererConfig::new(w, h, surface_format);
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let graph_scene_db = scene_db_handle(&scene_db);
        let mut renderer = RendererBuilder::new(config, graph_scene_db.clone())
            .with_editor_mode(true)
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;

        // ── Materials ───────────────────────────────────────────────────────
        let gold = spawn_material(
            &mut scene_db.world,
            make_material([0.95, 0.75, 0.25, 1.0], 0.25, 0.85, [0.0, 0.0, 0.0], 0.0),
        );
        let marble = spawn_material(
            &mut scene_db.world,
            make_material([0.85, 0.83, 0.80, 1.0], 0.55, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let crystal = spawn_material(
            &mut scene_db.world,
            make_material([0.3, 0.6, 1.0, 1.0], 0.05, 0.1, [0.2, 0.4, 1.0], 2.0),
        );
        let floor = spawn_material(
            &mut scene_db.world,
            make_material([0.22, 0.22, 0.25, 1.0], 0.7, 0.05, [0.0, 0.0, 0.0], 0.0),
        );

        // ── Ground ──────────────────────────────────────────────────────────
        let ground_mesh = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 12.0));
        let _ = spawn_object_with_movability(
            &mut scene_db.world,
            ground_mesh,
            floor,
            glam::Mat4::IDENTITY,
            12.0,
            None,
        );

        // ── Columns (box pillars + sphere tops) ────────────────────────────
        let pillar_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.15, 2.0, 0.15]),
        );
        let sphere_mesh_id = spawn_mesh(&mut scene_db.world, sphere_mesh([0.0, 0.0, 0.0], 0.4));

        let radius = 4.0;
        let positions = [
            (radius, 0.0, 0.0),
            (0.0, 0.0, radius),
            (-radius, 0.0, 0.0),
            (0.0, 0.0, -radius),
        ];
        let mut orbit_lights = Vec::new();
        for (i, (x, _, z)) in positions.iter().enumerate() {
            // Pillar
            let _ = spawn_object_with_movability(
                &mut scene_db.world,
                pillar_mesh,
                marble,
                glam::Mat4::from_translation(glam::vec3(*x, 2.0, *z)),
                0.15,
                None,
            );
            // Gold sphere on top
            let _ = spawn_object_with_movability(
                &mut scene_db.world,
                sphere_mesh_id,
                gold,
                glam::Mat4::from_translation(glam::vec3(*x, 4.3, *z)),
                0.4,
                None,
            );
            // Coloured light at each column
            let colors = [
                [1.0, 0.3, 0.3],
                [0.3, 1.0, 0.3],
                [0.3, 0.5, 1.0],
                [1.0, 0.8, 0.2],
            ];
            let lid = spawn_light(
                &mut scene_db.world,
                point_light([*x, 5.0, *z], colors[i], 8.0, 8.0),
            );
            orbit_lights.push(lid);
        }

        // ── Floating crystal (centre, rotating) ────────────────────────────
        let crystal_mesh = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.0, 0.0], 0.6));
        let spin_crystal = spawn_object_with_movability(
            &mut scene_db.world,
            crystal_mesh,
            crystal,
            glam::Mat4::from_translation(glam::vec3(0.0, 2.5, 0.0)),
            0.6,
            Some(Movability::Movable),
        )
        .unwrap();

        self.state = Some(State {
            window,
            surface,
            device,
            queue,
            surface_format,
            renderer,
            last_frame: std::time::Instant::now(),
            start_time: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 3.5, 10.0),
            cam_yaw: 0.0,
            cam_pitch: -0.25,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            scene_db,
            orbit_lights: orbit_lights.try_into().unwrap(),
            spin_crystal,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(s) = &mut self.state else {
            return;
        };

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
                if s.cursor_grabbed {
                    s.cursor_grabbed = false;
                    let _ = s.window.set_cursor_grab(CursorGrabMode::None);
                    s.window.set_cursor_visible(true);
                } else {
                    event_loop.exit();
                }
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
                    s.keys.insert(key);
                }
                ElementState::Released => {
                    s.keys.remove(&key);
                }
            },
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !s.cursor_grabbed {
                    let grabbed = s
                        .window
                        .set_cursor_grab(CursorGrabMode::Confined)
                        .or_else(|_| s.window.set_cursor_grab(CursorGrabMode::Locked))
                        .is_ok();
                    if grabbed {
                        s.window.set_cursor_visible(false);
                        s.cursor_grabbed = true;
                    }
                }
            }
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                let cfg = wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format: s.surface_format,
                    width: size.width,
                    height: size.height,
                    present_mode: wgpu::PresentMode::Fifo,
                    alpha_mode: wgpu::CompositeAlphaMode::Auto,
                    view_formats: vec![],
                    desired_maximum_frame_latency: 2,
                    color_space: wgpu::SurfaceColorSpace::Auto,
                };
                s.surface.configure(&s.device, &cfg);
                s.renderer.set_render_size(size.width, size.height);
            }
            WindowEvent::RedrawRequested => {
                let now = std::time::Instant::now();
                let dt = (now - s.last_frame).as_secs_f32().min(0.05);
                s.last_frame = now;
                s.render(dt);
                s.window.request_redraw();
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let Some(s) = &mut self.state {
            if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
                if s.cursor_grabbed {
                    s.mouse_delta.0 += dx as f32;
                    s.mouse_delta.1 += dy as f32;
                }
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(s) = &self.state {
            s.window.request_redraw();
        }
    }
}

impl State {
    fn render(&mut self, dt: f32) {
        // ── Camera ─────────────────────────────────────────────────────────
        const SPEED: f32 = 6.0;
        const SENS: f32 = 0.002;
        self.cam_yaw += self.mouse_delta.0 * SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * SENS).clamp(-1.5, 1.5);
        self.mouse_delta = (0.0, 0.0);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let fwd = glam::Vec3::new(sy * cp, sp, -cy * cp);
        let right = glam::Vec3::new(cy, 0.0, sy);
        if self.keys.contains(&KeyCode::KeyW) {
            self.cam_pos += fwd * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyS) {
            self.cam_pos -= fwd * SPEED * dt;
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
        let elapsed = self.start_time.elapsed().as_secs_f32();
        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            200.0,
        );

        // ── Animate orbiting lights ─────────────────────────────────────────
        let colors = [
            [1.0, 0.3, 0.3],
            [0.3, 1.0, 0.3],
            [0.3, 0.5, 1.0],
            [1.0, 0.8, 0.2],
        ];
        for (i, &lid) in self.orbit_lights.iter().enumerate() {
            let angle = elapsed * 0.6 + i as f32 * std::f32::consts::FRAC_PI_2;
            let x = angle.cos() * 4.0;
            let z = angle.sin() * 4.0;
            update_point_light(
                &mut self.scene_db.world,
                lid,
                glam::vec3(x, 5.0 + (elapsed * 1.2 + i as f32).sin() * 0.5, z),
                colors[i],
                10.0,
                10.0,
            );
        }

        // ── Animate crystal ────────────────────────────────────────────────
        let rot = glam::Quat::from_rotation_y(elapsed * 0.8)
            * glam::Quat::from_rotation_x((elapsed * 0.5).sin() * 0.3);
        let t = glam::Mat4::from_rotation_translation(rot, glam::vec3(0.0, 2.5, 0.0));
        let _ = update_object_transform(
            &mut self.scene_db.world,
            &mut self.renderer,
            self.spin_crystal,
            t,
        );

        // ── Render ─────────────────────────────────────────────────────────

        v3_demo_common::flush_scene_db(&self.scene_db, &self.queue);
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
    }
}
