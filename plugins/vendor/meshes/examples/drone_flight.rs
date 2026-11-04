//! Indoor cathedral example with HLFS ScreenSpace visibility
//!
//! A Gothic interior with ribbed vaults, clustered limestone piers, marble
//! paving, carved oak pews, bronze chandeliers and leaded stained glass.
//! Panes use alpha blending, with explicit thin-sheet RGB shadow transmission
//! in RT mode. Refraction and caustics are not simulated.
//! RT defaults to one daylight sun plus interior lights. Set
//! `HLFS_LEGACY_CATHEDRAL_LIGHTS=1` for the multi-window transmission stress setup.
//!
//! HLFS uses hierarchical light culling, visibility-guided sampling and
//! temporal/spatial filtering with a bounded shadow budget per shading pixel.
//! `--capture <directory>` renders a deterministic offscreen camera path.
//! This variant replaces manual flight with a looping cinematic drone route.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit
//!
//! The sky (the atmosphere and its clouds) and GI left Helio with their
//! plugins (Pulsar `plugins/vendor/sky`, `plugins/vendor/gi`): this scene
//! has no sky and no GI probes; the background colour and the ambient
//! light stand in.

mod architectural_mesh;
mod cathedral_detail;
mod hlfs_capture;
mod v3_demo_common;

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    HelioAction, HelioCommandBridge, Renderer, RendererBuilder, RendererConfig,
};
use helio_pass_perf_overlay::PerfOverlayMode;
use pulsar_scenedb::{Entity, SceneDb, World};
use v3_demo_common::{
    new_scene_db_with_gpu_mirror, point_light, scene_db_handle, spawn_light, update_light,
};

use std::io::{self, BufRead};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

use std::collections::HashSet;

// ── Scene data ────────────────────────────────────────────────────────────────

// Column positions along the nave (Z axis), symmetric at x = ±5.5
const COLUMN_Z: &[f32] = &[-22.0, -14.0, -6.0, 2.0, 10.0, 18.0];

// Stained glass window lights: (x_wall_side, y, z, r, g, b)
// Positive x = right-side windows, negative = left-side; placed just inside the wall
const GLASS_LIGHTS: &[(f32, f32, f32, f32, f32, f32)] = &[
    // Left wall (x ≈ -10.5), windows between columns
    (-10.3, 9.0, -22.0, 0.8, 0.2, 1.0), // violet
    (-10.3, 9.0, -6.0, 0.2, 0.7, 1.0),  // sky blue
    (-10.3, 9.0, 10.0, 0.2, 1.0, 0.4),  // emerald
    (-10.3, 9.0, 18.0, 1.0, 0.7, 0.1),  // gold
    // Right wall (x ≈ +10.5)
    (10.3, 9.0, -22.0, 1.0, 0.2, 0.3), // ruby
    (10.3, 9.0, -6.0, 1.0, 0.5, 0.1),  // amber
    (10.3, 9.0, 10.0, 0.1, 0.8, 0.9),  // teal
    (10.3, 9.0, 18.0, 0.9, 0.1, 0.7),  // magenta
    // Rose window above entrance (back wall, z ≈ +28)
    (0.0, 13.0, 27.0, 1.0, 0.75, 0.3), // warm gold
];

// Chandelier positions (x=0, hanging from y≈19.5, at z intervals)
const CHANDELIER_Z: &[f32] = &[-16.0, 0.0, 16.0];

// Candle cluster positions near the altar (z ≈ -24)
const CANDLES: &[(f32, f32, f32)] = &[
    (-3.0, 1.6, -23.5),
    (-1.5, 1.6, -23.0),
    (0.0, 1.6, -23.5),
    (1.5, 1.6, -23.0),
    (3.0, 1.6, -23.5),
];

fn main() {
    env_logger::init();
    if let Some(directory) = std::env::args()
        .nth(1)
        .filter(|a| a == "--capture")
        .and_then(|_| std::env::args().nth(2))
    {
        hlfs_capture::run(&directory, populate_cathedral);
        return;
    }
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run");
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
    renderer: Arc<Mutex<Renderer>>,
    action_rx: Receiver<HelioAction>,
    last_frame: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    // Debug
    debug_mode: u32,
    perf_overlay_mode: PerfOverlayMode,
    debug_overlay_enabled: bool,

    scene_db: SceneDb,
    acceleration: Option<helio_mesh_host::ray_tracing::SceneDbRayTracing>,

    // Scene state
    chandelier_light_ids: Vec<Entity>,
    candle_light_ids: Vec<Entity>,
    start_time: std::time::Instant,
    drone_flight: bool,
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
                        .with_title("Helio – Indoor Cathedral (HLFS)")
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
                application_name: "Drone Flight".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let surface_format = format;

        let config = RendererConfig::new(size.width, size.height, format).with_feature(
            helio_light_api::ShadowSettings {
                quality: helio_light_api::ShadowQuality::Ultra,
                ..Default::default()
            },
        );
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let (chandelier_light_ids, candle_light_ids) = populate_cathedral(&mut scene_db.world);

        let mut scene_handle = scene_db_handle(&scene_db);
        let stone_store =
            hlfs_capture::architectural_materials::load(&device, &queue, &mut scene_db.world);
        let has_stone = stone_store.is_some();
        if let Some(store) = stone_store {
            scene_handle = scene_handle.with_texture_store(store).unwrap();
        }
        let mut renderer = RendererBuilder::new(config, scene_handle)
            .with_editor_mode(true)
            .with_pass_build_context(Box::new(helio_mesh_host::presets::hlfs))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;
        if has_stone {
            hlfs_capture::architectural_materials::configure_sampler(&mut renderer);
        }
        let acceleration = if std::env::var_os("HLFS_RT").is_some() {
            hlfs_capture::enable_ray_shadows(&mut scene_db.world);
            let config = if std::env::var_os("HLFS_PRESAMPLED").is_some() {
                helio_pass_hlfs::HlfsConfig::ray_traced_presampled()
            } else {
                helio_pass_hlfs::HlfsConfig {
                    mode: helio_pass_hlfs::HlfsMode::RayTraced,
                    ..Default::default()
                }
            };
            renderer
                .find_pass_mut::<helio_pass_hlfs::HlfsPass>()
                .expect("HLFS pass")
                .set_config(&device, config);
            Some(helio_mesh_host::ray_tracing::SceneDbRayTracing::new(
                device.clone(),
                queue.clone(),
            ))
        } else {
            None
        };
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.05, 0.05, 0.08],
            intensity: 1.0,
        });
        renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);

        let renderer = Arc::new(Mutex::new(renderer));
        let (bridge, action_rx) = HelioCommandBridge::new();
        let command_bridge = Arc::new(bridge);

        // REPL thread to drive commands from stdin
        {
            let bridge = command_bridge.clone();
            std::thread::spawn(move || {
                let stdin = io::stdin();
                for line in stdin.lock().lines() {
                    match line {
                        Ok(cmd) if !cmd.trim().is_empty() => match bridge.run(&cmd) {
                            Ok(()) => println!("OK: {}", cmd),
                            Err(e) => println!("ERR: {} -> {}", cmd, e),
                        },
                        _ => {}
                    }
                }
            });
        }

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            renderer,
            action_rx,
            last_frame: std::time::Instant::now(),
            scene_db,
            acceleration,
            // Start at entrance, looking toward the altar
            cam_pos: glam::Vec3::new(0.0, 2.0, 24.0),
            cam_yaw: std::f32::consts::PI,
            cam_pitch: -0.05,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            debug_mode: 0,
            perf_overlay_mode: PerfOverlayMode::Disabled,
            debug_overlay_enabled: false,
            chandelier_light_ids,
            candle_light_ids,
            start_time: std::time::Instant::now(),
            drone_flight: true,
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

            // F1: cycle debug modes (0=normal → 10=shadow heatmap → 11=light-space depth → 0)
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::F1),
                        ..
                    },
                ..
            } => {
                state.debug_mode = match state.debug_mode {
                    0 => 10,
                    10 => 11,
                    _ => 0,
                };
                if let Ok(mut renderer) = state.renderer.lock() {
                    renderer.set_debug_mode(state.debug_mode);
                }
                println!("[debug] shadow debug mode = {}", state.debug_mode);
            }

            // F2: cycle perf overlay modes
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::F2),
                        ..
                    },
                ..
            } => {
                state.perf_overlay_mode = match state.perf_overlay_mode {
                    PerfOverlayMode::Disabled => PerfOverlayMode::PassOverdraw,
                    PerfOverlayMode::PassOverdraw => PerfOverlayMode::ShaderComplexity,
                    PerfOverlayMode::ShaderComplexity => PerfOverlayMode::TileLightCount,
                    PerfOverlayMode::TileLightCount => PerfOverlayMode::PassOutput,
                    PerfOverlayMode::PassOutput => PerfOverlayMode::Disabled,
                };
                if let Ok(mut renderer) = state.renderer.lock() {
                    if let Some(pass) =
                        renderer.find_pass_mut::<helio_pass_perf_overlay::PerfOverlayPass>()
                    {
                        pass.set_mode(state.perf_overlay_mode);
                    }
                }
                println!("[debug] perf overlay mode = {:?}", state.perf_overlay_mode);
            }

            // F3: toggle debug overlay
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::F3),
                        ..
                    },
                ..
            } => {
                state.debug_overlay_enabled = !state.debug_overlay_enabled;
                if let Ok(mut renderer) = state.renderer.lock() {
                    if let Some(pass) =
                        renderer.find_pass_mut::<helio_pass_debug_overlay::DebugOverlayPass>()
                    {
                        pass.set_enabled(state.debug_overlay_enabled);
                    }
                }
                println!("[debug] debug overlay = {:?}", state.debug_overlay_enabled);
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
                if let Ok(mut renderer) = state.renderer.lock() {
                    renderer.set_render_size(s.width, s.height);
                }
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
        const SENS: f32 = 0.002;

        if !self.drone_flight {
            self.cam_yaw += self.mouse_delta.0 * SENS;
            self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * SENS).clamp(-1.4, 1.4);
        }
        self.mouse_delta = (0.0, 0.0);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let movement_forward = glam::Vec3::new(sy * cp, sp, -cy * cp);
        let right = glam::Vec3::new(cy, 0.0, sy);

        if !self.drone_flight && self.keys.contains(&KeyCode::KeyW) {
            self.cam_pos += movement_forward * SPEED * dt;
        }
        if !self.drone_flight && self.keys.contains(&KeyCode::KeyS) {
            self.cam_pos -= movement_forward * SPEED * dt;
        }
        if !self.drone_flight && self.keys.contains(&KeyCode::KeyA) {
            self.cam_pos -= right * SPEED * dt;
        }
        if !self.drone_flight && self.keys.contains(&KeyCode::KeyD) {
            self.cam_pos += right * SPEED * dt;
        }
        if !self.drone_flight && self.keys.contains(&KeyCode::Space) {
            self.cam_pos += glam::Vec3::Y * SPEED * dt;
        }
        if !self.drone_flight && self.keys.contains(&KeyCode::ShiftLeft) {
            self.cam_pos -= glam::Vec3::Y * SPEED * dt;
        }

        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;
        let time = self.start_time.elapsed().as_secs_f32();

        let mut camera_up = glam::Vec3::Y;
        if self.drone_flight {
            let (position, target, bank) = drone_camera(time);
            self.cam_pos = position;
            // Helio's forward vector is `(sin(yaw), 0, -cos(yaw))`; use the
            // inverse of that convention so the drone looks along its route
            // instead of back out through the entrance toward the sky.
            self.cam_yaw = (target.x - position.x).atan2(-(target.z - position.z));
            self.cam_pitch =
                (target.y - position.y).atan2((target.x - position.x).hypot(target.z - position.z));
            camera_up = glam::Vec3::new(bank.sin(), bank.cos(), 0.0);
        }

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let forward = glam::Vec3::new(sy * cp, sp, -cy * cp);

        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            camera_up,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            200.0,
        );

        // Apply commands from REPL / quark to renderer
        let mut renderer = self.renderer.lock().unwrap();
        while let Ok(action) = self.action_rx.try_recv() {
            match action {
                HelioAction::SetDebugMode(mode) => renderer.set_debug_mode(mode),
                HelioAction::SetEditorMode(enabled) => renderer.set_editor_mode(enabled),
                HelioAction::DebugClear => renderer.debug_clear(),
            }
        }

        // Chandeliers flicker slightly
        let flicker = 1.0 + (time * 9.1).sin() * 0.03 + (time * 5.7).cos() * 0.02;
        // Candle flicker — more pronounced
        let cflicker = 1.0 + (time * 14.3).sin() * 0.07 + (time * 8.9).cos() * 0.05;

        let with_shadows = |mut light: helio_light_api::GpuLight| {
            light.set_ray_traced_shadows(self.acceleration.is_some());
            light
        };
        // Update flickering chandelier intensities
        for (i, &id) in self.chandelier_light_ids.iter().enumerate() {
            let z = CHANDELIER_Z[i];
            update_light(
                &mut self.scene_db.world,
                id,
                with_shadows(point_light(
                    [0.0_f32, 15.0, z],
                    [1.0, 0.92, 0.78],
                    160.0 * flicker,
                    22.0,
                )),
            );
        }
        // Update flickering candle intensities
        for (i, &id) in self.candle_light_ids.iter().enumerate() {
            let (x, y, z) = CANDLES[i];
            update_light(
                &mut self.scene_db.world,
                id,
                with_shadows(point_light(
                    [x, y, z],
                    [1.0, 0.6, 0.15],
                    8.0 * cflicker,
                    4.0,
                )),
            );
        }

        // Scene state is persistent — no per-frame setup needed.

        v3_demo_common::flush_scene_db(&self.scene_db, &self.queue);
        if let Some(acceleration) = &mut self.acceleration {
            acceleration
                .prepare(&self.scene_db.world)
                .expect("cathedral RT geometry");
            acceleration.publish(&mut renderer);
        }
        if let Err(e) = renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
    }
}

/// A 52-second looping film-drone route through the nave, columns, vaults and altar.
fn drone_camera(time: f32) -> (glam::Vec3, glam::Vec3, f32) {
    const PERIOD: f32 = 52.0;
    let t = time.rem_euclid(PERIOD);
    let phase = t / PERIOD * std::f32::consts::TAU;

    if t < 18.0 {
        let u = smoothstep(t / 18.0);
        let z = 24.0 - 42.0 * u;
        let x = 1.8 * (u * std::f32::consts::TAU).sin();
        (
            glam::Vec3::new(x, 2.2 + u + 0.25 * phase.sin(), z),
            glam::Vec3::new(0.0, 3.4 + 0.7 * u, z - 5.0),
            0.10 * (u * std::f32::consts::TAU).sin(),
        )
    } else if t < 34.0 {
        let u = smoothstep((t - 18.0) / 16.0);
        let angle = u * std::f32::consts::PI * 1.55;
        (
            glam::Vec3::new(4.0 * angle.cos(), 4.0 + 2.5 * u, -10.0 + 7.0 * angle.sin()),
            glam::Vec3::new(0.0, 6.2 + 0.8 * u, -12.0),
            -0.18 * angle.sin(),
        )
    } else if t < 43.0 {
        let u = smoothstep((t - 34.0) / 9.0);
        let angle = u * std::f32::consts::TAU;
        (
            glam::Vec3::new(3.4 * angle.cos(), 5.2 + 1.8 * u, -24.0 + 3.0 * angle.sin()),
            glam::Vec3::new(0.0, 3.3, -26.0),
            0.12 * angle.cos(),
        )
    } else {
        let u = smoothstep((t - 43.0) / 9.0);
        let z = -24.0 + 48.0 * u;
        (
            glam::Vec3::new(-3.4 * u.sin(), 7.0 - 4.2 * u, z),
            glam::Vec3::new(0.0, 3.8, z + 8.0),
            -0.16 * u.sin(),
        )
    }
}

fn smoothstep(value: f32) -> f32 {
    let x = value.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn populate_cathedral(world: &mut World) -> (Vec<Entity>, Vec<Entity>) {
    cathedral_detail::populate(world);

    // Register lights (chandelier & candle light_ids stored for per-frame flicker updates)
    let mut chandelier_light_ids = Vec::new();
    for &z in CHANDELIER_Z {
        chandelier_light_ids.push(spawn_light(
            world,
            point_light([0.0_f32, 15.0, z], [1.0, 0.92, 0.78], 160.0, 22.0),
        ));
    }
    // A single exterior sun supplies a coherent daylight direction. Keep the
    // older multi-window emitter setup as an explicit transmission stress case.
    if std::env::var_os("HLFS_RT").is_some()
        && std::env::var_os("HLFS_LEGACY_CATHEDRAL_LIGHTS").is_none()
    {
        spawn_light(
            world,
            v3_demo_common::directional_light([0.80, -0.48, -0.30], [1.0, 0.94, 0.84], 4.0),
        );
    } else {
        // Stained glass shafts — static, no need to store ids
        // RT uses white exterior sources: pane materials supply the transmitted tint.
        for &(x, y, z, r, g, b) in GLASS_LIGHTS {
            let light = if std::env::var_os("HLFS_RT").is_some() {
                let position = if x == 0.0 {
                    [0.0, 17.0, 34.0]
                } else {
                    [x.signum() * 16.0, 12.0, z]
                };
                point_light(position, [1.0; 3], 2500.0, 65.0)
            } else {
                point_light([x, y, z], [r, g, b], 35.0, 10.0)
            };
            spawn_light(world, light);
        }
    }
    let mut candle_light_ids = Vec::new();
    for &(x, y, z) in CANDLES {
        candle_light_ids.push(spawn_light(
            world,
            point_light([x, y, z], [1.0, 0.6, 0.15], 8.0, 4.0),
        ));
    }

    (chandelier_light_ids, candle_light_ids)
}
