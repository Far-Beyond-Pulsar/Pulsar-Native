//! Outdoor night plaza example – medium complexity
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit

mod v3_demo_common;

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use pulsar_scenedb::Entity;
use v3_demo_common::{
    box_mesh, make_material, new_scene_db_with_gpu_mirror, plane_mesh, point_light,
    scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object,
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
    /// Kept so its rows keep reaching the GPU mirror (Helio#266).
    scene_db: pulsar_scenedb::SceneDb,
    last_frame: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    _light_ids: Vec<Entity>,
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
                        .with_title("Helio – Outdoor Night Plaza")
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
                application_name: "Outdoor Night".into(),
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
            make_material([0.7, 0.7, 0.72, 1.0], 0.8, 0.0, [0.0, 0.0, 0.0], 0.0),
        );

        let ground = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 20.0));
        let bld_a = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [2.5, 7.0, 2.5]),
        );
        let bld_b = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [3.0, 4.5, 2.0]),
        );
        let bld_c = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [2.0, 3.0, 3.0]),
        );
        let bld_d = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [3.5, 1.5, 2.5]),
        );
        let bld_e = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [4.0, 9.5, 3.0]),
        );
        let lamp_pole_a = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.08, 2.5, 0.08]),
        );
        let lamp_pole_b = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.08, 2.5, 0.08]),
        );
        let lamp_pole_c = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.08, 2.5, 0.08]),
        );
        let lamp_pole_d = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.08, 2.5, 0.08]),
        );

        let _ = spawn_object(&mut scene_db.world, ground, mat, glam::Mat4::IDENTITY, 20.0);
        let _ = spawn_object(
            &mut scene_db.world,
            bld_a,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(8.0, 7.0, -6.0)),
            7.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            bld_b,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-7.0, 4.5, -5.0)),
            4.5,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            bld_c,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(6.0, 3.0, 6.0)),
            3.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            bld_d,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-5.0, 1.5, 5.0)),
            3.5,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            bld_e,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 9.5, -14.0)),
            9.5,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            lamp_pole_a,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-5.0, 2.5, -5.0)),
            2.5,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            lamp_pole_b,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(5.0, 2.5, -5.0)),
            2.5,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            lamp_pole_c,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-5.0, 2.5, 5.0)),
            2.5,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            lamp_pole_d,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(5.0, 2.5, 5.0)),
            2.5,
        );

        let mut _light_ids: Vec<Entity> = Vec::new();
        _light_ids.push(spawn_light(
            &mut scene_db.world,
            point_light([-5.0, 5.1, -5.0], [1.0, 0.72, 0.3], 6.0, 14.0),
        ));
        _light_ids.push(spawn_light(
            &mut scene_db.world,
            point_light([5.0, 5.1, -5.0], [1.0, 0.72, 0.3], 6.0, 14.0),
        ));
        _light_ids.push(spawn_light(
            &mut scene_db.world,
            point_light([-5.0, 5.1, 5.0], [1.0, 0.72, 0.3], 6.0, 14.0),
        ));
        _light_ids.push(spawn_light(
            &mut scene_db.world,
            point_light([5.0, 5.1, 5.0], [1.0, 0.72, 0.3], 6.0, 14.0),
        ));
        _light_ids.push(spawn_light(
            &mut scene_db.world,
            point_light([8.0, 12.0, -5.8], [1.0, 0.05, 0.8], 5.0, 12.0),
        ));
        _light_ids.push(spawn_light(
            &mut scene_db.world,
            point_light([0.0, 16.5, -14.0], [0.05, 0.9, 1.0], 4.0, 10.0),
        ));
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.1, 0.15, 0.3],
            intensity: 0.06,
        });
        renderer.set_clear_color([0.005, 0.005, 0.025, 1.0]);

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            renderer,
            scene_db,
            last_frame: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 3.0, 12.0),
            cam_yaw: std::f32::consts::PI,
            cam_pitch: -0.15,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            _light_ids,
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
        const SPEED: f32 = 6.0;
        const SENS: f32 = 0.002;

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

        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            200.0,
        );

        // Upload this frame's SceneDB writes to the GPU mirror; without it they
        // are never visible to the renderer (Helio#266).
        v3_demo_common::flush_scene_db(&self.scene_db, self.renderer.queue());
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
    }
}
