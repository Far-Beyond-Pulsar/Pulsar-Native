//! Lens Flare Demo — an emissive lamp seen through the camera's lens.
//!
//! Lens response is authored on the camera (CameraPostProcessComponent), not
//! on lights: the per-light `flare_*` fields below are legacy and inert.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit

mod common;

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, GpuLight,
    HelioGraphicsConfig, HelioGraphicsContext, LightType, Renderer, RendererBuilder,
    RendererConfig,
};
use common::{
    box_mesh, make_material, new_scene_db_with_gpu_mirror, plane_mesh, scene_db_handle,
    spawn_light, spawn_material, spawn_mesh, spawn_object,
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
    if std::env::args().any(|a| a == "--probe") {
        return probe();
    }
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

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    velocity: glam::Vec3,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),
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
                        .with_title("Helio — Lens Flare Demo")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 720u32)),
                )
                .expect("window"),
        );

        let size = window.inner_size();
        let mut graphics = pollster::block_on(HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            HelioGraphicsConfig {
                vr_support: true,
                application_name: "Lens Flare Demo".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let adapter = graphics.adapter.clone();
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        device.on_uncaptured_error(std::sync::Arc::new(|e: wgpu::Error| {
            panic!("[GPU UNCAPTURED ERROR] {:?}", e);
        }));
        let info = adapter.get_info();
        println!(
            "[WGPU] Backend: {:?}, Device: {}, Driver: {}",
            info.backend, info.name, info.driver
        );
        let format = graphics.surface_config.format;

        let (scene_db, renderer) = build_scene(
            device.clone(),
            queue.clone(),
            size.width,
            size.height,
            format,
            Some(&mut graphics),
        );
        let surface = graphics.surface;

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            renderer,
            scene_db,
            last_frame: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 1.2, 3.5),
            cam_yaw: 0.0,
            cam_pitch: -0.1,
            velocity: glam::Vec3::ZERO,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
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
                    state.window.set_cursor_visible(true);
                    let _ = state.window.set_cursor_grab(CursorGrabMode::None);
                } else {
                    event_loop.exit();
                }
            }

            WindowEvent::Resized(size) => {
                state.surface.configure(
                    &state.device,
                    &wgpu::SurfaceConfiguration {
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        format: state.surface_format,
                        width: size.width,
                        height: size.height,
                        present_mode: wgpu::PresentMode::Fifo,
                        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
                        view_formats: vec![],
                        desired_maximum_frame_latency: 2,
                        color_space: wgpu::SurfaceColorSpace::Auto,
                    },
                );
                state.renderer.set_render_size(size.width, size.height);
            }

            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: key_state,
                        ..
                    },
                ..
            } => match key_state {
                ElementState::Pressed => {
                    state.keys.insert(code);
                }
                ElementState::Released => {
                    state.keys.remove(&code);
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
                        state.cursor_grabbed = true;
                        state.window.set_cursor_visible(false);
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                let now = std::time::Instant::now();
                let dt = now.duration_since(state.last_frame).as_secs_f32().min(0.05);
                state.last_frame = now;
                state.update(dt);

                let size = state.window.inner_size();
                let camera = state.camera(size.width, size.height);

                common::flush_scene_db(&state.scene_db, &state.queue);
                if let Err(e) = state.renderer.render_window_frame(&camera, &state.surface) {
                    log::error!("Render error: {:?}", e);
                }
            }

            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let Some(state) = &mut self.state else { return };
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if state.cursor_grabbed {
                state.mouse_delta.0 += dx as f32;
                state.mouse_delta.1 += dy as f32;
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

// ── fly-camera helpers ─────────────────────────────────────────────────────────

const LOOK_SENS: f32 = 0.002;
const FLY_SPEED: f32 = 3.0;
const DRAG: f32 = 8.0;

impl AppState {
    fn update(&mut self, dt: f32) {
        let (dx, dy) = self.mouse_delta;
        self.mouse_delta = (0.0, 0.0);
        self.cam_yaw -= dx * LOOK_SENS;
        self.cam_pitch = (self.cam_pitch - dy * LOOK_SENS).clamp(-1.5, 1.5);

        let orientation =
            glam::Quat::from_euler(glam::EulerRot::YXZ, self.cam_yaw, self.cam_pitch, 0.0);
        let forward = orientation * -glam::Vec3::Z;
        let right = orientation * glam::Vec3::X;
        let up = glam::Vec3::Y;

        let mut accel = glam::Vec3::ZERO;
        if self.keys.contains(&KeyCode::KeyW) {
            accel += forward;
        }
        if self.keys.contains(&KeyCode::KeyS) {
            accel -= forward;
        }
        if self.keys.contains(&KeyCode::KeyA) {
            accel -= right;
        }
        if self.keys.contains(&KeyCode::KeyD) {
            accel += right;
        }
        if self.keys.contains(&KeyCode::Space) {
            accel += up;
        }
        if self.keys.contains(&KeyCode::ShiftLeft) {
            accel -= up;
        }
        if accel.length_squared() > 0.0 {
            accel = accel.normalize();
        }

        self.velocity += accel * FLY_SPEED * dt;
        self.velocity /= 1.0 + DRAG * dt;
        self.cam_pos += self.velocity * dt;
    }

    fn camera(&self, width: u32, height: u32) -> Camera {
        let orientation =
            glam::Quat::from_euler(glam::EulerRot::YXZ, self.cam_yaw, self.cam_pitch, 0.0);
        let target = self.cam_pos + orientation * -glam::Vec3::Z;
        let up = orientation * glam::Vec3::Y;
        Camera::perspective_look_at(
            self.cam_pos,
            target,
            up,
            std::f32::consts::FRAC_PI_4,
            width as f32 / height.max(1) as f32,
            0.01,
            100.0,
        )
    }
}

/// Scene and renderer shared by the window and `--probe`.
fn build_scene(
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    graphics: Option<&mut HelioGraphicsContext<'_>>,
) -> (pulsar_scenedb::SceneDb, Renderer) {
    let config = RendererConfig::new(width, height, format);
    let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
    let graph_scene_db = scene_db_handle(&scene_db);
    let builder = RendererBuilder::new(config, graph_scene_db.clone())
        .with_graph(common::preset());
    let mut renderer = if let Some(graphics) = graphics {
        builder
            .build_with_graphics_context(graphics)
            .expect("Helio renderer")
    } else {
        builder.build(device.clone(), queue.clone(), width, height, format)
    };

    // ── Scene objects ──────────────────────────────────────────────────────
    let mat_wall = spawn_material(
        &mut scene_db.world,
        make_material([0.6, 0.58, 0.55, 1.0], 0.7, 0.0, [0.0, 0.0, 0.0], 0.0),
    );
    let mat_floor = spawn_material(
        &mut scene_db.world,
        make_material([0.3, 0.28, 0.25, 1.0], 0.4, 0.0, [0.0, 0.0, 0.0], 0.0),
    );
    // Lens flare is camera optics, authored on the camera's post-process
    // settings (view 0). It responds to whatever is bright in frame, so
    // the lamp below gets a visible emissive bulb; the light alone would
    // only illuminate surfaces.
    let mut post = helio_pass_postprocess::PostProcessSettings::default();
    post.tonemap_operator = helio::TonemapOperator::Aces;
    post.bloom_enabled = true;
    post.bloom_intensity = 0.15;
    post.lens_flare = helio_pass_postprocess::LensFlareSettings {
        enabled: true,
        quality: 1,
        ghost_count: 6,
        intensity: 0.5,
        ..Default::default()
    };
    common::set_camera_postprocess(&mut scene_db.world, 0, &post);

    let mat_bulb = spawn_material(
        &mut scene_db.world,
        make_material(
            [1.0, 0.85, 0.55, 1.0],
            0.5,
            0.0,
            [1.0, 0.85, 0.55],
            // Radiance x projected area (~0.0144 m^2) matches the 8 cd light inside,
            // so the image path and the analytic light hand over seamlessly.
            555.0,
        ),
    );
    let bulb = spawn_mesh(
        &mut scene_db.world,
        box_mesh([-1.5, 2.0, -1.0], [0.06, 0.06, 0.06]),
    );
    let _ = spawn_object(
        &mut scene_db.world,
        bulb,
        mat_bulb,
        glam::Mat4::IDENTITY,
        0.1,
    );

    // Ground plane
    let floor = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, -0.5, 0.0], 6.0));
    let _ = spawn_object(
        &mut scene_db.world,
        floor,
        mat_floor,
        glam::Mat4::IDENTITY,
        6.0,
    );

    // Back wall — catches the light
    let back_wall = spawn_mesh(
        &mut scene_db.world,
        box_mesh([0.0, 1.5, -5.0], [6.0, 3.0, 0.1]),
    );
    let _ = spawn_object(
        &mut scene_db.world,
        back_wall,
        mat_wall,
        glam::Mat4::IDENTITY,
        6.0,
    );

    // Some pillars / columns to create depth
    for (x, z) in &[(-2.5, -2.0), (2.5, -2.0), (-2.5, 2.0), (2.5, 2.0)] {
        let pillar = spawn_mesh(
            &mut scene_db.world,
            box_mesh([*x, 0.5, *z], [0.3, 1.5, 0.3]),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            pillar,
            mat_wall,
            glam::Mat4::IDENTITY,
            1.0,
        );
    }

    // ── Lighting ───────────────────────────────────────────────────────────
    // Bright directional light shining toward the scene from above-right-front
    let sun_dir = glam::Vec3::new(-0.4, -0.6, 0.7).normalize();
    spawn_light(
        &mut scene_db.world,
        GpuLight {
            position_range: [0.0, 0.0, 0.0, f32::MAX],
            direction_outer: [sun_dir.x, sun_dir.y, sun_dir.z, 0.0],
            color_intensity: [1.0, 0.95, 0.85, 6.0],
            shadow_index: 0,
            light_type: LightType::Directional as u32,
            inner_angle: 0.0,
            _pad: 0,
            god_rays_enabled: 0,
            god_rays_density: 1.0,
            god_rays_weight: 0.6,
            god_rays_decay: 1.0,
            god_rays_exposure: 0.7,
            flare_enabled: 0,
            flare_type: 0,
            flare_intensity: 0.0,
            flare_scale: 0.0,
            flare_tint_r: 0.0,
            flare_tint_g: 0.0,
            flare_tint_b: 0.0,
            ies_profile_index: 0,
            light_function_index: 0,
            ies_angle_scale: 0.0,
            ies_angle_offset: 0.0,
        },
    );

    // A few fill point lights
    // Point light inside the emissive bulb — off-centre so ghosts spread across the axis
    spawn_light(
        &mut scene_db.world,
        GpuLight {
            position_range: [-1.5, 2.0, -1.0, 8.0],
            direction_outer: [0.0, -1.0, 0.0, 0.0],
            color_intensity: [1.0, 0.85, 0.55, 8.0],
            shadow_index: u32::MAX,
            light_type: LightType::Point as u32,
            inner_angle: 0.0,
            _pad: 0,
            god_rays_enabled: 0,
            god_rays_density: 1.0,
            god_rays_weight: 0.6,
            god_rays_decay: 1.0,
            god_rays_exposure: 0.7,
            flare_enabled: 1,
            flare_type: 1,
            flare_intensity: 0.25,
            flare_scale: 1.0,
            flare_tint_r: 1.0,
            flare_tint_g: 0.7,
            flare_tint_b: 0.35,
            ies_profile_index: 0,
            light_function_index: 0,
            ies_angle_scale: 0.0,
            ies_angle_offset: 0.0,
        },
    );

    spawn_light(
        &mut scene_db.world,
        GpuLight {
            position_range: [-3.0, 1.0, -3.0, 5.0],
            direction_outer: [0.0, -1.0, 0.0, 0.0],
            color_intensity: [0.3, 0.4, 0.6, 2.0],
            shadow_index: 0,
            light_type: LightType::Point as u32,
            inner_angle: 0.0,
            _pad: 0,
            ..Default::default()
        },
    );
    spawn_light(
        &mut scene_db.world,
        GpuLight {
            position_range: [3.0, 1.0, 2.0, 4.0],
            direction_outer: [0.0, -1.0, 0.0, 0.0],
            color_intensity: [0.6, 0.3, 0.2, 1.5],
            shadow_index: 0,
            light_type: LightType::Point as u32,
            inner_angle: 0.0,
            _pad: 0,
            ..Default::default()
        },
    );

    renderer.set_feature_live(helio::AmbientLight {
            color: [0.05, 0.05, 0.08],
            intensity: 0.02,
        });
    renderer.set_clear_color([0.01, 0.01, 0.03, 1.0]);
    (scene_db, renderer)
}

/// Headless check: render the demo offscreen through the same graph and
/// report scene-referred output statistics. `cargo run --bin lens_flare_demo -- --probe`
fn probe() {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Lens probe"),
        required_features: required_wgpu_features(adapter.features()),
        required_limits: required_wgpu_limits(adapter.limits()),
        experimental_features: required_experimental_features(adapter.features()),
        ..Default::default()
    }))
    .expect("device");
    device.on_uncaptured_error(Arc::new(|e: wgpu::Error| {
        panic!("[GPU UNCAPTURED ERROR] {e:?}")
    }));
    let (device, queue) = (Arc::new(device), Arc::new(queue));
    let (width, height, format) = (1280u32, 720u32, wgpu::TextureFormat::Rgba8UnormSrgb);
    let (scene_db, mut renderer) =
        build_scene(device.clone(), queue.clone(), width, height, format, None);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Lens probe target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let camera_at = |eye: glam::Vec3, look_at: glam::Vec3| {
        Camera::perspective_look_at(
            eye,
            look_at,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            width as f32 / height as f32,
            0.01,
            100.0,
        )
    };
    let mut render = |camera: &Camera, frames: u32| -> Vec<u8> {
        for _ in 0..frames {
            common::flush_scene_db(&scene_db, &queue);
            renderer.render(camera, &view).expect("probe frame");
        }
        let row = width * 4;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lens probe readback"),
            size: u64::from(row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
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
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let pixels = readback.slice(..).get_mapped_range().unwrap().to_vec();
        pixels
    };
    let stats = |pixels: &[u8]| {
        let mean = pixels
            .chunks_exact(4)
            .map(|p| (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0)
            .sum::<f64>()
            / (width * height) as f64;
        let saturated = pixels
            .chunks_exact(4)
            .filter(|p| p[0] > 250 && p[1] > 250)
            .count();
        (mean, saturated)
    };
    let home = glam::Vec3::new(0.0, 1.2, 3.5);

    // `--sweep`: pan across the lamp in small steps. Pops show up as jumps in
    // the frame mean between neighbouring steps.
    if std::env::args().any(|a| a == "--sweep") {
        let steps = 48;
        for step in 0..=steps {
            let yaw = 0.45 - 0.9 * step as f32 / steps as f32;
            let dir = glam::Quat::from_euler(glam::EulerRot::YXZ, yaw, -0.1, 0.0) * -glam::Vec3::Z;
            let pixels = render(&camera_at(home, home + dir), 3);
            let (mean, saturated) = stats(&pixels);
            println!("[sweep] {step:2} yaw {yaw:+.3} mean {mean:7.3} saturated {saturated:5}");
            if step % 12 == 0 {
                image::save_buffer(
                    format!("lens_sweep_{step:02}.png"),
                    &pixels,
                    width,
                    height,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
        return;
    }

    // `--probe [eye_x,eye_y,eye_z,target_x,target_y,target_z]` picks the view.
    let pose: Vec<f32> = std::env::args()
        .skip_while(|a| a != "--probe")
        .nth(1)
        .map(|s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect())
        .unwrap_or_default();
    let (eye, look_at) = if pose.len() == 6 {
        (
            glam::Vec3::new(pose[0], pose[1], pose[2]),
            glam::Vec3::new(pose[3], pose[4], pose[5]),
        )
    } else {
        (
            home,
            home + glam::Quat::from_rotation_x(-0.1) * -glam::Vec3::Z,
        )
    };
    let start = std::time::Instant::now();
    let pixels = render(&camera_at(eye, look_at), 8);
    println!(
        "[probe] 8 frames in {:.1} ms",
        start.elapsed().as_secs_f64() * 1e3
    );
    let (mean, saturated) = stats(&pixels);
    println!("[probe] mean {mean:.2}/255, saturated pixels {saturated}");
    image::save_buffer(
        "lens_flare_probe.png",
        &pixels,
        width,
        height,
        image::ColorType::Rgba8,
    )
    .unwrap();
    println!("[probe] wrote lens_flare_probe.png");
}
