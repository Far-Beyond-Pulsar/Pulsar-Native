//! Basic scene example using helio v3.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit

mod v3_demo_common;

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use helio_light_host::LightComponent;
use pulsar_scenedb::SceneDb;
use v3_demo_common::{
    cube_mesh, make_material, new_scene_db_with_gpu_mirror, plane_mesh, point_light,
    scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object,
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

#[cfg(test)]
mod scene_light_tests {
    use super::*;
    use pulsar_scenedb::World;

    #[test]
    fn projection_reads_current_scene_db_positions() {
        let mut world = World::new();
        let entity = spawn_light(
            &mut world,
            point_light([7.0, 8.0, 9.0], [1.0, 1.0, 1.0], 4.0, 5.0),
        );
        let light = world
            .get::<helio_light_host::LightComponent>(entity)
            .expect("SceneDB light missing");
        assert_eq!(&light.position_range[..3], &[7.0, 8.0, 9.0]);
    }
}

pub fn main() {
    env_logger::init();
    log::info!("Starting Helio Render V2 Basic Example");

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

    // Free-camera state
    cam_pos: glam::Vec3,
    cam_yaw: f32,   // radians, horizontal rotation
    cam_pitch: f32, // radians, vertical rotation (clamped)
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    // Authoritative scene state; Helio receives transient pass projections.
    scene_db: SceneDb,
    light_p0_entity: pulsar_scenedb::Entity,
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
                        .with_title("Helio Render V2 – Scene-Driven")
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
                application_name: "Render V2 Basic".into(),
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
        let mut renderer = RendererBuilder::new(config, scene_db_handle(&scene_db))
            .with_editor_mode(true)
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;

        let mat = spawn_material(
            &mut scene_db.world,
            make_material([0.7, 0.7, 0.72, 1.0], 0.7, 0.0, [0.0, 0.0, 0.0], 0.0),
        );

        let cube1 = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.0, 0.0], 0.5));
        let cube2 = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.0, 0.0], 0.4));
        let cube3 = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.0, 0.0], 0.3));
        let ground = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 5.0));

        let _ = spawn_object(
            &mut scene_db.world,
            cube1,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.5, 0.0)),
            0.5,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            cube2,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-2.0, 0.4, -1.0)),
            0.4,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            cube3,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(2.0, 0.3, 0.5)),
            0.3,
        );
        let _ = spawn_object(&mut scene_db.world, ground, mat, glam::Mat4::IDENTITY, 5.0);

        // p0 bobs up/down (animated), p1 and p2 are static
        let p0_init = [0.0f32, 2.2, 0.0];
        let p1 = [-3.5f32, 2.0, -1.5];
        let p2 = [3.5f32, 1.5, 1.5];
        let light_p0_entity = {
            let p0 = spawn_light(
                &mut scene_db.world,
                point_light(p0_init, [1.0, 0.55, 0.15], 6.0, 5.0),
            );
            spawn_light(
                &mut scene_db.world,
                point_light(p1, [0.25, 0.5, 1.0], 5.0, 6.0),
            );
            spawn_light(
                &mut scene_db.world,
                point_light(p2, [1.0, 0.3, 0.5], 5.0, 6.0),
            );
            p0
        };
        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format,
            renderer,
            last_frame: std::time::Instant::now(),
            start_time: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 2.5, 7.0),
            cam_yaw: 0.0, // yaw=0 looks down -Z toward the scene
            cam_pitch: -0.2,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            scene_db,
            light_p0_entity,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };

        match event {
            // ── Exit ──────────────────────────────────────────────────────────
            WindowEvent::CloseRequested => {
                log::info!("Shutting down");
                event_loop.exit();
            }
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
                    // First Escape releases the cursor
                    state.cursor_grabbed = false;
                    let _ = state.window.set_cursor_grab(CursorGrabMode::None);
                    state.window.set_cursor_visible(true);
                } else {
                    event_loop.exit();
                }
            }

            // ── Keyboard held state ───────────────────────────────────────────
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

            // ── Mouse button — grab cursor on click ───────────────────────────
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !state.cursor_grabbed {
                    // Try confined first, fall back to locked
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

            // ── Window resize ─────────────────────────────────────────────────
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
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

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
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

impl AppState {
    fn render(&mut self, dt: f32) {
        // ── Camera movement ────────────────────────────────────────────────────
        const SPEED: f32 = 5.0;
        const LOOK_SENS: f32 = 0.002;

        // Apply mouse look — yaw left/right, pitch up/down (non-inverted)
        self.cam_yaw += self.mouse_delta.0 * LOOK_SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * LOOK_SENS).clamp(-1.5, 1.5);
        self.mouse_delta = (0.0, 0.0);

        // Standard FPS basis: yaw=0 looks down -Z
        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let forward = glam::Vec3::new(sy * cp, sp, -cy * cp);
        let right = glam::Vec3::new(cy, 0.0, sy);
        let up = glam::Vec3::Y;

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
            self.cam_pos += up * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::ShiftLeft) {
            self.cam_pos -= up * SPEED * dt;
        }

        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;
        let time = self.start_time.elapsed().as_secs_f32();

        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            200.0,
        );

        // ── Acquire surface ────────────────────────────────────────────────────

        // p0 bobs up/down per-frame
        let p0 = [0.0f32, 2.2 + (time * 0.7).sin() * 0.3, 0.0];
        {
            self.scene_db
                .world
                .get_mut::<helio_light_host::LightComponent>(self.light_p0_entity)
                .expect("SceneDB animated light disappeared")
                .clone_from(&LightComponent::from(helio_light_api::LightRow::from(
                    point_light(p0, [1.0, 0.55, 0.15], 6.0, 5.0),
                )));
        }
        // Upload this frame's SceneDB writes to the GPU mirror; without it they
        // are never visible to the renderer (Helio#266).
        v3_demo_common::flush_scene_db(&self.scene_db, self.renderer.queue());
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
    }
}
