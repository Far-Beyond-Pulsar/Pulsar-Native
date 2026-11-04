//! Helio lens showcase — deterministic offscreen captures of camera optics.
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

use common::{
    box_mesh, make_material, new_scene_db_with_gpu_mirror, plane_mesh, scene_db_handle,
    spawn_light, spawn_material, spawn_mesh, spawn_object, sphere_mesh,
};
use common::{LensComponent, LensElementComponent};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, GpuLight,
    HelioGraphicsConfig, HelioGraphicsContext, LightType, Renderer, RendererBuilder,
    RendererConfig,
};
use helio_pass_lens::{LensRegion, ProjectionModel};

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

use std::collections::HashSet;
use std::sync::Arc;

const LENS_USER_HOOK: &str = r#"
fn user_ray_map(uv: vec2<f32>, params: array<f32, 16>, t: f32) -> vec2<f32> {
    let p = uv * 2.0 - vec2<f32>(1.0);
    return uv + vec2<f32>(p.y, -p.x) * params[0] * dot(p, p);
}
fn user_transmission(uv: vec2<f32>, params: array<f32, 16>) -> vec3<f32> {
    return mix(vec3<f32>(1.0), vec3<f32>(uv.x, uv.y, 1.0), params[1]);
}
fn user_focus(uv: vec2<f32>, params: array<f32, 16>, s: f32) -> f32 {
    let p = uv * 2.0 - vec2<f32>(1.0);
    return s + params[2] * p.x;
}"#;

fn main() {
    env_logger::init();
    if std::env::args().any(|a| a == "--all") {
        return capture_all();
    }
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
                application_name: "Lens Showcase".into(),
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
    let mut config = RendererConfig::new(width, height, format);
    config
        .features
        .insert(helio_pass_lens::LensUser(LENS_USER_HOOK));
    let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
    let graph_scene_db = scene_db_handle(&scene_db);
    let builder = RendererBuilder::new(config, graph_scene_db.clone()).with_graph(common::preset());
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
        make_material([0.32, 0.42, 0.58, 1.0], 0.75, 0.0, [0.0, 0.0, 0.0], 0.0),
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
    // Keep the unrelated lens-flare post effect off so these captures show
    // the camera-lens element being demonstrated, rather than stacking two
    // different optical effects into every image.
    post.lens_flare.enabled = false;
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
            60.0,
        ),
    );
    let bulb = spawn_mesh(&mut scene_db.world, sphere_mesh([-1.5, 2.0, -1.0], 0.075));
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

    // A simple color-and-edge target gives distortion, lateral color, and
    // localized focus a recognizable reference beyond the point lights.
    let chart_colors = [
        [0.92, 0.12, 0.08],
        [0.96, 0.52, 0.08],
        [0.92, 0.84, 0.16],
        [0.12, 0.72, 0.28],
        [0.08, 0.48, 0.92],
        [0.52, 0.16, 0.82],
        [0.88, 0.82, 0.72],
        [0.16, 0.19, 0.24],
    ];
    for row in 0..3 {
        for col in 0..6 {
            let color = chart_colors[(row * 2 + col) % chart_colors.len()];
            let material = spawn_material(
                &mut scene_db.world,
                make_material([color[0], color[1], color[2], 1.0], 0.45, 0.0, color, 1.15),
            );
            let x = (col as f32 - 2.5) * 0.46;
            let y = 0.58 + row as f32 * 0.48;
            let mesh = spawn_mesh(
                &mut scene_db.world,
                box_mesh([x, y, -4.72], [0.21, 0.21, 0.035]),
            );
            let _ = spawn_object(
                &mut scene_db.world,
                mesh,
                material,
                glam::Mat4::IDENTITY,
                0.32,
            );
        }
    }
    let hero_material = spawn_material(
        &mut scene_db.world,
        make_material([0.92, 0.24, 0.08, 1.0], 0.28, 0.1, [0.35, 0.055, 0.0], 0.6),
    );
    let hero_mesh = spawn_mesh(&mut scene_db.world, sphere_mesh([0.0, 1.12, -2.45], 0.42));
    let _ = spawn_object(
        &mut scene_db.world,
        hero_mesh,
        hero_material,
        glam::Mat4::IDENTITY,
        0.45,
    );

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

    // Bright foreground and background points make focus falloff, pupil
    // shape, and highlight scattering easy to compare in the same frame.
    let colors = [
        [1.0, 0.18, 0.08],
        [0.12, 0.45, 1.0],
        [1.0, 0.62, 0.12],
        [0.28, 1.0, 0.42],
        [0.8, 0.24, 1.0],
        [1.0, 0.9, 0.55],
    ];
    let depths = [0.1, -1.35, -3.1, -4.1];
    for (i, z) in depths.into_iter().enumerate() {
        for (j, x) in [-2.3_f32, -1.35, -0.4, 0.65, 1.7, 2.45]
            .into_iter()
            .enumerate()
        {
            let color = colors[(i + j) % colors.len()];
            let material = spawn_material(
                &mut scene_db.world,
                make_material([color[0], color[1], color[2], 1.0], 0.4, 0.0, color, 15.0),
            );
            let mesh = spawn_mesh(
                &mut scene_db.world,
                sphere_mesh([x, 1.3 + (j % 2) as f32 * 0.55, z], 0.085),
            );
            let _ = spawn_object(
                &mut scene_db.world,
                mesh,
                material,
                glam::Mat4::IDENTITY,
                0.12,
            );
        }
    }

    // Lens elements are addressed as a contiguous block in the GPU row
    // buffer. Reserve eight adjacent entities so each capture can replace
    // the authored stack without rebuilding the scene or render graph.
    let element_entities: Vec<_> = (0..8).map(|_| scene_db.world.spawn()).collect();
    for entity in element_entities {
        let neutral: LensElementComponent = bytemuck::Zeroable::zeroed();
        scene_db.world.insert(entity, neutral);
    }
    common::set_camera_lens(
        &mut scene_db.world,
        LensComponent::from(helio_pass_lens::LensRow::new(0)),
    );

    renderer.set_feature_live(helio::AmbientLight {
        color: [0.10, 0.12, 0.18],
        intensity: 0.04,
    });
    renderer.set_clear_color([0.01, 0.01, 0.03, 1.0]);
    (scene_db, renderer)
}

/// Render a curated set of lens variants through the production graph.
/// Usage: `cargo run -p examples --bin lens_showcase -- --all --output-dir PATH`.
fn capture_all() {
    let args: Vec<String> = std::env::args().collect();
    let output_dir = args
        .windows(2)
        .find(|w| w[0] == "--output-dir")
        .map(|w| std::path::PathBuf::from(&w[1]))
        .unwrap_or_else(|| std::path::PathBuf::from("lens-showcase"));
    std::fs::create_dir_all(&output_dir).expect("create output directory");

    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Helio lens showcase"),
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
    let (mut scene_db, mut renderer) =
        build_scene(device.clone(), queue.clone(), width, height, format, None);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Lens showcase target"),
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
    let camera = Camera::perspective_look_at(
        glam::Vec3::new(0.0, 1.2, 3.5),
        glam::Vec3::new(0.0, 1.1, -2.5),
        glam::Vec3::Y,
        std::f32::consts::FRAC_PI_4,
        width as f32 / height as f32,
        0.01,
        100.0,
    );
    let cases = [
        "baseline",
        "no-lens",
        "wide-open",
        "shallow-depth",
        "stopped-down",
        "anamorphic",
        "swirly-bokeh",
        "longitudinal-ca",
        "lateral-ca",
        "barrel",
        "fisheye",
        "projection-equisolid",
        "projection-orthographic",
        "projection-stereographic",
        "projection-kannala-brandt",
        "split-diopter",
        "split-seam",
        "pro-mist",
        "black-pro-mist",
        "halation",
        "halation-tint",
        "star-filter",
        "coma-astigmatism",
        "iris-rounded-blades",
        "iris-texture-star",
        "iris-texture-heart",
        "iris-texture-triangle",
        "anamorphic-133",
        "anamorphic-200",
        "cats-eye",
        "spherical-rim",
        "spherical-soft",
        "onion-rings",
        "apodization",
        "central-obstruction",
        "split-horizontal",
        "split-vertical",
        "split-diagonal",
        "split-multi-ellipse",
        "split-radial-band",
        "split-polygon",
        "sweet-spot",
        "tilt-focus",
        "field-curvature",
        "autofocus",
        "division-distortion",
        "pincushion",
        "anamorphic-distortion",
        "lens-shift",
        "keystone",
        "panini",
        "cylindrical",
        "focus-breathing",
        "prism-line",
        "prism-ring",
        "anamorphic-squeeze",
        "flat-port",
        "dome-port",
        "natural-vignette",
        "optical-vignette",
        "circular-hood",
        "matte-box",
        "nd-filter",
        "graduated-nd",
        "color-filter",
        "t-stop",
        "spherical-glow",
        "corner-softness",
        "diffusion",
        "diffusion-tail",
        "glint",
        "glimmerglass",
        "fog-diffusion",
        "water-drops",
        "water-drops-transformed",
        "smear",
        "dirt",
        "prescription-thin-lens",
        "prescription-cooke",
        "prescription-double-gauss",
        "user-hook",
    ];
    for name in cases {
        apply_case(&mut scene_db, name);
        // Allow render-history resources to settle consistently before readback.
        for _ in 0..4 {
            common::flush_scene_db(&scene_db, &queue);
            renderer.render(&camera, &view).expect("showcase frame");
        }
        let row = width * 4;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lens showcase readback"),
            size: u64::from(row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
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
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.expect("map showcase buffer"));
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll showcase readback");
        let pixels = buffer
            .slice(..)
            .get_mapped_range()
            .expect("mapped showcase pixels")
            .to_vec();
        let path = output_dir.join(format!("{name}.png"));
        image::save_buffer(&path, &pixels, width, height, image::ColorType::Rgba8)
            .expect("save showcase image");
        println!(
            "[lens_showcase] wrote {} ({}x{})",
            path.display(),
            width,
            height
        );
        drop(pixels);
        buffer.unmap();
    }
}

fn apply_case(scene_db: &mut pulsar_scenedb::SceneDb, name: &str) {
    use LensRegion::FullFrame as F;
    let mut lens = LensComponent::from(helio_pass_lens::LensRow::new(0));
    lens.f_number = 2.8;
    lens.focus_distance_m = 6.0;
    lens.quality = 1;
    let elements: Vec<LensElementComponent> = match name {
        "baseline" => vec![],
        "no-lens" => {
            lens.enabled = 0;
            vec![]
        }
        "wide-open" => {
            lens.f_number = 1.2;
            lens.aperture_blades = 0;
            vec![]
        }
        "shallow-depth" => {
            lens.f_number = 1.2;
            lens.focus_distance_m = 3.4;
            vec![]
        }
        "stopped-down" => {
            lens.f_number = 5.6;
            lens.aperture_blades = 6;
            vec![]
        }
        "anamorphic" => {
            lens.f_number = 2.0;
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::pupil(F).with_anamorphic(1.8),
            )]
        }
        "swirly-bokeh" => {
            lens.f_number = 1.4;
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::pupil(F)
                    .with_swirl(1.4)
                    .with_cats_eye(0.8, 1.0),
            )]
        }
        "longitudinal-ca" => {
            lens.f_number = 1.8;
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::longitudinal_ca(-0.65, 0.0, -0.65, F),
            )]
        }
        "lateral-ca" => {
            lens.quality = 2;
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::lateral_ca(0.08, 0.0, F),
            )]
        }
        "barrel" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::brown_conrady(
                [-0.35, 0.04, 0.0, 0.0, 0.0, 0.0],
                [0.0; 2],
                F,
            ),
        )],
        "fisheye" => {
            lens.focal_length_mm = 16.0;
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::projection(ProjectionModel::Equidistant, F),
            )]
        }
        "projection-equisolid" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::projection(ProjectionModel::Equisolid, F),
        )],
        "projection-orthographic" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::projection(ProjectionModel::Orthographic, F),
        )],
        "projection-stereographic" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::projection(ProjectionModel::Stereographic, F),
        )],
        "projection-kannala-brandt" => {
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::projection(ProjectionModel::KannalaBrandt, F)
                    .with_kannala_brandt([-0.02, 0.003, 0.0, 0.0]),
            )]
        }
        "split-diopter" => {
            lens.f_number = 2.0;
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::diopter(
                    4.8,
                    LensRegion::HalfPlane {
                        angle: 0.0,
                        offset: 0.0,
                    },
                )
                .with_feather(0.025),
            )]
        }
        "pro-mist" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::pro_mist(0.45, F),
        )],
        "black-pro-mist" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::black_pro_mist(0.45, F),
        )],
        "halation" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::halation(0.7, 0.018, F),
        )],
        "halation-tint" => {
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::halation(0.7, 0.018, F)
                    .with_halation_tint([1.0, 0.25, 0.08]),
            )]
        }
        "star-filter" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::star_filter(6, 0.2, 0.12, 0.5, F),
        )],
        "coma-astigmatism" => vec![
            LensElementComponent::from(helio_pass_lens::LensElementRow::coma(0.5, 0.09, F)),
            LensElementComponent::from(helio_pass_lens::LensElementRow::astigmatism(
                0.035, 0.35, F,
            )),
        ],
        "iris-rounded-blades" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::pupil(F)
                .with_iris(6, 0.0, 0.25)
                .with_f_number_rounding(4.0),
        )],
        "iris-texture-star" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::pupil(F).with_aperture_texture(0, 1.0),
        )],
        "iris-texture-heart" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::pupil(F).with_aperture_texture(1, 1.0),
        )],
        "iris-texture-triangle" => {
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::pupil(F).with_aperture_texture(2, 1.0),
            )]
        }
        "anamorphic-133" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::pupil(F).with_anamorphic(1.33),
        )],
        "anamorphic-200" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::pupil(F).with_anamorphic(2.0),
        )],
        "cats-eye" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::pupil(F).with_cats_eye(1.4, 1.0),
        )],
        "spherical-rim" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::pupil(F).with_spherical_aberration(0.8, 0.0),
        )],
        "spherical-soft" => {
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::pupil(F).with_spherical_aberration(-0.8, 1.0),
            )]
        }
        "onion-rings" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::pupil(F).with_onion_rings(0.9, 8.0),
        )],
        "apodization" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::pupil(F).with_apodization(0.38),
        )],
        "central-obstruction" => {
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::pupil(F).with_central_obstruction(0.55),
            )]
        }
        "split-horizontal" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::diopter(
                4.8,
                LensRegion::HalfPlane {
                    angle: 0.0,
                    offset: 0.0,
                },
            )
            .with_feather(0.02),
        )],
        "split-vertical" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::diopter(
                4.8,
                LensRegion::HalfPlane {
                    angle: std::f32::consts::FRAC_PI_2,
                    offset: 0.0,
                },
            )
            .with_feather(0.02),
        )],
        "split-diagonal" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::diopter(
                4.8,
                LensRegion::HalfPlane {
                    angle: std::f32::consts::FRAC_PI_4,
                    offset: 0.0,
                },
            )
            .with_feather(0.02),
        )],
        "split-multi-ellipse" => vec![
            LensElementComponent::from(helio_pass_lens::LensElementRow::diopter(
                4.8,
                LensRegion::Ellipse {
                    center: [-0.22, 0.0],
                    radii: [0.22, 0.42],
                    rotation: -0.2,
                },
            )),
            LensElementComponent::from(helio_pass_lens::LensElementRow::diopter(
                8.0,
                LensRegion::Ellipse {
                    center: [0.22, 0.0],
                    radii: [0.22, 0.42],
                    rotation: 0.2,
                },
            )),
        ],
        "split-radial-band" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::diopter(
                4.8,
                LensRegion::RadialBand {
                    r0: 0.18,
                    r1: 0.52,
                    center: [0.5, 0.5],
                },
            ),
        )],
        "split-polygon" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::diopter(
                4.8,
                LensRegion::Polygon(&[
                    [-0.55, -0.4],
                    [0.0, -0.48],
                    [0.25, 0.0],
                    [0.0, 0.48],
                    [-0.55, 0.4],
                ]),
            ),
        )],
        "split-seam" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::diopter(
                4.8,
                LensRegion::HalfPlane {
                    angle: std::f32::consts::FRAC_PI_4,
                    offset: 0.08,
                },
            )
            .with_feather(0.06)
            .with_seam_darkening(0.75)
            .with_mount_distance(0.04),
        )],
        "sweet-spot" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::sweet_spot(
                18.0,
                LensRegion::Ellipse {
                    center: [0.0, 0.0],
                    radii: [0.27, 0.34],
                    rotation: 0.0,
                },
            )
            .with_feather(0.08),
        )],
        "tilt-focus" => {
            lens.tilt_angle = 0.16;
            lens.tilt_axis = 0.25;
            vec![]
        }
        "field-curvature" => {
            lens.field_curvature = 0.65;
            vec![]
        }
        "autofocus" => {
            lens.focus_mode = helio_pass_lens::FOCUS_AUTO_POINT;
            lens.af_u = 0.28;
            lens.af_v = 0.48;
            lens.af_speed = 0.0;
            vec![]
        }
        "division-distortion" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::division(-0.35, 0.05, F),
        )],
        "pincushion" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::brown_conrady(
                [0.28, 0.02, 0.0, 0.0, 0.0, 0.0],
                [0.0; 2],
                F,
            ),
        )],
        "anamorphic-distortion" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::anamorphic_distortion([-0.3, 0.0], [0.25, 0.0], F),
        )],
        "anamorphic-squeeze" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::anamorphic_squeeze(1.8, F),
        )],
        "lens-shift" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::shift(0.18, -0.08, F),
        )],
        "keystone" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::keystone(0.35, -0.2, F),
        )],
        "panini" => {
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::projection(ProjectionModel::Panini, F)
                    .with_panini(0.8, 0.7),
            )]
        }
        "cylindrical" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::projection(ProjectionModel::Cylindrical, F),
        )],
        "focus-breathing" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::focus_breathing(0.35, F),
        )],
        "prism-line" => {
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::prism(4, 0.055, 0.25, F).with_prism_falloff(0.8),
            )]
        }
        "prism-ring" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::prism(5, 0.055, 0.0, F)
                .with_prism_ring()
                .with_prism_spin(0.2)
                .with_prism_falloff(0.75),
        )],
        "flat-port" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::flat_port(1.333, 0.02, F),
        )],
        "dome-port" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::dome_port(0.18, 0.08, F),
        )],
        "natural-vignette" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::natural_vignette(1.0, F),
        )],
        "optical-vignette" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::optical_vignette(1.1, 0.8, 2.8, F),
        )],
        "circular-hood" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::circular_hood(0.62, 0.12, F),
        )],
        "matte-box" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::matte_box([0.46, 0.42], 0.12, 0.08, F),
        )],
        "nd-filter" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::nd_filter(1.5, F),
        )],
        "graduated-nd" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::graduated_nd(1.8, 1.15, 0.0, 0.2, [1.0, 0.9, 0.75], F),
        )],
        "color-filter" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::color_filter([0.9, 0.55, 0.35], 0.7, F),
        )],
        "t-stop" => {
            lens.f_number = 2.8;
            lens.t_stop = 5.6;
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::t_stop_loss(1.0, F),
            )]
        }
        "spherical-glow" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::spherical_glow(0.7, 2.8, 2.0, 0.025, F),
        )],
        "corner-softness" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::corner_softness(0.022, 0.45, F),
        )],
        "diffusion" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::diffusion(0.45, 0.018, F),
        )],
        "diffusion-tail" => {
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::diffusion(0.45, 0.018, F).with_diffusion_tail(0.9),
            )]
        }
        "glint" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::pro_mist(0.4, F).with_glint(0.8),
        )],
        "glimmerglass" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::glimmerglass(0.45, F).with_glint(0.8),
        )],
        "fog-diffusion" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::fog(0.45, F),
        )],
        "water-drops" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::water_drops(
                0.025,
                0,
                0.008,
                LensRegion::MaskChannel {
                    layer: 0,
                    channel: 3,
                    invert: false,
                },
            ),
        )],
        "water-drops-transformed" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::water_drops(
                0.025,
                0,
                0.008,
                LensRegion::MaskChannel {
                    layer: 0,
                    channel: 3,
                    invert: false,
                },
            )
            .with_mask_transform(1.7, [0.12, -0.08], true),
        )],
        "smear" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::smear(0.55, 0.06, 0.35, F),
        )],
        "dirt" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::dirt(
                0.55,
                0.03,
                0.12,
                LensRegion::MaskChannel {
                    layer: 0,
                    channel: 3,
                    invert: false,
                },
            ),
        )],
        "prescription-thin-lens" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::prescription(0, F),
        )],
        "prescription-cooke" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::prescription(1, F),
        )],
        "prescription-double-gauss" => vec![LensElementComponent::from(
            helio_pass_lens::LensElementRow::prescription(2, F),
        )],
        "user-hook" => {
            let mut params = [0.0; 16];
            params[0] = 0.035;
            params[1] = 0.65;
            params[2] = 0.25;
            vec![LensElementComponent::from(
                helio_pass_lens::LensElementRow::user(
                    helio_pass_lens::user::MAP
                        | helio_pass_lens::user::TRANSMISSION
                        | helio_pass_lens::user::FOCUS
                        | helio_pass_lens::user::SPECTRAL,
                    params,
                    F,
                ),
            )]
        }
        _ => panic!("unknown lens case {name}"),
    };
    if matches!(
        name,
        "anamorphic"
            | "swirly-bokeh"
            | "iris-rounded-blades"
            | "iris-texture-star"
            | "iris-texture-heart"
            | "iris-texture-triangle"
            | "anamorphic-133"
            | "anamorphic-200"
            | "cats-eye"
            | "spherical-rim"
            | "spherical-soft"
            | "onion-rings"
            | "apodization"
            | "central-obstruction"
    ) {
        lens.f_number = 1.2;
    }
    lens.element_count = elements.len() as u32;
    let mut slots: Vec<_> = scene_db
        .world
        .query::<&LensElementComponent>()
        .map(|(entity, _)| entity)
        .collect();
    slots.sort_by_key(|entity| entity.index());
    assert!(slots.len() >= 8, "lens showcase element slots missing");
    lens.element_first = slots[0].index();
    for (slot, element) in slots.into_iter().zip(elements.into_iter()) {
        if lens.element_count > 0 && slot.index() < lens.element_first + lens.element_count {
            scene_db.world.insert(slot, element);
        }
    }
    if name.starts_with("split-") {
        // A longer, faster lens makes the two split-focus distances visible
        // on this small showcase render while preserving the same scene.
        lens.focal_length_mm = 85.0;
        lens.f_number = 1.4;
        lens.focus_distance_m = 8.0;
    }
    if name.starts_with("projection-") || name == "fisheye" {
        lens.focal_length_mm = 16.0;
    }
    common::set_camera_lens(&mut scene_db.world, lens);
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
