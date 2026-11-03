//! Volumetric fog + light shafts test scene.
//!
//! A roofed colonnade: two rows of pillars with gaps between them, and a low sun
//! raking through those gaps. The gaps are the point — shafts are only legible
//! where a shadow caster chops the light into slices, so a low angle through a
//! gapped wall shows far more than an open field would.
//!
//! What it exercises:
//!   - Uniform and height-based fog (M)
//!   - Light shafts through the CSM shadow atlas (G) — shafts must line up with
//!     the pillar shadows on the floor; if they don't, the fog pass and deferred
//!     lighting disagree about cascade selection.
//!   - Henyey-Greenstein anisotropy (3/4) — face the sun and the halo should
//!     brighten at positive g, flatten at 0.
//!   - World media: haze fills the hall (a LocalFogVolumeComponent; clear air
//!     outside, so sunlight arrives unattenuated) and a dense
//!     pocket sits mid-hall, z = -6..6 (LocalFogVolumeComponent), visible from
//!     inside and out. Everything is SceneDB components; no camera fog.
//!
//! Controls:
//!   WASD        — move, Space/Shift up/down, mouse drag to look (click to grab)
//!   Q/E         — rotate sun (low angles make the best shafts)
//!   F           — fog on/off
//!   G           — light shafts on/off
//!   M           — uniform / height-based fog
//!   1 / 2       — fog density down / up
//!   3 / 4       — anisotropy (g) down / up
//!   Escape      — release cursor / exit

mod common;

use common::{
    box_mesh, build_default_renderer, build_default_renderer_with_context, directional_light,
    make_material, new_scene_db_with_gpu_mirror, plane_mesh, set_volumetric_quality, spawn_light,
    spawn_local_fog, spawn_material, spawn_mesh, spawn_object, update_light, volumetric_light,
    SHADOW_BASES,
};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererConfig,
};
use helio_pass_postprocess::FogMode;
use helio_pass_volumetric_fog::host_rows::GlobalFogComponent;
use pulsar_scenedb::SceneDb;

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

use std::collections::HashSet;
use std::sync::Arc;

// Hall dimensions. Pillars run along z at +/-HALL_HALF_X, with GAP between them.
const HALL_HALF_X: f32 = 7.0;
const HALL_HALF_Z: f32 = 22.0;
const ROOF_Y: f32 = 9.0;
const PILLAR_SPACING: f32 = 4.0;
const PILLAR_HALF_W: f32 = 0.7;

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
    surface_format: wgpu::TextureFormat,
    renderer: Renderer,
    scene_db: SceneDb,
    last_frame: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    just_pressed: Vec<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    sun_angle: f32,
    sun_light_id: pulsar_scenedb::Entity,
    /// The hall's ambient medium: a GlobalFogComponent row, edited in place.
    haze: pulsar_scenedb::Entity,

    // Medium state, written to the haze component every frame.
    fog_enabled: bool,
    shafts_enabled: bool,
    fog_mode: FogMode,
    fog_density: f32,
    fog_anisotropy: f32,
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
                        .with_title("Helio – Volumetric Fog & Light Shafts")
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
                application_name: "Volumetric Fog Demo".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let surface_format = graphics.surface_config.format;

        // Full internal resolution. RendererConfig defaults to render_scale 0.75,
        // which upscales 960x540 -> 1280x720 and shows as soft, stair-stepped
        // edges — fine in a game demo, but here it would be mistaken for fog
        // quality. Fog accumulates at a quarter of *this*, so the base wants to
        // be honest.
        let config =
            RendererConfig::new(size.width, size.height, surface_format).with_render_scale(1.0);
        let (scene_db, renderer, sun_light_id, haze) =
            build_scene(device.clone(), queue.clone(), config, Some(&mut graphics));

        let surface = graphics.surface;

        print_help();

        self.state = Some(AppState {
            window,
            surface,
            device,
            surface_format,
            renderer,
            scene_db,
            last_frame: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 2.0, 16.0),
            cam_yaw: 0.0,
            cam_pitch: -0.05,
            keys: HashSet::new(),
            just_pressed: Vec::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            sun_angle: 0.35,
            sun_light_id,
            haze,
            fog_enabled: true,
            shafts_enabled: true,
            fog_mode: FogMode::Uniform,
            fog_density: 0.04,
            fog_anisotropy: 0.35,
        });
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
                        state: ks,
                        physical_key: PhysicalKey::Code(key),
                        repeat,
                        ..
                    },
                ..
            } => match ks {
                ElementState::Pressed => {
                    if !repeat {
                        state.just_pressed.push(key);
                    }
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

/// Sun direction as a light "ray direction" (pointing into the scene).
///
/// Orbits in the XY plane with a slight z tilt, so low angles rake across the
/// pillar gaps rather than shining down the hall.
fn sun_light_dir(angle: f32) -> [f32; 3] {
    let to_sun = glam::Vec3::new(angle.cos(), angle.sin(), 0.18).normalize();
    [-to_sun.x, -to_sun.y, -to_sun.z]
}

fn print_help() {
    println!("\n── Volumetric fog demo ──────────────────────────────────");
    println!("  WASD/Space/Shift  move      mouse drag  look (click to grab)");
    println!("  Q/E   rotate sun (low = best shafts)   F  fog on/off");
    println!("  G     light shafts on/off              M  uniform/height fog");
    println!("  1/2   density down/up                  3/4  anisotropy down/up");
    println!("  Dense fog pocket sits mid-hall, z = -6 .. 6 — walk through it.");
    println!("─────────────────────────────────────────────────────────\n");
}

impl AppState {
    fn handle_toggles(&mut self) {
        let pressed = std::mem::take(&mut self.just_pressed);
        let mut dirty = false;

        for key in pressed {
            match key {
                KeyCode::KeyF => {
                    self.fog_enabled = !self.fog_enabled;
                    dirty = true;
                }
                KeyCode::KeyG => {
                    self.shafts_enabled = !self.shafts_enabled;
                    dirty = true;
                }
                KeyCode::KeyM => {
                    self.fog_mode = match self.fog_mode {
                        FogMode::Uniform => FogMode::HeightBased,
                        FogMode::HeightBased => FogMode::Smoke,
                        FogMode::Smoke => FogMode::Uniform,
                    };
                    dirty = true;
                }
                _ => {}
            }
        }

        if dirty {
            println!(
                "fog={} shafts={} mode={:?} density={:.3} g={:.2}",
                self.fog_enabled,
                self.shafts_enabled,
                self.fog_mode,
                self.fog_density,
                self.fog_anisotropy
            );
        }
    }

    fn render(&mut self, dt: f32) {
        const SPEED: f32 = 6.0;
        const LOOK_SENS: f32 = 0.002;
        const SUN_SPEED: f32 = 0.5;

        self.handle_toggles();

        if self.keys.contains(&KeyCode::KeyQ) {
            self.sun_angle -= SUN_SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyE) {
            self.sun_angle += SUN_SPEED * dt;
        }
        if self.keys.contains(&KeyCode::Digit1) {
            self.fog_density = (self.fog_density - 0.05 * dt).max(0.0);
        }
        if self.keys.contains(&KeyCode::Digit2) {
            self.fog_density = (self.fog_density + 0.05 * dt).min(1.0);
        }
        if self.keys.contains(&KeyCode::Digit3) {
            self.fog_anisotropy = (self.fog_anisotropy - 0.5 * dt).max(-0.95);
        }
        if self.keys.contains(&KeyCode::Digit4) {
            self.fog_anisotropy = (self.fog_anisotropy + 0.5 * dt).min(0.95);
        }

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
            500.0,
        );

        // The hall haze. Height mode sits at floor level and thins upward.
        self.scene_db.world.insert(
            self.haze,
            hall_haze(GlobalFogComponent {
                enabled: self.fog_enabled as u32,
                mode: self.fog_mode as u32,
                extinction: self.fog_density,
                albedo: [0.62, 0.70, 0.85],
                anisotropy: self.fog_anisotropy,
                height: 0.0,
                height_falloff: 0.25,
                ..Default::default()
            }),
        );

        let mut sun = volumetric_light(
            directional_light(sun_light_dir(self.sun_angle), [1.0, 0.9, 0.75], 4.0),
            SHADOW_BASES[0],
        );
        // G: the sun still lights surfaces but stops scattering in the medium.
        sun.god_rays_enabled = self.shafts_enabled as u32;
        update_light(&mut self.scene_db.world, self.sun_light_id, sun);

        common::flush_scene_db(&self.scene_db, self.renderer.queue());
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
    }
}

/// The colonnade, its lights and media, shared by the window and `--probe`.
fn build_scene(
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    config: RendererConfig,
    graphics: Option<&mut helio::HelioGraphicsContext<'_>>,
) -> (
    SceneDb,
    Renderer,
    pulsar_scenedb::Entity,
    pulsar_scenedb::Entity,
) {
    let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
    let mut renderer = if let Some(graphics) = graphics {
        build_default_renderer_with_context(&scene_db, graphics, config)
    } else {
        build_default_renderer(&scene_db, device.clone(), queue.clone(), config)
    };

    let stone = spawn_material(
        &mut scene_db.world,
        make_material([0.62, 0.60, 0.58, 1.0], 0.85, 0.0, [0.0, 0.0, 0.0], 0.0),
    );

    // Floor
    let floor = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 40.0));
    let _ = spawn_object(
        &mut scene_db.world,
        floor,
        stone,
        glam::Mat4::IDENTITY,
        40.0,
    );

    // Roof — without it the sun lights everything and there is nothing to
    // slice the light into shafts.
    let roof = spawn_mesh(
        &mut scene_db.world,
        box_mesh([0.0, 0.0, 0.0], [HALL_HALF_X + 1.0, 0.3, HALL_HALF_Z]),
    );
    let _ = spawn_object(
        &mut scene_db.world,
        roof,
        stone,
        glam::Mat4::from_translation(glam::Vec3::new(0.0, ROOF_Y, 0.0)),
        HALL_HALF_Z,
    );

    // Two rows of pillars. The gaps between them are what the sun cuts through.
    let pillar = spawn_mesh(
        &mut scene_db.world,
        box_mesh(
            [0.0, 0.0, 0.0],
            [PILLAR_HALF_W, ROOF_Y * 0.5, PILLAR_HALF_W],
        ),
    );

    let count = (HALL_HALF_Z * 2.0 / PILLAR_SPACING) as i32;
    for i in 0..=count {
        let z = -HALL_HALF_Z + i as f32 * PILLAR_SPACING;
        for side in [-1.0_f32, 1.0] {
            let _ = spawn_object(
                &mut scene_db.world,
                pillar,
                stone,
                glam::Mat4::from_translation(glam::Vec3::new(side * HALL_HALF_X, ROOF_Y * 0.5, z)),
                ROOF_Y * 0.5,
            );
        }
    }

    // Sun. Shafts are the shadowed part of lit medium, so it needs both:
    // participation in the medium and a shadow map (the CSM cascades) for
    // the pillars and roof to carve the light into slices.
    let sun = volumetric_light(
        directional_light(sun_light_dir(1.0), [1.0, 0.9, 0.75], 4.0),
        SHADOW_BASES[0],
    );
    let sun_light_id = spawn_light(&mut scene_db.world, sun);

    // Media are world components in physical units (extinction in m^-1).
    // The hall is filled with haze and the outside is clear air, so sunlight
    // arrives at full strength and scatters only inside, where the pillars and
    // roof carve it into shafts. (A global uniform medium would also attenuate
    // the sunlight along its whole path: 120 m at 0.08/m leaves e^-9.6.)
    let haze = scene_db.world.spawn();
    scene_db.world.insert(
        haze,
        hall_haze(GlobalFogComponent::from(
            helio_pass_volumetric_fog::GlobalFogRow::default(),
        )),
    );

    // A denser pocket mid-hall. It is a local medium, so it is visible
    // from anywhere, not only while the camera is inside it, and overlaps
    // add to the haze. Its 4 m edge fade keeps the boundary soft.
    spawn_local_fog(
        &mut scene_db.world,
        [-HALL_HALF_X, 0.0, -6.0],
        [HALL_HALF_X, ROOF_Y, 6.0],
        GlobalFogComponent {
            extinction: 0.08,
            albedo: [0.75, 0.80, 0.95],
            anisotropy: 0.7,
            ..Default::default()
        },
        4.0,
    );
    // High-quality tier, integrating just past the far end of the hall.
    let fog_settings = set_volumetric_quality(&mut scene_db.world, 1, 120.0);
    // Probe-only overrides for measuring reconstruction loss.
    if let Some(blend) = std::env::var("PROBE_FOG_BLEND")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        let quality = std::env::var("PROBE_FOG_QUALITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        scene_db.world.insert(
            fog_settings,
            helio_pass_volumetric_fog::host_rows::VolumetricFogSettingsComponent {
                quality,
                max_distance: 120.0,
                light_max_distance: 120.0,
                temporal_blend: blend,
                ..Default::default()
            },
        );
    }
    (scene_db, renderer, sun_light_id, haze)
}

/// Headless: `cargo run --bin volumetric_fog_demo -- --probe` writes
/// volumetric_fog_probe.png from the default viewpoint and sun angle.
fn probe() {
    let (device, queue) = common::headless_gpu();
    let (width, height) = (1280, 720);
    let mut config = RendererConfig::new(width, height, wgpu::TextureFormat::Rgba8UnormSrgb)
        .with_render_scale(1.0);
    if let Some(mode) = std::env::var("PROBE_DEBUG_MODE")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        config.debug_mode = mode;
    }
    let (mut scene_db, mut renderer, sun_light_id, haze) =
        build_scene(device.clone(), queue.clone(), config, None);
    let sun_intensity = std::env::var("PROBE_SUN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4.0);
    let sun = volumetric_light(
        directional_light(sun_light_dir(0.35), [1.0, 0.9, 0.75], sun_intensity),
        SHADOW_BASES[0],
    );
    let mut sun = sun;
    if std::env::var_os("PROBE_NOSHADOW").is_some() {
        sun.shadow_index = u32::MAX;
    }
    update_light(&mut scene_db.world, sun_light_id, sun);
    scene_db.world.insert(
        haze,
        hall_haze(GlobalFogComponent {
            extinction: std::env::var("PROBE_EXT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.04),
            emission: if std::env::var_os("PROBE_DENSE").is_some() {
                [0.3, 0.1, 0.1]
            } else {
                [0.0; 3]
            },
            albedo: [0.62, 0.70, 0.85],
            anisotropy: 0.35,
            height_falloff: 0.25,
            ..Default::default()
        }),
    );
    // `PROBE_TOWARD_SUN`: look across the hall into the sun through the pillar
    // gaps, where forward scattering makes shafts brightest.
    let (eye, look) = if std::env::var_os("PROBE_TOWARD_SUN").is_some() {
        (
            glam::Vec3::new(-4.0, 2.0, 4.0),
            glam::Vec3::new(1.0, 0.25, -0.35),
        )
    } else {
        (
            glam::Vec3::new(0.0, 2.0, 16.0),
            glam::Vec3::new(0.0, -0.05, -1.0),
        )
    };
    let camera = Camera::perspective_look_at(
        eye,
        eye + look,
        glam::Vec3::Y,
        std::f32::consts::FRAC_PI_4,
        width as f32 / height as f32,
        0.1,
        500.0,
    );
    let mean = common::capture_png(
        &scene_db,
        &mut renderer,
        &device,
        &queue,
        &camera,
        (width, height),
        30,
        "volumetric_fog_probe.png",
    );
    println!("[probe] mean {mean:.2}/255 -> volumetric_fog_probe.png");
    if std::env::var_os("PROBE_SHADOW_DEBUG").is_some() {
        if let Some(batch) = renderer.find_pass::<helio_pass_object_batch::ObjectBatchPass>() {
            println!(
                "[probe] batch counts (draws, shadow static, shadow movable) = {:?}",
                batch.counts()
            );
        }
        if let Some(matrix_pass) =
            renderer.find_pass::<helio_pass_shadow_matrix::ShadowMatrixPass>()
        {
            let src = matrix_pass.matrices();
            let dump = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: src.size(),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut e = device.create_command_encoder(&Default::default());
            e.copy_buffer_to_buffer(src, 0, &dump, 0, src.size());
            queue.submit([e.finish()]);
            dump.slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let m: Vec<f32> =
                bytemuck::cast_slice(&dump.slice(..).get_mapped_range().unwrap()).to_vec();
            println!("[probe] matrix 0 = {:?}", &m[..16]);
        }
    }
}

/// The colonnade's haze: a local medium just larger than the hall, with a
/// soft edge, so light outside travels through clear air.
fn hall_haze(
    medium: GlobalFogComponent,
) -> helio_pass_volumetric_fog::host_rows::LocalFogVolumeComponent {
    let mut volume = helio_pass_volumetric_fog::host_rows::LocalFogVolumeComponent::from(
        helio_pass_volumetric_fog::LocalFogVolumeRow::new(
            [-HALL_HALF_X - 2.0, 0.0, -HALL_HALF_Z - 2.0],
            [HALL_HALF_X + 2.0, ROOF_Y + 1.0, HALL_HALF_Z + 2.0],
            medium.row(),
        ),
    );
    volume.edge_fade = 1.5;
    volume
}
