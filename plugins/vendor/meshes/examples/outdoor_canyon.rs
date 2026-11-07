//! Outdoor canyon example – high complexity
//!
//! A dramatic desert canyon at golden hour.  The sun is low on the horizon
//! (controllable with Q/E), casting long shadows across layered rock terraces.
//! Three campfire-orange point lights sit in a valley camp.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Q/E         — rotate sun (time of day)
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit

mod v3_demo_common;

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use pulsar_scenedb::{Entity, SceneDb};
use v3_demo_common::{
    box_mesh, cube_mesh, directional_light, make_material, new_scene_db_with_gpu_mirror,
    plane_mesh, point_light, scene_db_handle, spawn_light, spawn_material, spawn_mesh,
    spawn_object, update_light,
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
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("Event loop error");
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
    last_frame: std::time::Instant,
    start_time: std::time::Instant,

    scene_db: SceneDb,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),
    sun_angle: f32,

    sun_light_id: Entity,
    fire_light_id: Entity,
    _moon_light_id: Entity,
    _ember_ids: Vec<Entity>,
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
                        .with_title("Helio – Outdoor Canyon")
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
                application_name: "Outdoor Canyon".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let surface_format = format;

        let config = RendererConfig::new(size.width, size.height, format);
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let graph_scene_db = scene_db_handle(&scene_db);
        let mut renderer = RendererBuilder::new(config, graph_scene_db.clone())
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;

        let mat = spawn_material(
            &mut scene_db.world,
            make_material([0.72, 0.58, 0.42, 1.0], 0.85, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let fire_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.3, 0.1, 0.05, 1.0], 0.9, 0.0, [1.0, 0.4, 0.05], 4.0),
        );

        let valley_floor = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 35.0));
        let wall_l1 = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [3.0, 4.0, 30.0]),
        );
        let wall_l2 = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [3.0, 8.0, 25.0]),
        );
        let wall_l3 = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [3.0, 14.0, 20.0]),
        );
        let wall_r1 = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [3.0, 4.0, 30.0]),
        );
        let wall_r2 = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [3.0, 8.0, 25.0]),
        );
        let wall_r3 = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [3.0, 14.0, 20.0]),
        );
        let terrace_l1 = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [1.5, 0.2, 12.0]),
        );
        let terrace_l2 = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [1.5, 0.2, 8.0]),
        );
        let terrace_r1 = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [1.5, 0.2, 12.0]),
        );
        let terrace_r2 = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [1.5, 0.2, 8.0]),
        );
        let mesa = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [10.0, 12.0, 8.0]),
        );
        let tent_a = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.8, 0.6, 1.2]),
        );
        let tent_b = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.9, 0.7, 1.3]),
        );
        let tent_c = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.7, 0.55, 1.1]),
        );
        let firepit = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.0, 0.0], 0.2));

        let _ = spawn_object(
            &mut scene_db.world,
            valley_floor,
            mat,
            glam::Mat4::IDENTITY,
            35.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            wall_l1,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-12.0, 4.0, 0.0)),
            20.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            wall_l2,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-18.0, 8.0, 0.0)),
            20.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            wall_l3,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-24.0, 14.0, 0.0)),
            20.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            wall_r1,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(12.0, 4.0, 0.0)),
            20.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            wall_r2,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(18.0, 8.0, 0.0)),
            20.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            wall_r3,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(24.0, 14.0, 0.0)),
            20.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            terrace_l1,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-13.5, 8.1, -2.0)),
            10.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            terrace_l2,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-19.5, 16.1, -4.0)),
            8.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            terrace_r1,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(13.5, 8.1, -2.0)),
            10.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            terrace_r2,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(19.5, 16.1, -4.0)),
            8.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            mesa,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(3.0, 12.0, -38.0)),
            14.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            tent_a,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-2.5, 0.6, 8.0)),
            1.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            tent_b,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.7, 7.5)),
            1.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            tent_c,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(2.8, 0.55, 8.5)),
            1.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            firepit,
            fire_mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.15, 9.5)),
            0.3,
        );

        let fire_pos = [0.0f32, 0.5, 9.5];
        let moon_dir = glam::Vec3::new(0.4, -0.7, 0.3).normalize();
        let sun_light_id = spawn_light(
            &mut scene_db.world,
            directional_light([-0.0, -1.0, -0.5], [1.0, 0.9, 0.7], 0.005),
        );
        let fire_light_id = spawn_light(
            &mut scene_db.world,
            point_light(fire_pos, [1.0, 0.45, 0.1], 5.0, 12.0),
        );
        let ember_a_id = spawn_light(
            &mut scene_db.world,
            point_light([-0.4, 0.4, 9.2], [1.0, 0.35, 0.05], 1.5, 5.0),
        );
        let ember_b_id = spawn_light(
            &mut scene_db.world,
            point_light([0.4, 0.4, 9.8], [1.0, 0.35, 0.05], 1.5, 5.0),
        );
        let moon_light_id = spawn_light(
            &mut scene_db.world,
            directional_light([moon_dir.x, moon_dir.y, moon_dir.z], [0.5, 0.65, 1.0], 0.05),
        );
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.6, 0.55, 0.45],
            intensity: 0.08,
        });
        renderer.set_clear_color([0.45, 0.6, 0.85, 1.0]);

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            renderer,
            last_frame: std::time::Instant::now(),
            start_time: std::time::Instant::now(),
            scene_db,
            cam_pos: glam::Vec3::new(0.0, 4.0, 25.0),
            cam_yaw: 0.0,
            cam_pitch: -0.15,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            sun_angle: 0.45,
            sun_light_id,
            fire_light_id,
            _moon_light_id: moon_light_id,
            _ember_ids: vec![ember_a_id, ember_b_id],
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
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
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        format: state.surface_format,
                        width: s.width,
                        height: s.height,
                        present_mode: wgpu::PresentMode::Fifo,
                        alpha_mode: wgpu::CompositeAlphaMode::Auto,
                        view_formats: vec![],
                        desired_maximum_frame_latency: 2,
                        color_space: wgpu::SurfaceColorSpace::Auto,
                    },
                );
                state.renderer.set_render_size(s.width, s.height);
            }
            WindowEvent::RedrawRequested => {
                let now = std::time::Instant::now();
                let dt = (now - state.last_frame).as_secs_f32();
                state.last_frame = now;
                state.render(dt);
                state.window.request_redraw();
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
        if let Some(s) = &self.state {
            s.window.request_redraw();
        }
    }
}

impl AppState {
    fn render(&mut self, dt: f32) {
        const SPEED: f32 = 8.0;
        const SENS: f32 = 0.002;
        const SUN_SPEED: f32 = 0.4;

        if self.keys.contains(&KeyCode::KeyQ) {
            self.sun_angle -= SUN_SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyE) {
            self.sun_angle += SUN_SPEED * dt;
        }

        self.cam_yaw += self.mouse_delta.0 * SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * SENS).clamp(-1.4, 1.4);
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
            self.cam_pos += glam::Vec3::Y * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::ShiftLeft) {
            self.cam_pos -= glam::Vec3::Y * SPEED * dt;
        }

        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;
        let time = self.start_time.elapsed().as_secs_f32();

        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            1000.0,
        );

        // Sun direction
        let sun_dir =
            glam::Vec3::new(self.sun_angle.cos() * 0.3, self.sun_angle.sin(), 0.5).normalize();
        let light_dir = [-sun_dir.x, -sun_dir.y, -sun_dir.z];
        let sun_elev = sun_dir.y.clamp(-1.0, 1.0);
        let sun_lux = (sun_elev * 3.0).clamp(0.0, 1.0);
        let warmth = (1.0 - sun_elev).clamp(0.0, 1.0);
        let sun_color = [
            1.0_f32.min(1.0 + warmth * 0.5),
            (0.75 + sun_elev * 0.25).clamp(0.0, 1.0),
            (0.55 + sun_elev * 0.35).clamp(0.0, 1.0),
        ];

        // Campfire flicker
        let flicker = 1.0 + (time * 13.1).sin() * 0.08 + (time * 7.3).cos() * 0.05;
        let fire_pos = [0.0f32, 0.5, 9.5];

        update_light(
            &mut self.scene_db.world,
            self.sun_light_id,
            directional_light(light_dir, sun_color, (sun_lux * 0.4).max(0.005)),
        );
        update_light(
            &mut self.scene_db.world,
            self.fire_light_id,
            point_light(fire_pos, [1.0, 0.45, 0.1], 5.0 * flicker, 12.0),
        );

        v3_demo_common::flush_scene_db(&self.scene_db, &self.queue);
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
    }
}
