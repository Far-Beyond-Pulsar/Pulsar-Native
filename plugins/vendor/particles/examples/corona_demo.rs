//! Corona — GPU particle system demo.
//!
//! Showcases 4 distinct emitter configurations at scale (1M total particles):
//!   1. Fountain  (Point, upward velocity, gravity, blue→cyan fade)
//!   2. Nebula    (Sphere, slow orbit, purple→pink)
//!   3. FireRing  (Sphere, radial outward, orange→red)
//!   4. Galaxy    (Sphere, spinning disk, white→blue)
//!
//! Controls:
//!   WASD        — fly
//!   Space/Shift — up/down
//!   Mouse       — look (click to grab)
//!   Escape      — release / quit

mod common;

use std::collections::HashSet;
use std::sync::Arc;

use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use helio_light_api::{GpuLight, LightType};
use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

// ── constants ─────────────────────────────────────────────────────────────────

const LOOK_SENS: f32 = 0.002;
const FLY_SPEED: f32 = 5.0;
const DRAG: f32 = 8.0;

// ── app ───────────────────────────────────────────────────────────────────────

struct App {
    state: Option<AppState>,
}

struct AppState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    capture: common::SurfaceCapture,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,
    renderer: Renderer,
    last_frame: std::time::Instant,
    start_time: std::time::Instant,
    // fly camera
    cam_pos: Vec3,
    yaw: f32,
    pitch: f32,
    velocity: Vec3,
    // input
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),
    // SceneDB: owns the corona emitters (and the floor object/sun light
    // spawned during setup) for the lifetime of the app, since emitter
    // transforms are updated every frame below.
    scene_db: pulsar_scenedb::SceneDb,
    corona_entities: [pulsar_scenedb::Entity; 4],
}

impl App {
    fn new() -> Self {
        Self { state: None }
    }

    fn build_emitters(elapsed: f32) -> [helio_pass_corona::GpuCoronaEmitter; 4] {
        let t = elapsed;

        // ── 1. Fountain: blue fountain bursting from origin ────────────────
        let fountain = helio_pass_corona::CoronaEmitterDescriptor {
            max_particles: 262_144,
            emit_rate: 8000.0,
            lifetime: 3.0,
            lifetime_variation: 1.0,
            start_size: [0.8, 0.8],
            end_size: [0.05, 0.05],
            start_color: [0.2, 0.6, 1.0, 1.0],
            end_color: [0.0, 1.0, 1.0, 0.0],
            velocity: [0.0, 8.0, 0.0],
            velocity_variation: [3.0, 2.0, 3.0],
            gravity: -5.0,
            shape: helio_pass_corona::CoronaEmitterShape::Point,
            texture_index: -1,
            position: [0.0, 0.0, 0.0],
        };

        // ── 2. Nebula: slow purple cloud orbiting origin ───────────────────
        let nebula_pos = [
            6.0 * t.cos() * 0.5,
            2.0 + 1.5 * (t * 0.7).sin(),
            6.0 * t.sin() * 0.5,
        ];
        let nebula = helio_pass_corona::CoronaEmitterDescriptor {
            max_particles: 131_072,
            emit_rate: 3000.0,
            lifetime: 6.0,
            lifetime_variation: 2.0,
            start_size: [1.5, 1.5],
            end_size: [3.0, 3.0],
            start_color: [0.8, 0.2, 0.8, 0.6],
            end_color: [0.3, 0.1, 0.6, 0.0],
            velocity: [0.0, 0.0, 0.0],
            velocity_variation: [0.5, 0.5, 0.5],
            gravity: 0.0,
            shape: helio_pass_corona::CoronaEmitterShape::Sphere { radius: 2.5 },
            texture_index: -1,
            position: nebula_pos,
        };

        // ── 3. FireRing: radial burst in XZ plane ──────────────────────────
        let ring_angle = t * 0.6;
        let ring_pos = [4.0 * ring_angle.cos(), 0.5, 4.0 * ring_angle.sin()];
        let fire = helio_pass_corona::CoronaEmitterDescriptor {
            max_particles: 65_536,
            emit_rate: 2000.0,
            lifetime: 1.5,
            lifetime_variation: 0.5,
            start_size: [0.6, 0.6],
            end_size: [0.0, 0.0],
            start_color: [1.0, 0.6, 0.0, 1.0],
            end_color: [1.0, 0.0, 0.0, 0.0],
            velocity: [3.0 * ring_angle.cos(), 0.0, 3.0 * ring_angle.sin()],
            velocity_variation: [2.0, 1.0, 2.0],
            gravity: -2.0,
            shape: helio_pass_corona::CoronaEmitterShape::Sphere { radius: 0.8 },
            texture_index: -1,
            position: ring_pos,
        };

        // ── 4. Galaxy: spinning disk of white-blue particles ───────────────
        let galaxy = helio_pass_corona::CoronaEmitterDescriptor {
            max_particles: 131_072,
            emit_rate: 4000.0,
            lifetime: 8.0,
            lifetime_variation: 3.0,
            start_size: [0.4, 0.4],
            end_size: [0.8, 0.8],
            start_color: [0.7, 0.8, 1.0, 0.8],
            end_color: [0.2, 0.3, 0.6, 0.0],
            velocity: [0.0, 0.0, 0.0],
            velocity_variation: [0.0, 0.0, 0.0],
            gravity: 0.0,
            shape: helio_pass_corona::CoronaEmitterShape::Sphere { radius: 6.0 },
            texture_index: -1,
            position: [0.0, 8.0, 0.0],
        };

        [
            fountain.to_gpu(),
            nebula.to_gpu(),
            fire.to_gpu(),
            galaxy.to_gpu(),
        ]
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
                        .with_title("Helio — Corona GPU Particle System Demo")
                        .with_inner_size(winit::dpi::LogicalSize::new(1600u32, 900u32)),
                )
                .expect("create window"),
        );

        let size = window.inner_size();
        let mut graphics = pollster::block_on(helio::HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            helio::HelioGraphicsConfig {
                vr_support: true,
                surface_usage: common::SurfaceCapture::from_env().usage(),
                application_name: "Corona Demo".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let surface_format = format;

        let config = RendererConfig::new(size.width, size.height, surface_format);

        // SceneDB is the sole scene authority: a renderer cannot be built
        // without a GPU mirror handle. This demo cohort places nothing
        // through a shared, persistent `SceneDb` (see `v3_demo_common`'s
        // module docs), so its mirror is created fresh, used to spawn this
        // scene's content, and dropped once setup finishes -- there is
        // nothing here another system needs to keep querying later.
        let mut scene_db = common::new_scene_db_with_gpu_mirror(&device, &queue);
        let scene_db_handle = common::scene_db_handle(&scene_db);

        let mut renderer = RendererBuilder::new(config, scene_db_handle)
            .with_graph(common::preset())
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;

        // ── Sky + lighting ───────────────────────────────────────────────────
        common::spawn_sky(&mut scene_db.world, [0.08, 0.10, 0.20]);
        common::spawn_light(
            &mut scene_db.world,
            GpuLight {
                position_range: [0.0, 0.0, 0.0, f32::MAX],
                direction_outer: [-0.3, -1.0, -0.5, 0.0],
                color_intensity: [0.9, 0.85, 0.75, 3.0],
                // No dynamic shadow-caster selection on this path yet (see
                // `LightComponent`'s module doc) -- `u32::MAX` means "no
                // shadow", not "unassigned".
                shadow_index: u32::MAX,
                light_type: LightType::Directional as u32,
                inner_angle: 0.0,
                ..Default::default()
            },
        );
        // SceneDB owns the light data end to end from here: the row uploaded
        // to the "scene_lights" buffer on insert, above, and
        // `ForwardLitPass` resolves that buffer by key every frame on its
        // own -- no renderer call needed at all.
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.08, 0.10, 0.18],
            intensity: 0.6,
        });
        renderer.set_clear_color([0.02, 0.03, 0.08, 1.0]);

        // ── Floor plane ─────────────────────────────────────────────────────
        let floor_mesh_id = common::spawn_mesh(
            &mut scene_db.world,
            common::plane_mesh([0.0, 0.0, 0.0], 30.0),
        );
        let floor_mat = common::spawn_material(
            &mut scene_db.world,
            common::make_material([0.06, 0.06, 0.08, 1.0], 0.8, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let _floor_entity = common::spawn_object(
            &mut scene_db.world,
            floor_mesh_id,
            floor_mat,
            Mat4::from_translation(glam::Vec3::new(0.0, -0.5, 0.0)),
            43.0,
        )
        .expect("floor object placement");

        // Spawn the 4 emitters, one per fixed particle slot (see
        // `spawn_corona_emitter`'s doc).
        let emitters = Self::build_emitters(0.0);
        let corona_entities = std::array::from_fn(|slot| {
            common::spawn_corona_emitter(&mut scene_db.world, slot as u32, emitters[slot])
        });

        self.state = Some(AppState {
            window,
            surface,
            capture: common::SurfaceCapture::from_env(),
            device,
            queue,
            surface_format,
            renderer,
            last_frame: std::time::Instant::now(),
            start_time: std::time::Instant::now(),
            cam_pos: Vec3::new(0.0, 4.0, 12.0),
            yaw: 0.0,
            pitch: -0.3,
            velocity: Vec3::ZERO,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            scene_db,
            corona_entities,
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
                        usage: state.capture.usage(),
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

                // Update fly camera
                let (dx, dy) = state.mouse_delta;
                state.mouse_delta = (0.0, 0.0);
                state.yaw -= dx * LOOK_SENS;
                state.pitch = (state.pitch - dy * LOOK_SENS).clamp(-1.5, 1.5);
                let orientation = Quat::from_euler(EulerRot::YXZ, state.yaw, state.pitch, 0.0);
                let forward = orientation * -Vec3::Z;
                let right = orientation * Vec3::X;
                let up = Vec3::Y;
                let mut accel = Vec3::ZERO;
                if state.keys.contains(&KeyCode::KeyW) {
                    accel += forward;
                }
                if state.keys.contains(&KeyCode::KeyS) {
                    accel -= forward;
                }
                if state.keys.contains(&KeyCode::KeyA) {
                    accel -= right;
                }
                if state.keys.contains(&KeyCode::KeyD) {
                    accel += right;
                }
                if state.keys.contains(&KeyCode::Space) {
                    accel += up;
                }
                if state.keys.contains(&KeyCode::ShiftLeft) {
                    accel -= up;
                }
                if accel.length_squared() > 0.0 {
                    accel = accel.normalize();
                }
                state.velocity += accel * FLY_SPEED * dt;
                state.velocity /= 1.0 + DRAG * dt;
                state.cam_pos += state.velocity * dt;

                let size = state.window.inner_size();
                let camera = Camera::perspective_look_at(
                    state.cam_pos,
                    state.cam_pos + orientation * -Vec3::Z,
                    orientation * Vec3::Y,
                    std::f32::consts::FRAC_PI_4,
                    size.width as f32 / size.height.max(1) as f32,
                    0.01,
                    200.0,
                );

                // ── Update emitters with time-varying positions ──────────────
                let elapsed = state.start_time.elapsed().as_secs_f32();
                let new_emitters = App::build_emitters(elapsed);
                for (slot, (&entity, &emitter)) in state
                    .corona_entities
                    .iter()
                    .zip(new_emitters.iter())
                    .enumerate()
                {
                    common::update_corona_emitter(
                        &mut state.scene_db.world,
                        entity,
                        slot as u32,
                        emitter,
                    );
                }

                // ── Render ────────────────────────────────────────────────────

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

fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();
    log::info!("=== Corona GPU Particle System Demo ===");
    log::info!("590,824 total particles across 4 emitters");
    log::info!("Fly around to see the full effect!");
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop error");
}
