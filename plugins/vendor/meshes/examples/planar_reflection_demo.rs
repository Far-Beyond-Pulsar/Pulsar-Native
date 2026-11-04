// Planar Reflection Demo
// A glossy floor plane with coloured objects above it, demonstrating the
// planar reflection capture system.
//
// The sky and GI left Helio with their plugins (Pulsar `plugins/vendor/sky`,
// `plugins/vendor/gi`): no atmosphere or GI probes here.
//
// Controls:
//   WASD        — move
//   Space/Shift — up/down
//   Mouse       — look (click to grab)
//   Escape      — release cursor / quit

mod v3_demo_common;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::{
    Camera, HelioGraphicsConfig, HelioGraphicsContext, Renderer, RendererBuilder, RendererConfig,
};
use pulsar_scenedb::{Entity, SceneDb};
use v3_demo_common::{
    box_mesh, cube_mesh, directional_light, make_material, new_scene_db_with_gpu_mirror,
    plane_mesh, scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object,
    spawn_reflection_capture_box, sphere_mesh, update_object_transform,
};
use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

const LOOK_SENS: f32 = 0.002;
const FLY_SPEED: f32 = 8.0;
const DRAG: f32 = 6.0;

struct App {
    state: Option<AppState>,
}

struct AppState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    adapter: wgpu::Adapter,
    surface_format: wgpu::TextureFormat,
    renderer: Renderer,
    scene_db: SceneDb,
    frame_count: u64,
    last_frame: Instant,
    cam_pos: Vec3,
    yaw: f32,
    pitch: f32,
    velocity: Vec3,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),
    spinning_cube: Entity,
    orbiting_sphere: Entity,
    sphere_angle: f32,
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
                        .with_title("Helio — Planar Reflection Demo")
                        .with_inner_size(winit::dpi::PhysicalSize::new(1600, 900)),
                )
                .expect("create window"),
        );

        let size = window.inner_size();
        let mut graphics = pollster::block_on(HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            HelioGraphicsConfig {
                vr_support: true,
                application_name: "Planar Reflection Demo".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let adapter = graphics.adapter.clone();
        let surface_format = graphics.surface_config.format;

        let mut renderer_config = RendererConfig::new(size.width, size.height, surface_format);
        // HELIO_DEBUG_MODE=30 -> SSR confidence, 31 -> SSR colour. See the debug
        // blocks in deferred_lighting.wgsl for the full list.
        if let Ok(mode) = std::env::var("HELIO_DEBUG_MODE") {
            if let Ok(mode) = mode.parse::<u32>() {
                renderer_config.debug_mode = mode;
            }
        }

        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);

        // ── Materials ──────────────────────────────────────────────────────
        let floor_mat = spawn_material(
            &mut scene_db.world,
            make_material(
                [0.97, 0.97, 0.97, 1.0], // silver-tinted mirror
                0.001,                   // near-zero roughness = mirror-sharp
                1.0,                     // metallic mirror
                [0.0; 3],
                0.0,
            ),
        );
        let red_mat = spawn_material(
            &mut scene_db.world,
            make_material([1.0, 0.2, 0.2, 1.0], 0.3, 0.0, [0.0; 3], 0.0),
        );
        let blue_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.2, 0.4, 1.0, 1.0], 0.2, 0.1, [0.0; 3], 0.0),
        );
        let green_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.2, 0.9, 0.3, 1.0], 0.6, 0.0, [0.0; 3], 0.0),
        );
        let gold_mat = spawn_material(
            &mut scene_db.world,
            make_material([1.0, 0.85, 0.4, 1.0], 0.15, 1.0, [0.0; 3], 0.0),
        );
        let white_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.95, 0.95, 0.98, 1.0], 0.4, 0.0, [0.0; 3], 0.0),
        );

        // ── Meshes ─────────────────────────────────────────────────────────
        let cube_mesh = spawn_mesh(&mut scene_db.world, cube_mesh([0.0; 3], 0.5));
        let sphere_mesh = spawn_mesh(&mut scene_db.world, sphere_mesh([0.0; 3], 0.5));
        let floor_mesh = spawn_mesh(&mut scene_db.world, plane_mesh([0.0; 3], 6.0));
        let pillar_mesh = spawn_mesh(&mut scene_db.world, box_mesh([0.0; 3], [0.15, 1.5, 0.15]));

        // ── Floor (mirror) ─────────────────────────────────────────────────
        let _ = spawn_object(
            &mut scene_db.world,
            floor_mesh,
            floor_mat,
            Mat4::from_translation(Vec3::new(0.0, -1.5, 0.0)),
            6.0,
        );

        // ── Pillars ────────────────────────────────────────────────────────
        for x in [-2.0_f32, 2.0] {
            for z in [-2.0_f32, 2.0] {
                let _ = spawn_object(
                    &mut scene_db.world,
                    pillar_mesh,
                    white_mat,
                    Mat4::from_translation(Vec3::new(x, -0.75, z)),
                    0.5,
                );
            }
        }

        // ── Central objects ────────────────────────────────────────────────
        let cube_id = spawn_object(
            &mut scene_db.world,
            cube_mesh,
            red_mat,
            Mat4::from_translation(Vec3::new(-1.2, 0.5, 0.0)),
            0.5,
        )
        .unwrap();

        let _ = spawn_object(
            &mut scene_db.world,
            sphere_mesh,
            blue_mat,
            Mat4::from_translation(Vec3::new(1.2, 0.5, -0.8)),
            0.5,
        );

        let _ = spawn_object(
            &mut scene_db.world,
            sphere_mesh,
            gold_mat,
            Mat4::from_translation(Vec3::new(0.0, 0.5, 1.2)),
            0.5,
        );

        let sphere_id = spawn_object(
            &mut scene_db.world,
            sphere_mesh,
            green_mat,
            Mat4::from_translation(Vec3::new(-1.5, 1.8, 1.5)),
            0.5,
        )
        .unwrap();

        // ── Reflection capture ─────────────────────────────────────────────
        // A box capture spanning the room, centred on the box's own centre.
        // Its cubemap layer is assigned by the probe bake, so this contributes
        // nothing until the scene has been baked.
        spawn_reflection_capture_box(
            &mut scene_db.world,
            Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0)),
            [6.0, 3.0, 6.0],
            1.0,
        );

        // ── Sky ────────────────────────────────────────────────────────────
        // `ambient_color` from the removed `SkyActor` builder is now the
        // renderer's own ambient term (`set_ambient`, below). The atmosphere
        // is the sky plugin's (Pulsar); this scene has none.

        // ── Lights ─────────────────────────────────────────────────────────
        spawn_light(
            &mut scene_db.world,
            directional_light([-0.5, -1.0, -0.3], [1.0, 0.95, 0.9], 12.0),
        );

        spawn_light(
            &mut scene_db.world,
            helio_light_api::GpuLight {
                position_range: [3.0, 4.0, 2.0, 10.0],
                direction_outer: [0.0; 4],
                color_intensity: [1.0, 0.85, 0.6, 30.0],
                shadow_index: 0,
                light_type: helio_light_api::LightType::Point as u32,
                inner_angle: 0.0,
                _pad: 0,
                ..Default::default()
            },
        );

        spawn_light(
            &mut scene_db.world,
            helio_light_api::GpuLight {
                position_range: [-3.0, 3.0, -2.0, 10.0],
                direction_outer: [0.0; 4],
                color_intensity: [0.4, 0.6, 1.0, 20.0],
                shadow_index: 0,
                light_type: helio_light_api::LightType::Point as u32,
                inner_angle: 0.0,
                _pad: 0,
                ..Default::default()
            },
        );

        // ── Build renderer ─────────────────────────────────────────────────
        let mut renderer = RendererBuilder::new(renderer_config, scene_db_handle(&scene_db))
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;

        renderer.set_feature_live(helio::AmbientLight {
            color: [0.15, 0.18, 0.25],
            intensity: 1.0,
        });

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            adapter,
            surface_format,
            renderer,
            scene_db,
            frame_count: 0,
            last_frame: Instant::now(),
            cam_pos: Vec3::new(0.0, 2.0, 6.0),
            yaw: 0.0,
            pitch: -0.2,
            velocity: Vec3::ZERO,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            spinning_cube: cube_id,
            orbiting_sphere: sphere_id,
            sphere_angle: 0.0,
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let state = self.state.as_mut().unwrap();

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(s) if s.width > 0 && s.height > 0 => {
                let w = s.width;
                let h = s.height;
                let caps = state.surface.get_capabilities(&state.adapter);
                state.surface.configure(
                    &state.device,
                    &wgpu::SurfaceConfiguration {
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        format: state.surface_format,
                        width: w,
                        height: h,
                        present_mode: wgpu::PresentMode::Fifo,
                        alpha_mode: caps.alpha_modes[0],
                        color_space: wgpu::SurfaceColorSpace::Auto,
                        view_formats: vec![],
                        desired_maximum_frame_latency: 2,
                    },
                );
                state.renderer.set_render_size(w, h);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(key),
                        state: key_state,
                        ..
                    },
                ..
            } => match (key, key_state) {
                (KeyCode::Escape, ElementState::Pressed) => {
                    if state.cursor_grabbed {
                        state.cursor_grabbed = false;
                        let _ = state.window.set_cursor_grab(CursorGrabMode::None);
                        state.window.set_cursor_visible(true);
                    } else {
                        event_loop.exit();
                    }
                }
                _ => {
                    if key_state == ElementState::Pressed {
                        state.keys.insert(key);
                    } else {
                        state.keys.remove(&key);
                    }
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
            WindowEvent::RedrawRequested => {
                let dt = state.last_frame.elapsed().as_secs_f32().min(0.05);
                state.last_frame = Instant::now();

                // ---- Input ----
                let (dx, dy) = state.mouse_delta;
                state.mouse_delta = (0.0, 0.0);
                state.yaw -= dx * LOOK_SENS;
                state.pitch = (state.pitch - dy * LOOK_SENS).clamp(-1.5, 1.5);

                let orientation = Quat::from_euler(EulerRot::YXZ, state.yaw, state.pitch, 0.0);
                let forward = orientation * -Vec3::Z;
                let right = orientation * Vec3::X;

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
                    accel += Vec3::Y;
                }
                if state.keys.contains(&KeyCode::ShiftLeft) {
                    accel -= Vec3::Y;
                }
                if accel.length_squared() > 0.0 {
                    accel = accel.normalize();
                }
                state.velocity += accel * FLY_SPEED * dt;
                state.velocity /= 1.0 + DRAG * dt;
                state.cam_pos += state.velocity * dt;

                // ---- Animate ----
                state.frame_count += 1;
                let angle = state.frame_count as f32 * 0.02;
                let cube_transform = Mat4::from_axis_angle(Vec3::Y, angle)
                    * Mat4::from_axis_angle(Vec3::X, angle * 0.5)
                    * Mat4::from_translation(Vec3::new(-1.2, 0.5, 0.0));
                let _ = update_object_transform(
                    &mut state.scene_db.world,
                    &mut state.renderer,
                    state.spinning_cube,
                    cube_transform,
                );

                state.sphere_angle += dt * 0.6;
                let orbit_x = 2.5 * state.sphere_angle.cos();
                let orbit_z = 2.5 * state.sphere_angle.sin();
                let sphere_pos = Vec3::new(orbit_x, 1.0 + 0.5 * state.sphere_angle.sin(), orbit_z);
                let _ = update_object_transform(
                    &mut state.scene_db.world,
                    &mut state.renderer,
                    state.orbiting_sphere,
                    Mat4::from_translation(sphere_pos),
                );

                // ---- Camera ----
                let target = state.cam_pos + forward;
                let up = orientation * Vec3::Y;
                let size = state.window.inner_size();
                let camera = Camera::perspective_look_at(
                    state.cam_pos,
                    target,
                    up,
                    std::f32::consts::FRAC_PI_4,
                    size.width as f32 / size.height.max(1) as f32,
                    0.1,
                    500.0,
                );

                // ---- Render ----
                // Upload this frame's SceneDB writes to the GPU mirror; without it they
                // are never visible to the renderer (Helio#266).
                v3_demo_common::flush_scene_db(&state.scene_db, state.renderer.queue());
                if let Err(e) = state.renderer.render_window_frame(&camera, &state.surface) {
                    log::error!("Render: {:?}", e);
                }
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

fn main() {
    env_logger::init();
    EventLoop::new().unwrap().run_app(&mut App::new()).unwrap();
}
