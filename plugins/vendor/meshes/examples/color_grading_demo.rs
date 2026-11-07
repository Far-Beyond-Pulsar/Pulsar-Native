//! Color Grading Demo — interactive lift/gamma/gain, hue shift, LUT.
//!
//! Controls:
//!   WASD        — move
//!   Space/Shift — up/down
//!   Mouse drag  — look
//!   1/2/3       — cycle grading mode (simple / lift-gamma-gain / LUT)
//!   Q/E         — adjust hue shift
//!   R/F         — adjust LUT intensity
//!   T/G         — cycle LUT size (16/32/64)
//!   Escape      — release / exit

mod v3_demo_common;

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    HdrOutputMode, Renderer, RendererConfig, TonemapOperator,
};
use helio_pass_postprocess::LutBuilder;
use pulsar_scenedb::SceneDb;
use v3_demo_common::{
    build_default_renderer_with_context, cube_mesh, make_material, new_scene_db_with_gpu_mirror,
    plane_mesh, point_light, spawn_light, spawn_material, spawn_mesh, spawn_object,
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
    log::info!("Starting Color Grading Demo");

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
    scene_db: SceneDb,
    last_frame: std::time::Instant,
    start_time: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    // Grading state
    grading_mode: u32, // 0=simple, 1=lift-gamma-gain, 2=LUT
    hue_shift: f32,
    lut_intensity: f32,
    lut_size: u32,
    lut_generation: u32,

    // GPU LUT builder (manual dispatch before render)
    lut_builder: Option<LutBuilder>,
    /// The settings the LUT is built from (this host's, as the tone map's
    /// baseline sees them).
    settings_buffer: wgpu::Buffer,
}

impl App {
    fn new() -> Self {
        Self { state: None }
    }

    fn update_title(state: &AppState) {
        let mode = match state.grading_mode {
            0 => "Simple",
            1 => "Lift-Gamma-Gain",
            _ => "3D LUT",
        };
        let title = format!(
            "Color Grading — {} | Hue: {:.0}° | LUT: {:.0}% | {}³",
            mode,
            state.hue_shift,
            state.lut_intensity * 100.0,
            state.lut_size,
        );
        state.window.set_title(&title);
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
                        .with_title("Helio — Color Grading Demo")
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
                application_name: "Color Grading Demo".into(),
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

        spawn_light(
            &mut scene_db.world,
            point_light([3.0, 4.0, -3.0], [1.0, 0.9, 0.7], 20.0, 15.0),
        );

        // Create LUT builder
        let lut_builder = LutBuilder::new(&device, 16);
        let settings_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Color Grading Demo Settings"),
            size: std::mem::size_of::<helio_pass_postprocess::GpuPostProcessUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format,
            renderer,
            scene_db,
            last_frame: std::time::Instant::now(),
            start_time: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 3.0, 6.0),
            cam_yaw: 0.0,
            cam_pitch: -0.3,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            grading_mode: 0,
            hue_shift: 0.0,
            lut_intensity: 1.0,
            lut_size: 16,
            lut_generation: 1,
            lut_builder: Some(lut_builder),
            settings_buffer,
        });

        App::update_title(self.state.as_ref().unwrap());
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
                    let changed = match code {
                        KeyCode::Digit1 => {
                            state.grading_mode = 0;
                            true
                        }
                        KeyCode::Digit2 => {
                            state.grading_mode = 1;
                            true
                        }
                        KeyCode::Digit3 => {
                            state.grading_mode = 2;
                            state.lut_generation += 1;
                            true
                        }
                        KeyCode::KeyQ => {
                            state.hue_shift = (state.hue_shift - 5.0).clamp(-180.0, 180.0);
                            true
                        }
                        KeyCode::KeyE => {
                            state.hue_shift = (state.hue_shift + 5.0).clamp(-180.0, 180.0);
                            true
                        }
                        KeyCode::KeyR => {
                            state.lut_intensity = (state.lut_intensity - 0.1).clamp(0.0, 1.0);
                            true
                        }
                        KeyCode::KeyF => {
                            state.lut_intensity = (state.lut_intensity + 0.1).clamp(0.0, 1.0);
                            true
                        }
                        KeyCode::KeyT => {
                            state.lut_size = match state.lut_size {
                                16 => 32,
                                32 => 64,
                                _ => 16,
                            };
                            if let Some(ref mut b) = state.lut_builder {
                                b.on_resize(&state.device, state.lut_size);
                            }
                            state.lut_generation += 1;
                            true
                        }
                        _ => false,
                    };
                    if changed {
                        App::update_title(state);
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
                settings.tonemap_operator = TonemapOperator::Aces;

                // Apply grading mode
                match state.grading_mode {
                    0 => {
                        // Simple: contrast + saturation boost
                        settings.lut_platform = 0;
                        settings.color_contrast = [1.2, 1.2, 1.2];
                        settings.color_saturation = [1.3, 1.3, 1.3];
                    }
                    1 => {
                        // Lift/Gamma/Gain
                        settings.lut_platform = 0;
                        settings.lift_color = [0.02, 0.01, 0.04]; // slight purple shadows
                        settings.gamma_color = [0.95, 1.0, 1.05];
                        settings.gain_color = [1.1, 1.0, 0.95]; // warm highlights
                        settings.hue_shift = state.hue_shift;
                    }
                    _ => {
                        // 3D LUT
                        settings.lut_platform = 1;
                        settings.lut_intensity = state.lut_intensity;
                        settings.hue_shift = state.hue_shift;
                    }
                }

                state.queue.write_buffer(
                    &state.settings_buffer,
                    0,
                    bytemuck::bytes_of(&settings.to_gpu()),
                );
                let resolved = state.settings_buffer.clone();
                // Build LUT if needed
                if let Some(ref mut builder) = state.lut_builder {
                    let rebuilt = builder.build_if_needed(
                        &state.device,
                        &state.queue,
                        &resolved,
                        state.lut_generation,
                    );
                    // The builder rewrites one texture: the tone map is given its
                    // view once (a feature setting, so the graph rebuilds once).
                    let _ = rebuilt;
                    if state
                        .renderer
                        .feature::<helio_pass_postprocess::ColorGradingLut>()
                        .is_none()
                    {
                        let view = builder.lut_view().clone();
                        state
                            .renderer
                            .set_feature(helio_pass_postprocess::ColorGradingLut(view));
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
