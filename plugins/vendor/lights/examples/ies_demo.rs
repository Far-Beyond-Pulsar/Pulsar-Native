//! IES Light Profile Demo.
//!
//! Three spot lights in a dark scene:
//!   - Narrow spot (IES-like cone)
//!   - Medium spot
//!   - Wide flood with gobo projection
//!
//! Controls:
//!   WASD        — move
//!   Space/Shift — up/down
//!   Mouse drag  — look
//!   1/2/3       — toggle IES on spot 1/2/3
//!   G           — toggle gobo on all lights
//!   Escape      — release / exit

mod common;

use common::{
    make_material, new_scene_db_with_gpu_mirror, plane_mesh, scene_db_handle, spawn_light,
    spawn_material, spawn_mesh, spawn_object, update_light, SurfaceCapture,
};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig, TonemapOperator,
};
use pulsar_scenedb::{Entity, SceneDb};

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
    log::info!("Starting IES Light Profile Demo");

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
    capture: SurfaceCapture,
    last_frame: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),
    ies_enabled: [bool; 3],
    gobo_enabled: bool,
    scene_db: SceneDb,
    light_ids: [Entity; 3],
}

impl App {
    fn new() -> Self {
        Self { state: None }
    }

    fn update_lights(state: &mut AppState) {
        for (i, &enabled) in state.ies_enabled.iter().enumerate() {
            let mut light = helio_light_api::GpuLight::default();
            light.light_type = helio_light_api::LightType::Spot as u32;
            light.color_intensity = match i {
                0 => [1.0, 0.3, 0.2, 8.0], // red-tinted narrow
                1 => [0.2, 1.0, 0.3, 8.0], // green-tinted medium
                _ => [0.3, 0.4, 1.0, 8.0], // blue-tinted wide
            };
            light.direction_outer = match i {
                0 => [0.0, -1.0, 0.0, 0.85], // narrow (outer cos=0.85 ≈ 32°)
                1 => [0.0, -1.0, 0.0, 0.75], // medium (outer cos=0.75 ≈ 41°)
                _ => [0.0, -1.0, 0.0, 0.50], // wide (outer cos=0.50 ≈ 60°)
            };
            light.position_range = match i {
                0 => [-1.5, 3.0, -1.0, 8.0],
                1 => [1.5, 3.0, -1.0, 8.0],
                _ => [0.0, 3.0, 2.0, 8.0],
            };
            light.inner_angle = match i {
                0 => 0.98, // very tight hotspot
                1 => 0.92,
                _ => 0.80,
            };
            if enabled {
                light.ies_profile_index = 0;
                light.ies_angle_scale = match i {
                    0 => 0.5,
                    1 => 1.0,
                    _ => 2.0,
                };
            }
            if state.gobo_enabled {
                light.light_function_index = 1;
            }
            update_light(&mut state.scene_db.world, state.light_ids[i], light);
        }
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
                        .with_title("Helio — IES Light Profiles")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 720u32)),
                )
                .expect("Failed to create window"),
        );

        let size = window.inner_size();
        let capture = SurfaceCapture::from_env();
        let mut graphics = pollster::block_on(helio::HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            helio::HelioGraphicsConfig {
                vr_support: true,
                application_name: "Ies Demo".into(),
                surface_usage: capture.usage(),
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

        // Dark floor
        let floor_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.15, 0.15, 0.16, 1.0], 0.8, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let ground = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 6.0));
        let _ = spawn_object(
            &mut scene_db.world,
            ground,
            floor_mat,
            glam::Mat4::IDENTITY,
            6.0,
        );

        // Spot lights
        let light_ids: [Entity; 3] = std::array::from_fn(|i| {
            let mut light = helio_light_api::GpuLight::default();
            light.light_type = helio_light_api::LightType::Spot as u32;
            light.color_intensity = match i {
                0 => [1.0, 0.3, 0.2, 8.0],
                1 => [0.2, 1.0, 0.3, 8.0],
                _ => [0.3, 0.4, 1.0, 8.0],
            };
            light.direction_outer = match i {
                0 => [0.0, -1.0, 0.0, 0.85],
                1 => [0.0, -1.0, 0.0, 0.75],
                _ => [0.0, -1.0, 0.0, 0.50],
            };
            light.position_range = match i {
                0 => [-1.5, 3.0, -1.0, 8.0],
                1 => [1.5, 3.0, -1.0, 8.0],
                _ => [0.0, 3.0, 2.0, 8.0],
            };
            light.inner_angle = match i {
                0 => 0.98,
                1 => 0.92,
                _ => 0.80,
            };
            spawn_light(&mut scene_db.world, light)
        });

        // Upload a 2-layer IES texture array: layer 0 = spotlight gradient, layer 1 = checkerboard gobo
        const IES_W: u32 = 64;
        const IES_H: u32 = 64;
        let mut ies_pixels = Vec::with_capacity((IES_W * IES_H * 2) as usize);
        // Layer 0: gaussian spotlight
        for y in 0..IES_H {
            for x in 0..IES_W {
                let u = (x as f32 + 0.5) / IES_W as f32 * 2.0 - 1.0;
                let v = (y as f32 + 0.5) / IES_H as f32 * 2.0 - 1.0;
                let dist = (u * u + v * v).sqrt();
                let val = (-dist * dist * 6.0).exp(); // tight gaussian
                ies_pixels.push((val.clamp(0.0, 1.0) * 255.0) as u8);
            }
        }
        // Layer 1: checkerboard gobo
        for y in 0..IES_H {
            for x in 0..IES_W {
                let tile = ((x / 6) + (y / 6)) & 1;
                ies_pixels.push(if tile == 0 { 235u8 } else { 30u8 });
            }
        }
        let ies_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Demo IES Textures"),
            size: wgpu::Extent3d {
                width: IES_W,
                height: IES_H,
                depth_or_array_layers: 2,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // Write layer 0
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &ies_tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &ies_pixels[..(IES_W * IES_H) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(IES_W),
                rows_per_image: Some(IES_H),
            },
            wgpu::Extent3d {
                width: IES_W,
                height: IES_H,
                depth_or_array_layers: 1,
            },
        );
        // Write layer 1
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &ies_tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: 1 },
                aspect: wgpu::TextureAspect::All,
            },
            &ies_pixels[(IES_W * IES_H) as usize..],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(IES_W),
                rows_per_image: Some(IES_H),
            },
            wgpu::Extent3d {
                width: IES_W,
                height: IES_H,
                depth_or_array_layers: 1,
            },
        );
        let ies_view = ies_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        renderer.set_feature(helio_pass_deferred_light::IesProfiles(ies_view));

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format,
            renderer,
            capture,
            last_frame: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 2.0, 5.0),
            cam_yaw: 0.0,
            cam_pitch: -0.2,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            ies_enabled: [false, false, false],
            gobo_enabled: false,
            scene_db,
            light_ids,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let state = match self.state.as_mut() {
            Some(s) => s,
            None => return,
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
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
                            state.ies_enabled[0] = !state.ies_enabled[0];
                            true
                        }
                        KeyCode::Digit2 => {
                            state.ies_enabled[1] = !state.ies_enabled[1];
                            true
                        }
                        KeyCode::Digit3 => {
                            state.ies_enabled[2] = !state.ies_enabled[2];
                            true
                        }
                        KeyCode::KeyG => {
                            state.gobo_enabled = !state.gobo_enabled;
                            true
                        }
                        _ => false,
                    };
                    if changed {
                        App::update_lights(state);
                        let status = format!(
                            "IES: {},{},{}  Gobo: {}",
                            if state.ies_enabled[0] { "ON" } else { "OFF" },
                            if state.ies_enabled[1] { "ON" } else { "OFF" },
                            if state.ies_enabled[2] { "ON" } else { "OFF" },
                            if state.gobo_enabled { "ON" } else { "OFF" },
                        );
                        state
                            .window
                            .set_title(&format!("Helio — IES Light Profiles  |  {}", status));
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
                        usage: state.capture.usage(),
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

                let camera = Camera::perspective_look_at(
                    state.cam_pos,
                    state.cam_pos + forward,
                    glam::Vec3::Y,
                    std::f32::consts::FRAC_PI_4,
                    aspect,
                    0.1,
                    200.0,
                );

                // Upload this frame's SceneDB writes to the GPU mirror; without it they
                // are never visible to the renderer (Helio#266).
                common::flush_scene_db(&state.scene_db, state.renderer.queue());
                if let Err(e) =
                    state
                        .capture
                        .render_window_frame(&mut state.renderer, &camera, &state.surface)
                {
                    log::error!("Render error: {:?}", e);
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}
