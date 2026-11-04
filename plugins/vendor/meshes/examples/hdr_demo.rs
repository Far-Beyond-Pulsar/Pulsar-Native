//! HDR Display Output Demo.
//!
//! Demonstrates HDR10 (PQ ST 2084), scRGB, and Passthrough output modes
//! with a scene containing emissive objects at extreme brightness levels.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Mouse drag  — look around (click to grab cursor)
//!   H           — cycle HDR output mode
//!   Escape      — release cursor / exit

mod v3_demo_common;

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    HdrOutputMode, Renderer, RendererBuilder, RendererConfig,
};
use v3_demo_common::{
    cube_mesh, directional_light, make_material, new_scene_db_with_gpu_mirror, plane_mesh,
    point_light, scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object,
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
    log::info!("Starting Helio HDR Demo");

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
    scene_db: pulsar_scenedb::SceneDb,
    last_frame: std::time::Instant,
    start_time: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    hdr_mode: u32,
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
                        .with_title("Helio — HDR Display Output")
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
                application_name: "Hdr Demo".into(),
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
        let graph_scene_db = scene_db_handle(&scene_db);
        let mut renderer = RendererBuilder::new(config, graph_scene_db.clone())
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;
        renderer.set_editor_mode(true);

        let white = spawn_material(
            &mut scene_db.world,
            make_material([0.9, 0.9, 0.92, 1.0], 0.6, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let emissive_red = spawn_material(
            &mut scene_db.world,
            make_material([1.0, 0.1, 0.1, 1.0], 0.3, 0.0, [10.0, 0.5, 0.5], 10.0),
        );
        let emissive_green = spawn_material(
            &mut scene_db.world,
            make_material([0.1, 1.0, 0.1, 1.0], 0.3, 0.0, [0.5, 10.0, 0.5], 10.0),
        );
        let emissive_blue = spawn_material(
            &mut scene_db.world,
            make_material([0.1, 0.1, 1.0, 1.0], 0.3, 0.0, [0.5, 0.5, 10.0], 10.0),
        );
        let emissive_sun = spawn_material(
            &mut scene_db.world,
            make_material([1.0, 0.9, 0.7, 1.0], 0.2, 0.0, [50.0, 45.0, 35.0], 50.0),
        );
        let metal = spawn_material(
            &mut scene_db.world,
            make_material([0.95, 0.93, 0.88, 1.0], 0.1, 1.0, [0.0, 0.0, 0.0], 0.0),
        );

        let ground = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 8.0));
        let _ = spawn_object(
            &mut scene_db.world,
            ground,
            white,
            glam::Mat4::IDENTITY,
            8.0,
        );

        let red_cube = spawn_mesh(&mut scene_db.world, cube_mesh([-1.5, 0.5, -1.0], 0.5));
        let _ = spawn_object(
            &mut scene_db.world,
            red_cube,
            emissive_red,
            glam::Mat4::IDENTITY,
            0.5,
        );

        let green_cube = spawn_mesh(&mut scene_db.world, cube_mesh([1.5, 0.5, -1.0], 0.5));
        let _ = spawn_object(
            &mut scene_db.world,
            green_cube,
            emissive_green,
            glam::Mat4::IDENTITY,
            0.5,
        );

        let blue_cube = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.5, 1.5], 0.5));
        let _ = spawn_object(
            &mut scene_db.world,
            blue_cube,
            emissive_blue,
            glam::Mat4::IDENTITY,
            0.5,
        );

        let metal_cube = spawn_mesh(&mut scene_db.world, cube_mesh([-1.5, 0.5, 2.5], 0.5));
        let _ = spawn_object(
            &mut scene_db.world,
            metal_cube,
            metal,
            glam::Mat4::IDENTITY,
            0.5,
        );

        let sun_sphere = spawn_mesh(&mut scene_db.world, cube_mesh([3.0, 4.0, -3.0], 0.4));
        let _ = spawn_object(
            &mut scene_db.world,
            sun_sphere,
            emissive_sun,
            glam::Mat4::IDENTITY,
            0.4,
        );

        spawn_light(
            &mut scene_db.world,
            directional_light([0.3, -0.8, 0.5], [1.0, 0.95, 0.85], 15.0),
        );
        spawn_light(
            &mut scene_db.world,
            point_light([3.0, 4.0, -3.0], [1.0, 0.9, 0.7], 20.0, 15.0),
        );

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            scene_db,
            surface_format,
            renderer,
            last_frame: std::time::Instant::now(),
            start_time: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 3.0, 6.0),
            cam_yaw: 0.0,
            cam_pitch: -0.3,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            hdr_mode: 0,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let state = match self.state.as_mut() {
            Some(s) => s,
            None => return,
        };

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::Escape),
                        state: ElementState::Released,
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
                        physical_key: PhysicalKey::Code(code),
                        state: key_state,
                        ..
                    },
                ..
            } => {
                if key_state == ElementState::Pressed {
                    state.keys.insert(code);
                    if code == KeyCode::KeyH {
                        state.hdr_mode = (state.hdr_mode + 1) % 4;
                        let name = match state.hdr_mode {
                            0 => "LDR (sRGB)",
                            1 => "HDR10 (PQ)",
                            2 => "scRGB",
                            _ => "Passthrough",
                        };
                        log::info!("[HDR Demo] Switched to {}", name);
                        state.window.set_title(&format!("Helio — HDR: {}", name));
                    }
                } else {
                    state.keys.remove(&code);
                }
            }
            WindowEvent::CursorMoved {
                device_id: _,
                position,
            } => {
                if state.cursor_grabbed {
                    let center = state.window.inner_size();
                    let dx = position.x as f32 - center.width as f32 / 2.0;
                    let dy = position.y as f32 - center.height as f32 / 2.0;
                    state.mouse_delta = (dx, dy);
                    let _ = state
                        .window
                        .set_cursor_position(winit::dpi::PhysicalPosition::new(
                            center.width as f64 / 2.0,
                            center.height as f64 / 2.0,
                        ));
                }
            }
            WindowEvent::MouseInput {
                button: MouseButton::Left,
                state: ElementState::Pressed,
                ..
            } => {
                state.cursor_grabbed = true;
                let _ = state.window.set_cursor_grab(CursorGrabMode::Confined);
                state.window.set_cursor_visible(false);
            }
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    let config = wgpu::SurfaceConfiguration {
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
                    state.surface.configure(&state.device, &config);
                    state.renderer.set_render_size(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                let now = std::time::Instant::now();
                let dt = (now - state.last_frame).as_secs_f32().min(0.05);
                state.last_frame = now;
                let time = state.start_time.elapsed().as_secs_f32();

                let speed = 5.0;
                let (sy, cy) = state.cam_yaw.sin_cos();
                let (sp, cp) = state.cam_pitch.sin_cos();
                let forward = glam::Vec3::new(sy * cp, sp, -cy * cp);
                let right = glam::Vec3::new(cy, 0.0, sy);

                if state.keys.contains(&KeyCode::KeyW) {
                    state.cam_pos += forward * speed * dt;
                }
                if state.keys.contains(&KeyCode::KeyS) {
                    state.cam_pos -= forward * speed * dt;
                }
                if state.keys.contains(&KeyCode::KeyA) {
                    state.cam_pos -= right * speed * dt;
                }
                if state.keys.contains(&KeyCode::KeyD) {
                    state.cam_pos += right * speed * dt;
                }
                if state.keys.contains(&KeyCode::Space) {
                    state.cam_pos.y += speed * dt;
                }
                if state.keys.contains(&KeyCode::ShiftLeft) {
                    state.cam_pos.y -= speed * dt;
                }

                state.cam_yaw -= state.mouse_delta.0 * 0.005;
                state.cam_pitch =
                    (state.cam_pitch - state.mouse_delta.1 * 0.005).clamp(-1.55, 1.55);
                state.mouse_delta = (0.0, 0.0);

                let aspect = state.window.inner_size().width as f32
                    / state.window.inner_size().height.max(1) as f32;

                let mut settings = helio_pass_postprocess::PostProcessSettings::default();
                let camera = Camera::perspective_look_at(
                    state.cam_pos,
                    state.cam_pos + forward,
                    glam::Vec3::Y,
                    std::f32::consts::FRAC_PI_4,
                    aspect,
                    0.1,
                    200.0,
                );
                match state.hdr_mode {
                    0 => {
                        settings.hdr_output_mode = HdrOutputMode::Ldr;
                        settings.tonemap_operator = helio::TonemapOperator::Aces;
                    }
                    1 => {
                        settings.hdr_output_mode = HdrOutputMode::Hdr10;
                        settings.tonemap_operator = helio::TonemapOperator::None;
                    }
                    2 => {
                        settings.hdr_output_mode = HdrOutputMode::ScRgb;
                        settings.tonemap_operator = helio::TonemapOperator::None;
                    }
                    _ => {
                        settings.hdr_output_mode = HdrOutputMode::Passthrough;
                        settings.tonemap_operator = helio::TonemapOperator::None;
                    }
                }

                v3_demo_common::set_post_baseline(&mut state.renderer, &settings);
                v3_demo_common::flush_scene_db(&state.scene_db, &state.queue);
                if let Err(e) = state.renderer.render_window_frame(&camera, &state.surface) {
                    log::error!("Render error: {:?}", e);
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}
