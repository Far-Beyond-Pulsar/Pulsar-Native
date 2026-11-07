//! Billboard example – minimal scene to verify `BillboardComponent` /
//! `BillboardPass` actually render, in isolation from the engine integration.
//!
//! As of this writing, nothing in the Pulsar-Native engine (`engine_backend`/
//! `helio-component`) ever inserts a `BillboardComponent` row anywhere, and no
//! other Helio example did either — `BillboardPass` is wired into the default
//! graph (`helio-presets::add_late_passes`, built-in spotlight
//! texture) but had literally zero test coverage. This example spawns
//! `BillboardComponent` rows directly (the same "insert the SceneDB record,
//! SceneDB owns the deferred GPU upload" pattern `common.rs` uses for
//! its rows) to answer:
//! does the pass itself work at all?
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit

mod common;

use common::{
    box_mesh, make_material, new_scene_db_with_gpu_mirror, plane_mesh, point_light,
    scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object,
};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use helio_light_host::BillboardComponent;
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

/// Spawns a billboard directly as a SceneDB `BillboardComponent` row --
/// the usual idiom (`world.spawn()` + `world.insert(entity, component)`),
/// which no helper
/// exists for yet since no example has used billboards before this one.
///
/// `scale` is the quad's world-space (or screen-space, see `screen_scale`)
/// width/height. `screen_scale`: when true the quad keeps a constant
/// projected size regardless of distance (see the shader's `screen_scale`
/// branch); when false it's a normal world-space-sized quad that shrinks
/// with distance like any other object.
fn spawn_billboard(
    world: &mut pulsar_scenedb::World,
    position: [f32; 3],
    scale: [f32; 2],
    color: [f32; 4],
    screen_scale: bool,
) -> pulsar_scenedb::Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        BillboardComponent {
            world_pos: [position[0], position[1], position[2], 0.0],
            scale_flags: [
                scale[0],
                scale[1],
                if screen_scale { 1.0 } else { 0.0 },
                0.0,
            ],
            color,
        },
    );
    entity
}

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
    start_time: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    scene_db: SceneDb,
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
                        .with_title("Helio - Billboard Demo")
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
                application_name: "Billboard Demo".into(),
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

        // Ground plane, purely for visual scale/reference and to give
        // `BillboardPass::set_occluded_by_geometry(true)` (set by default in
        // `add_late_passes`) something to occlude against.
        let mat = spawn_material(
            &mut scene_db.world,
            make_material([0.5, 0.5, 0.52, 1.0], 0.9, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let floor = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 20.0));
        let _ = spawn_object(&mut scene_db.world, floor, mat, glam::Mat4::IDENTITY, 20.0);

        // A wall segment directly between the camera's start position and one
        // billboard, to sanity-check occlusion-by-geometry as well as plain
        // visibility.
        let wall = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [2.0, 1.5, 0.1]),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            wall,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(6.0, 1.5, 0.0)),
            2.0,
        );

        spawn_light(
            &mut scene_db.world,
            point_light([0.0, 4.0, 0.0], [1.0, 0.95, 0.85], 3.0, 15.0),
        );

        // Five billboards: a color spread at varying distance/height, one
        // deliberately placed behind the wall above (x=8, occluded from a
        // camera at the origin looking down +X), one using `screen_scale` to
        // demonstrate constant-projected-size behavior.
        spawn_billboard(
            &mut scene_db.world,
            [3.0, 1.0, -2.0],
            [1.0, 1.0],
            [1.0, 0.2, 0.2, 1.0],
            false,
        );
        spawn_billboard(
            &mut scene_db.world,
            [3.0, 1.0, 0.0],
            [1.5, 1.5],
            [0.2, 1.0, 0.2, 1.0],
            false,
        );
        spawn_billboard(
            &mut scene_db.world,
            [3.0, 1.0, 2.0],
            [1.0, 1.0],
            [0.2, 0.4, 1.0, 1.0],
            false,
        );
        spawn_billboard(
            &mut scene_db.world,
            [8.0, 1.5, 0.0],
            [1.2, 1.2],
            [1.0, 1.0, 0.2, 1.0],
            false,
        ); // behind the wall
        spawn_billboard(
            &mut scene_db.world,
            [3.0, 2.5, 0.0],
            [0.3, 0.3],
            [1.0, 0.4, 1.0, 1.0],
            true,
        ); // screen_scale

        renderer.set_feature_live(helio::AmbientLight {
            color: [1.0, 0.97, 0.92],
            intensity: 0.15,
        });
        renderer.set_clear_color([0.02, 0.02, 0.06, 1.0]);

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            renderer,
            last_frame: std::time::Instant::now(),
            start_time: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 1.6, 0.0),
            cam_yaw: 0.0,
            cam_pitch: 0.0,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            scene_db,
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
        const SPEED: f32 = 3.0;
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
            100.0,
        );

        // See indoor_cathedral.rs's render loop for why this is required:
        // `#[gpu(layout = packed)]` components (BillboardComponent included)
        // only get `mark_gpu_row_dirty`-tracked by `World::insert`/`get_mut` --
        // the actual `queue.write_buffer` upload only happens here. Almost
        // every other example in this crate is missing this call (see the
        // session note filed alongside this example); without it, nothing
        // GPU-mirrored renders, not just billboards.
        common::flush_scene_db(&self.scene_db, &self.queue);

        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
    }
}
