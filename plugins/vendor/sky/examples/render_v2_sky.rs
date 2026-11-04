//! The sky plugin's demo: Earth's atmosphere over a lit scene (the lighting
//! plugins' features over Helio's deferred preset).
//!
//! A simple scene with a sun directional light and three colored point lights.
//! Q/E keys rotate the sun, simulating time of day.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Q/E         — rotate sun left/right (changes time of day)
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit

mod common;

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererConfig,
};
use pulsar_scenedb::SceneDb;
use common::{
    box_mesh, build_default_renderer_with_context, cube_mesh, directional_light, make_material,
    new_scene_db_with_gpu_mirror, plane_mesh, point_light, spawn_light, spawn_material, spawn_mesh,
    spawn_object, spawn_sky, update_light,
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
    log::info!("Starting Helio Sky Example");

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

    // Free-camera state
    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    // Time-of-day: sun_angle=0 → noon, PI → midnight
    sun_angle: f32,

    // Scene state
    scene_db: SceneDb,
    sun_light_id: pulsar_scenedb::Entity,
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
                        .with_title("Helio – Volumetric Sky")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 720u32)),
                )
                .expect("Failed to create window"),
        );

        let size = window.inner_size();
        let mut graphics = pollster::block_on(helio::HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            helio::HelioGraphicsConfig {
                vr_support: true,
                application_name: "Render V2 Sky".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let surface_format = format;

        let config = RendererConfig::new(size.width, size.height, surface_format);
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let mut renderer = build_default_renderer_with_context(&scene_db, &mut graphics, config);
        let surface = graphics.surface;

        let mat = spawn_material(
            &mut scene_db.world,
            make_material([0.7, 0.7, 0.72, 1.0], 0.7, 0.0, [0.0, 0.0, 0.0], 0.0),
        );

        let cube1 = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.0, 0.0], 0.5));
        let cube2 = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.0, 0.0], 0.4));
        let cube3 = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.0, 0.0], 0.3));
        let ground = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 20.0));
        let roof = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [4.5, 0.15, 4.5]),
        );

        let _ = spawn_object(
            &mut scene_db.world,
            cube1,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.5, 0.0)),
            0.5,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            cube2,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-2.0, 0.4, -1.0)),
            0.4,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            cube3,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(2.0, 0.3, 0.5)),
            0.3,
        );
        let _ = spawn_object(&mut scene_db.world, ground, mat, glam::Mat4::IDENTITY, 20.0);
        let _ = spawn_object(
            &mut scene_db.world,
            roof,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 2.85, 0.0)),
            4.5,
        );

        // Compute initial sun direction from starting sun_angle=1.0
        let init_sun_dir = glam::Vec3::new(1.0_f32.cos() * 0.3, 1.0_f32.sin(), 0.5).normalize();
        let init_light_dir = [-init_sun_dir.x, -init_sun_dir.y, -init_sun_dir.z];
        let init_elev = init_sun_dir.y.clamp(-1.0, 1.0);
        let init_lux = (init_elev * 3.0).clamp(0.0, 1.0);
        let sun_light_id = spawn_light(
            &mut scene_db.world,
            directional_light(
                init_light_dir,
                [1.0, 0.85, 0.7],
                (init_lux * 0.35).max(0.01),
            ),
        );
        spawn_light(
            &mut scene_db.world,
            point_light([0.0, 2.5, 0.0], [1.0, 0.85, 0.6], 4.0, 8.0),
        );
        spawn_light(
            &mut scene_db.world,
            point_light([-2.5, 2.0, -1.5], [0.4, 0.6, 1.0], 3.5, 7.0),
        );
        spawn_light(
            &mut scene_db.world,
            point_light([2.5, 1.8, 1.5], [1.0, 0.3, 0.3], 3.0, 6.0),
        );
        spawn_sky(&mut scene_db.world, [0.15, 0.18, 0.25]);

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format,
            renderer,
            scene_db,
            last_frame: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 2.5, 7.0),
            cam_yaw: 0.0,
            cam_pitch: -0.2,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            // Start at a nice afternoon angle (sun ~50° above horizon)
            sun_angle: 1.0,
            sun_light_id,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

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
                    let grabbed = state
                        .window
                        .set_cursor_grab(CursorGrabMode::Confined)
                        .or_else(|_| state.window.set_cursor_grab(CursorGrabMode::Locked))
                        .is_ok();
                    if grabbed {
                        state.window.set_cursor_visible(false);
                        state.cursor_grabbed = true;
                    }
                }
            }

            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                let cfg = wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format: state.surface_format,
                    width: size.width,
                    height: size.height,
                    present_mode: wgpu::PresentMode::Fifo,
                    alpha_mode: wgpu::CompositeAlphaMode::Auto,
                    view_formats: vec![],
                    desired_maximum_frame_latency: 2,
                    color_space: wgpu::SurfaceColorSpace::Auto,
                };
                state.surface.configure(&state.device, &cfg);
                state.renderer.set_render_size(size.width, size.height);
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
        const SPEED: f32 = 5.0;
        const LOOK_SENS: f32 = 0.002;
        const SUN_SPEED: f32 = 0.5; // radians/sec

        // Sun rotation (Q/E keys)
        if self.keys.contains(&KeyCode::KeyQ) {
            self.sun_angle -= SUN_SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyE) {
            self.sun_angle += SUN_SPEED * dt;
        }

        // Camera look
        self.cam_yaw += self.mouse_delta.0 * LOOK_SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * LOOK_SENS).clamp(-1.5, 1.5);
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

        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            1000.0,
        );

        // Sun direction: orbits in the XY plane (rotate sun_angle around Z axis)
        let sun_dir =
            glam::Vec3::new(self.sun_angle.cos() * 0.3, self.sun_angle.sin(), 0.5).normalize();
        // SceneLight direction = "ray direction" (toward scene), so negate the toward-sun vector
        let light_dir = [-sun_dir.x, -sun_dir.y, -sun_dir.z];

        // Sun intensity dims at horizon/night
        let sun_elev = sun_dir.y.clamp(-1.0, 1.0);
        let sun_lux = (sun_elev * 3.0).clamp(0.0, 1.0);
        let sun_color = [
            1.0_f32.min(1.0 + (1.0 - sun_elev) * 0.3), // warmer at horizon
            (0.85 + sun_elev * 0.15).clamp(0.0, 1.0),
            (0.7 + sun_elev * 0.3).clamp(0.0, 1.0),
        ];

        // Update dynamic sun light
        update_light(
            &mut self.scene_db.world,
            self.sun_light_id,
            directional_light(light_dir, sun_color, (sun_lux * 0.35).max(0.01)),
        );
        // Upload this frame's SceneDB writes to the GPU mirror; without it they
        // are never visible to the renderer (Helio#266).
        common::flush_scene_db(&self.scene_db, self.renderer.queue());
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
    }
}
