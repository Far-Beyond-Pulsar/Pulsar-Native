//! Load and display a 3D model file using the new `helio` wrapper.

mod v3_demo_common;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use helio_asset_compat::load_scene_file_with_config;
use pulsar_scenedb::{Entity, SceneDb};
use v3_demo_common::{
    new_scene_db_with_gpu_mirror, point_light, scene_db_handle, spawn_light, spawn_material,
    spawn_mesh, spawn_object, update_point_light,
};
use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

use crate::v3_demo_common::cube_mesh;

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
    point_light_id: Entity,
    point_light_pos: Vec3,
    last_frame: Instant,
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),
}

impl App {
    fn new() -> Self {
        Self { state: None }
    }
}

impl AppState {
    fn update_camera(&mut self, dt: f32) -> Vec3 {
        const SPEED: f32 = 20.0;
        const LOOK_SENS: f32 = 0.002;

        self.cam_yaw += self.mouse_delta.0 * LOOK_SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * LOOK_SENS).clamp(-1.5, 1.5);
        self.mouse_delta = (0.0, 0.0);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let forward = Vec3::new(sy * cp, sp, -cy * cp);
        let right = Vec3::new(cy, 0.0, sy);
        let up = Vec3::Y;

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

        forward
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
                        .with_title("Helio - Asset Loading")
                        .with_inner_size(winit::dpi::PhysicalSize::new(1280, 720)),
                )
                .expect("failed to create window"),
        );

        let size = window.inner_size();
        let mut graphics = pollster::block_on(helio::HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            helio::HelioGraphicsConfig {
                vr_support: true,
                application_name: "Load Fbx".into(),
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
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;
        renderer.set_clear_color([0.03, 0.03, 0.04, 1.0]);
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.06, 0.06, 0.09],
            intensity: 1.0,
        });

        let scene_path = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "test.fbx".to_string());
        let config = helio_asset_compat::LoadConfig::default().with_uv_flip(false);
        match load_scene_file_with_config(&scene_path, config) {
            Ok(scene) => {
                log::info!(
                    "Loaded '{}' ({} meshes, {} materials)",
                    scene.name,
                    scene.meshes.len(),
                    scene.materials.len()
                );
                let material_ids =
                    helio_mesh_host::upload_scene_materials(&mut scene_db.world, &scene);
                for mesh in scene.meshes {
                    let radius = mesh
                        .vertices
                        .iter()
                        .map(|v| Vec3::from_array(v.position).length())
                        .fold(0.5, f32::max);
                    let mesh_id = spawn_mesh(
                        &mut scene_db.world,
                        helio_mesh_api::MeshUpload {
                            vertices: mesh.vertices,
                            indices: mesh.indices,
                        },
                    );
                    let material = mesh
                        .material_index
                        .and_then(|index| material_ids.get(index).copied())
                        .unwrap_or_else(|| {
                            spawn_material(
                                &mut scene_db.world,
                                v3_demo_common::make_material(
                                    [0.7, 0.7, 0.75, 1.0],
                                    0.6,
                                    0.0,
                                    [0.0, 0.0, 0.0],
                                    0.0,
                                ),
                            )
                        });
                    let _ = spawn_object(
                        &mut scene_db.world,
                        mesh_id,
                        material,
                        glam::Mat4::IDENTITY,
                        radius,
                    );
                }
            }
            Err(error) => {
                log::warn!(
                    "Failed to load '{}': {}. Using fallback cube.",
                    scene_path,
                    error
                );
                let mesh = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.0, 0.0], 0.5));
                let material = spawn_material(
                    &mut scene_db.world,
                    v3_demo_common::make_material(
                        [0.55, 0.68, 0.9, 1.0],
                        0.35,
                        0.15,
                        [0.0, 0.0, 0.0],
                        0.0,
                    ),
                );
                let _ = spawn_object(
                    &mut scene_db.world,
                    mesh,
                    material,
                    glam::Mat4::IDENTITY,
                    0.9,
                );
            }
        }

        let point_light_pos = Vec3::new(0.0, 3.0, 0.0);
        let point_light_id = spawn_light(
            &mut scene_db.world,
            point_light(point_light_pos.to_array(), [1.0, 0.95, 0.8], 12.0, 18.0),
        );

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format,
            renderer,
            scene_db,
            point_light_id,
            point_light_pos,
            last_frame: Instant::now(),
            cam_pos: Vec3::new(0.0, 2.0, 7.0),
            cam_yaw: 0.0,
            cam_pitch: -0.2,
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
                    let _ = state
                        .window
                        .set_cursor_grab(winit::window::CursorGrabMode::None);
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
                    let grabbed = state
                        .window
                        .set_cursor_grab(winit::window::CursorGrabMode::Confined)
                        .or_else(|_| {
                            state
                                .window
                                .set_cursor_grab(winit::window::CursorGrabMode::Locked)
                        })
                        .is_ok();
                    if grabbed {
                        state.cursor_grabbed = true;
                        state.window.set_cursor_visible(false);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = now.duration_since(state.last_frame).as_secs_f32().min(0.05);
                state.last_frame = now;

                let forward = state.update_camera(dt);
                const LIGHT_SPEED: f32 = 5.0;
                if state.keys.contains(&KeyCode::KeyI) {
                    state.point_light_pos.z -= LIGHT_SPEED * dt;
                }
                if state.keys.contains(&KeyCode::KeyK) {
                    state.point_light_pos.z += LIGHT_SPEED * dt;
                }
                if state.keys.contains(&KeyCode::KeyJ) {
                    state.point_light_pos.x -= LIGHT_SPEED * dt;
                }
                if state.keys.contains(&KeyCode::KeyL) {
                    state.point_light_pos.x += LIGHT_SPEED * dt;
                }
                update_point_light(
                    &mut state.scene_db.world,
                    state.point_light_id,
                    state.point_light_pos,
                    [1.0, 0.95, 0.8],
                    12.0,
                    18.0,
                );

                let size = state.window.inner_size();
                let camera = Camera::perspective_look_at(
                    state.cam_pos,
                    state.cam_pos + forward,
                    Vec3::Y,
                    std::f32::consts::FRAC_PI_4,
                    size.width as f32 / size.height.max(1) as f32,
                    0.1,
                    200.0,
                );

                // Upload this frame's SceneDB writes to the GPU mirror; without it they
                // are never visible to the renderer (Helio#266).
                v3_demo_common::flush_scene_db(&state.scene_db, state.renderer.queue());
                if let Err(error) = state.renderer.render_window_frame(&camera, &state.surface) {
                    log::error!("Render error: {:?}", error);
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
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop error");
}
