//! Oil rig at night – water volume demonstration
//!
//! A platform rising from a dark ocean with underwater accent lights beneath the rig.
//! Includes nighttime sky and glowing light arrays on the rig legs to highlight caustics
//! and reflection behavior in the water scene actor.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit

mod common;

use common::{
    box_mesh, make_material, new_scene_db_with_gpu_mirror, point_light, scene_db_handle,
    spawn_light, spawn_material, spawn_mesh, spawn_object, spawn_sky, spawn_water_volume,
    WaterVolumeDescriptor,
};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use helio_pass_water_sim::WaterSimPass;
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

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    scene_db: SceneDb,
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
                        .with_title("Helio – Oil Rig Night")
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
                application_name: "Oil Rig Night".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let surface_format = format;

        let config = RendererConfig::new(size.width, size.height, format).with_feature(
            helio_pass_shadow_matrix::ShadowSettings {
                quality: helio::ShadowQuality::High,
                ..Default::default()
            },
        );
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let mut renderer = RendererBuilder::new(config, scene_db_handle(&scene_db))
            .with_graph(common::preset())
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;

        spawn_sky(&mut scene_db.world, [0.02, 0.03, 0.08]);

        // Ocean water volume — mid-ocean night, Beaufort 4 (~25 km/h). Static
        // appearance goes in the SceneDB descriptor; the heightfield sim's own
        // wind/wave dynamics are pass-owned runtime state, set below via
        // `WaterSimPass` (see `WaterVolumeDescriptor`'s own doc for why).
        let ocean = WaterVolumeDescriptor {
            bounds_min: [-120.0, -20.0, -120.0],
            bounds_max: [120.0, 40.0, 120.0],
            surface_height: 0.0,

            // Legacy Gerstner (not used by heightfield sim, kept for compat)
            wave_amplitude: 0.8,
            wave_frequency: 0.28,
            wave_speed: 1.0,
            wave_direction: [0.97, 0.14],
            wave_steepness: 0.35,

            // Deep pelagic water: red absorbed ~3m, green ~8m, blue ~30m
            water_color: [0.005, 0.025, 0.09],
            extinction: [0.38, 0.13, 0.03],

            foam_threshold: 0.72,
            foam_amount: 0.55,

            // Night mirror reflection, seawater Fresnel
            reflection_strength: 0.92,
            refraction_strength: 0.30,
            fresnel_power: 5.5,

            caustics_enabled: true,
            caustics_intensity: 1.1,
            caustics_scale: 4.5,
            caustics_speed: 0.5,

            fog_density: 0.016,
            god_rays_intensity: 0.15,

            ..Default::default()
        };
        spawn_water_volume(&mut scene_db.world, ocean);

        // NNE wind, Beaufort 4.
        // wave_scale=0.45 => primary swell wavelength ~28m in the 240m domain.
        // wave_speed=1.0 => phase velocity ~15 m/s for the primary swell.
        // wind_strength=1.5 drives ~0.4m significant wave height.
        // wave_spring=0.04 => sqrt(0.04) * 112.5 m/s = ~22 m/s -- realistic ocean swell.
        // wave_damping=0.990 => moderate decay: waves persist ~2 s before damping out.
        if let Some(sim) = renderer.find_pass_mut::<WaterSimPass>() {
            sim.set_wind([0.97, 0.14], 1.5);
            sim.set_wave_scale(0.45);
            sim.set_wave_speed(1.0);
            sim.set_sim_dynamics(0.04, 0.990);
        }

        let mat_platform = spawn_material(
            &mut scene_db.world,
            make_material([0.2, 0.2, 0.2, 1.0], 0.35, 1.0, [0.1, 0.1, 0.1], 0.3),
        );

        let mat_leg = spawn_material(
            &mut scene_db.world,
            make_material([0.25, 0.25, 0.25, 1.0], 0.6, 1.0, [0.02, 0.02, 0.02], 0.15),
        );

        // Platform base
        let platform_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [14.0, 0.8, 20.0]),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            platform_mesh,
            mat_platform,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 8.4, 0.0)),
            23.0,
        );

        // Rig legs
        let leg_positions = [
            (-12.0, 4.0, -16.0),
            (12.0, 4.0, -16.0),
            (-12.0, 4.0, 16.0),
            (12.0, 4.0, 16.0),
        ];
        for (x, y, z) in leg_positions {
            let leg_mesh = spawn_mesh(
                &mut scene_db.world,
                box_mesh([0.0, 0.0, 0.0], [0.9, 4.2, 0.9]),
            );
            let _ = spawn_object(
                &mut scene_db.world,
                leg_mesh,
                mat_leg,
                glam::Mat4::from_translation(glam::Vec3::new(x, y, z)),
                4.5,
            );
        }

        // Central tower
        let tower_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [2.5, 5.5, 2.5]),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            tower_mesh,
            mat_platform,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 12.0, 0.0)),
            6.0,
        );

        // Under-platform accent lights (lots of bright colored lights beneath rig)
        let mut _light_ids: Vec<Entity> = Vec::new();
        let grid_x = (-10..=10).step_by(5).collect::<Vec<i32>>();
        let grid_z = (-14..=14).step_by(5).collect::<Vec<i32>>();
        for gx in grid_x.iter() {
            for gz in grid_z.iter() {
                let pos = [*gx as f32, 1.1, *gz as f32];
                let hue = ((*gx + *gz + 20) as f32 % 6.0) / 6.0;
                let color = match hue {
                    h if h < 0.17 => [1.0, 0.6, 0.3],
                    h if h < 0.33 => [0.3, 0.6, 1.0],
                    h if h < 0.5 => [0.5, 1.0, 0.35],
                    h if h < 0.67 => [0.9, 0.2, 1.0],
                    h if h < 0.83 => [1.0, 0.9, 0.25],
                    _ => [0.8, 0.3, 0.6],
                };
                _light_ids.push(spawn_light(
                    &mut scene_db.world,
                    point_light(pos, color, 40.0, 10.5),
                ));
            }
        }

        // Additional rig perimeter lights
        for i in 0..12 {
            let angle = i as f32 * std::f32::consts::TAU / 12.0;
            let x = angle.cos() * 11.0;
            let z = angle.sin() * 14.5;
            _light_ids.push(spawn_light(
                &mut scene_db.world,
                point_light([x, 6.5, z], [1.0, 0.9, 0.75], 25.0, 15.0),
            ));
        }

        // Optional faint moon as directional component-like sky bloom (general ambient control)
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.045, 0.06, 0.08],
            intensity: 0.08,
        });
        renderer.set_clear_color([0.0025, 0.0035, 0.01, 1.0]);

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            renderer,
            last_frame: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 9.0, 35.0),
            cam_yaw: std::f32::consts::PI,
            cam_pitch: -0.15,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            scene_db,
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

        common::flush_scene_db(&self.scene_db, &self.queue);
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
    }
}
