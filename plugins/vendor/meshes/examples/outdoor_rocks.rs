//! Outdoor rocks demo — dozens of photorealistic rock meshes scattered across a
//! terrain, with animated billboard markers floating above them, and the ship
//! FBX parked nearby.
//!
//! Three rock types are loaded from the `3d/` directory:
//!   - Chiseled Rock (rafue)
//!   - Granite Rock (pjtsT)
//!   - Granite Rock (pkeeM)
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Q/E         — rotate sun (time of day)
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit
//!   F3          — toggle triangle debug view

mod v3_demo_common;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use helio_asset_compat::{load_scene_bytes_with_config, load_scene_file_with_config, LoadConfig};
use pulsar_scenedb::SceneDb;
use v3_demo_common::{
    cube_mesh, directional_light, flush_scene_db, make_material, new_scene_db_with_gpu_mirror,
    point_light, scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object,
    update_light,
};
use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

// ── Embedded ship asset ───────────────────────────────────────────────────────
const SHIP_BYTES: &[u8] = include_bytes!("assets/test.fbx");

// ── Rock scatter parameters ───────────────────────────────────────────────────
const ROCK_COUNT_PER_TYPE: usize = 30;
const FIELD_RADIUS: f32 = 80.0;
const BILLBOARD_EVERY_N: usize = 4; // place a billboard above every Nth rock

fn base_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Simple LCG random number generator (identical to ship_flight's).
fn lcg(seed: &mut u64) -> f32 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    ((*seed >> 33) as f32) / (u32::MAX as f32)
}
fn rand_s(seed: &mut u64) -> f32 {
    lcg(seed) * 2.0 - 1.0
}

// ── Marker color palette (RGBA, linear) ──────────────────────────────────────
const MARKER_COLORS: [[f32; 4]; 6] = [
    [1.0, 0.8, 0.1, 0.85], // amber
    [0.2, 0.8, 1.0, 0.85], // cyan
    [1.0, 0.3, 0.3, 0.85], // red
    [0.4, 1.0, 0.4, 0.85], // green
    [0.9, 0.4, 1.0, 0.85], // violet
    [1.0, 1.0, 1.0, 0.85], // white
];

// ─────────────────────────────────────────────────────────────────────────────

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
    last_frame: Instant,
    start_time: Instant,

    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    scene_db: SceneDb,
    sun_entity: pulsar_scenedb::Entity,
    sun_angle: f32,

    // ── VG debug mode ─────────────────────────────────────────────
    /// True when F3 VG triangle debug is active (debug_mode 20).
    vg_debug: bool,

    debug_overlay_enabled: bool,

    // ── CPU-side profiling ───────────────────────────────────────────────
    frame_count: u64,
    prof_frame_total: f64, // ms
    prof_update: f64,      // ms: sun, billboards, camera update
    prof_render: f64,      // ms: renderer.render() call
    prof_present: f64,     // ms: get_current_texture + present
    prof_frame_min: f64,
    prof_frame_max: f64,
}

impl App {
    fn new() -> Self {
        Self { state: None }
    }
}

// ── Camera free-fly helper ────────────────────────────────────────────────────
impl AppState {
    fn update_camera(&mut self, dt: f32) -> Vec3 {
        const SPEED: f32 = 28.0;
        const LOOK_SENS: f32 = 0.002;

        self.cam_yaw += self.mouse_delta.0 * LOOK_SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * LOOK_SENS).clamp(-1.48, 1.48);
        self.mouse_delta = (0.0, 0.0);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let forward = Vec3::new(sy * cp, sp, -cy * cp);
        let right = Vec3::new(cy, 0.0, sy);

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
            self.cam_pos.y += SPEED * dt;
        }
        if self.keys.contains(&KeyCode::ShiftLeft) {
            self.cam_pos.y -= SPEED * dt;
        }

        forward
    }
}

// ── ApplicationHandler ────────────────────────────────────────────────────────
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        // ── Window + wgpu setup ────────────────────────────────────────────
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Helio — Outdoor Rocks")
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
                application_name: "Outdoor Rocks".into(),
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
            .with_external_device()
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;
        renderer.set_clear_color([0.34, 0.48, 0.72, 1.0]);
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.38, 0.44, 0.50],
            intensity: 1.3,
        });

        // ── Sun light ─────────────────────────────────────────────────────
        let sun_angle: f32 = 0.62; // radians above horizon
        let sun_dir = Vec3::new(-sun_angle.cos(), -sun_angle.sin(), -0.6).normalize();
        let sun_entity = {
            let sun_entity = spawn_light(
                &mut scene_db.world,
                directional_light(sun_dir.to_array(), [1.0, 0.93, 0.75], 4.2),
            );
            spawn_light(
                &mut scene_db.world,
                point_light([0.0, 8.0, 0.0], [0.6, 0.7, 1.0], 12.0, 50.0),
            );
            spawn_light(
                &mut scene_db.world,
                point_light([60.0, 4.0, -40.0], [1.0, 0.85, 0.5], 8.0, 30.0),
            );
            sun_entity
        };

        // ── Ground plane ──────────────────────────────────────────────────
        let ground_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.28, 0.23, 0.18, 1.0], 0.92, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let ground_mesh = spawn_mesh(
            &mut scene_db.world,
            v3_demo_common::plane_mesh([0.0, 0.0, 0.0], 250.0),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            ground_mesh,
            ground_mat,
            Mat4::IDENTITY,
            250.0,
        );

        // ── Load rock FBX files as classic meshes ────────────
        // Each rock type is uploaded once and instanced; the GPU cull
        // compute handles per-instance culling with O(1) CPU.
        let asset_dir = base_dir().join("3d");
        let rock_paths = [
            asset_dir.join("Chiseled_Rock_rafue_Raw.fbx"),
            asset_dir.join("Granite_Rock_pjtsT_Raw.fbx"),
            asset_dir.join("Granite_Rock_pkeeM_Raw.fbx"),
        ];

        // Fallback cube material/mesh for any type that failed to load
        let fallback_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.35, 0.30, 0.25, 1.0], 0.85, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let fallback_mesh = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.0, 0.0], 0.5));
        // rock_vg[type] = Some(vec of (VirtualMeshId, material_slot_u32))
        let rock_meshes: Vec<Option<Vec<(pulsar_scenedb::Entity, pulsar_scenedb::Entity)>>> =
            rock_paths
                .iter()
                .map(|path| {
                    let scene = match load_scene_file_with_config(path, LoadConfig::default()) {
                        Ok(s) => s,
                        Err(e) => {
                            log::warn!("Could not load rock '{}': {e}", path.display());
                            return None;
                        }
                    };
                    let mat_ids =
                        helio_mesh_host::upload_scene_materials(&mut scene_db.world, &scene);
                    let entries: Vec<(pulsar_scenedb::Entity, pulsar_scenedb::Entity)> = scene
                        .meshes
                        .iter()
                        .map(|mesh| {
                            let vm_id = spawn_mesh(
                                &mut scene_db.world,
                                helio_mesh_api::MeshUpload {
                                    vertices: mesh.vertices.clone(),
                                    indices: mesh.indices.clone(),
                                },
                            );
                            let mat_id = mesh
                                .material_index
                                .and_then(|idx| mat_ids.get(idx))
                                .or_else(|| mat_ids.first())
                                .copied()
                                .unwrap_or(fallback_mat);
                            (vm_id, mat_id)
                        })
                        .collect();
                    Some(entries)
                })
                .collect();

        // ── Scatter rocks ─────────────────────────────────────────────────
        let mut seed: u64 = 0xDEAD_BEEF_CAFE_1234;
        let mut _global_rock_idx: usize = 0;
        let mut billboards = Vec::new();

        for rock_type in 0..3usize {
            let vg_entries = rock_meshes[rock_type].as_deref();

            for _ in 0..ROCK_COUNT_PER_TYPE {
                // Random position in a disc
                let angle = lcg(&mut seed) * std::f32::consts::TAU;
                let dist = FIELD_RADIUS * lcg(&mut seed).sqrt();
                let pos = Vec3::new(angle.cos() * dist, 0.0, angle.sin() * dist);

                // Random scale — small floor rocks, no overlapping giants
                let base_scale = 0.08 + lcg(&mut seed) * 0.22; // 0.08..0.30
                let scale = Vec3::new(
                    base_scale * (0.8 + lcg(&mut seed) * 0.4),
                    base_scale * (0.5 + lcg(&mut seed) * 0.5),
                    base_scale * (0.8 + lcg(&mut seed) * 0.4),
                );
                let rot = Quat::from_euler(
                    EulerRot::XYZ,
                    0.0,
                    rand_s(&mut seed) * std::f32::consts::PI,
                    0.0,
                );
                let transform = Mat4::from_scale_rotation_translation(scale, rot, pos);

                // World-space bounding sphere used for instance-level culling.
                let center = pos + Vec3::Y * scale.y * 0.5;
                let bounds_radius = base_scale * 1.2;

                if _global_rock_idx % BILLBOARD_EVERY_N == 0 {
                    billboards.push(helio_pass_billboard::BillboardInstance {
                        world_pos: [pos.x, pos.y + scale.y * 2.0, pos.z, 0.0],
                        scale_flags: [1.4, 1.4, 0.0, 0.0],
                        color: MARKER_COLORS
                            [_global_rock_idx / BILLBOARD_EVERY_N % MARKER_COLORS.len()],
                    });
                }

                match vg_entries {
                    None => {
                        let _ = spawn_object(
                            &mut scene_db.world,
                            fallback_mesh,
                            fallback_mat,
                            transform,
                            base_scale,
                        );
                    }
                    Some(entries) => {
                        for &(mesh_id, mat_id) in entries {
                            let _ = spawn_object(
                                &mut scene_db.world,
                                mesh_id,
                                mat_id,
                                transform,
                                bounds_radius,
                            );
                        }
                    }
                }

                _global_rock_idx += 1;
            }
        }
        for billboard in billboards {
            let entity = scene_db.world.spawn();
            scene_db.world.insert(
                entity,
                helio_components::BillboardComponent {
                    world_pos: billboard.world_pos,
                    scale_flags: billboard.scale_flags,
                    color: billboard.color,
                },
            );
        }

        // ── Ship (parked nearby) ───────────────────────────────────────────
        let ship_pos = Vec3::new(18.0, 0.0, -12.0);
        let load_result = load_scene_bytes_with_config(
            SHIP_BYTES,
            "fbx",
            Some(base_dir().as_path()),
            LoadConfig::default().with_uv_flip(false),
        );
        match load_result {
            Ok(scene) => {
                // Upload meshes + materials in a single traversal.
                {
                    let mat_ids =
                        helio_mesh_host::upload_scene_materials(&mut scene_db.world, &scene);
                    let ship_transform =
                        Mat4::from_rotation_translation(Quat::from_rotation_y(0.4), ship_pos);
                    for mesh in &scene.meshes {
                        let radius = mesh
                            .vertices
                            .iter()
                            .map(|v| Vec3::from_array(v.position).length())
                            .fold(0.5_f32, f32::max);
                        let mesh_id = spawn_mesh(
                            &mut scene_db.world,
                            helio_mesh_api::MeshUpload {
                                vertices: mesh.vertices.clone(),
                                indices: mesh.indices.clone(),
                            },
                        );
                        let mat = mesh
                            .material_index
                            .and_then(|idx| mat_ids.get(idx))
                            .or_else(|| mat_ids.first())
                            .copied()
                            .unwrap_or(fallback_mat);
                        let _ =
                            spawn_object(&mut scene_db.world, mesh_id, mat, ship_transform, radius);
                    }
                }
            }
            Err(e) => {
                log::warn!("Could not load ship FBX: {e} — placing fallback cube");
                let ship_mesh = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.0, 0.0], 1.5));
                let ship_mat = spawn_material(
                    &mut scene_db.world,
                    make_material([0.55, 0.70, 0.90, 1.0], 0.25, 0.75, [0.0, 0.0, 0.0], 0.0),
                );
                let transform = Mat4::from_translation(ship_pos);
                let _ = spawn_object(&mut scene_db.world, ship_mesh, ship_mat, transform, 1.5);
            }
        }

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format,
            renderer,
            last_frame: Instant::now(),
            start_time: Instant::now(),
            cam_pos: Vec3::new(0.0, 12.0, 50.0),
            cam_yaw: 0.0,
            cam_pitch: -0.18,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            scene_db,
            sun_entity,
            sun_angle,
            vg_debug: false,
            debug_overlay_enabled: false,
            frame_count: 0,
            prof_frame_total: 0.0,
            prof_update: 0.0,
            prof_render: 0.0,
            prof_present: 0.0,
            prof_frame_min: f64::MAX,
            prof_frame_max: 0.0,
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
                        present_mode: wgpu::PresentMode::AutoNoVsync,
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
                        physical_key: PhysicalKey::Code(KeyCode::F3),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                state.vg_debug = !state.vg_debug;
                state
                    .renderer
                    .set_debug_mode(if state.vg_debug { 20 } else { 0 });
                state.window.set_title(if state.vg_debug {
                    "Helio — Outdoor Rocks  [TRIANGLE DEBUG — F3 to exit]"
                } else {
                    "Helio — Outdoor Rocks"
                });
            }

            // F5: toggle debug overlay (F3 is VG debug)
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::F5),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                state.debug_overlay_enabled = !state.debug_overlay_enabled;
                if let Some(pass) = state
                    .renderer
                    .find_pass_mut::<helio_pass_debug_overlay::DebugOverlayPass>()
                {
                    pass.set_enabled(state.debug_overlay_enabled);
                }
                println!("[debug] debug overlay = {:?}", state.debug_overlay_enabled);
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
                let frame_start = Instant::now();
                let now = frame_start;
                let dt = now.duration_since(state.last_frame).as_secs_f32().min(0.05);
                let t = now.duration_since(state.start_time).as_secs_f32();
                state.last_frame = now;

                // ── Sun rotation ──────────────────────────────────────────
                let t_update_start = Instant::now();
                const SUN_SPEED: f32 = 0.6;
                if state.keys.contains(&KeyCode::KeyQ) {
                    state.sun_angle += SUN_SPEED * dt;
                }
                if state.keys.contains(&KeyCode::KeyE) {
                    state.sun_angle -= SUN_SPEED * dt;
                }
                state.sun_angle = state.sun_angle.clamp(-1.48, 1.48);
                let sun_dir = Vec3::new(
                    -state.sun_angle.cos(),
                    -state.sun_angle.sin().abs() - 0.3,
                    -0.6,
                )
                .normalize();
                {
                    update_light(
                        &mut state.scene_db.world,
                        state.sun_entity,
                        directional_light(sun_dir.to_array(), [1.0, 0.93, 0.75], 4.2),
                    );
                }
                flush_scene_db(&state.scene_db, &state.queue);

                // ── Camera ────────────────────────────────────────────────
                let forward = state.update_camera(dt);
                let size = state.window.inner_size();
                let camera = Camera::perspective_look_at(
                    state.cam_pos,
                    state.cam_pos + forward,
                    Vec3::Y,
                    std::f32::consts::FRAC_PI_4,
                    size.width as f32 / size.height.max(1) as f32,
                    0.15,
                    2000.0,
                );
                let t_update_ms = t_update_start.elapsed().as_secs_f64() * 1000.0;

                // ── Render ────────────────────────────────────────────────
                let t_render_start = Instant::now();

                if let Err(e) = state.renderer.render_window_frame(&camera, &state.surface) {
                    log::error!("Render error: {:?}", e);
                }
                let t_render_ms = t_render_start.elapsed().as_secs_f64() * 1000.0;

                let t_present_start = Instant::now();
                let t_present_ms = t_present_start.elapsed().as_secs_f64() * 1000.0;

                // ── Profiling accumulate + print every 100 frames ─────────
                let frame_ms = frame_start.elapsed().as_secs_f64() * 1000.0;
                state.prof_frame_total += frame_ms;
                state.prof_update += t_update_ms;
                state.prof_render += t_render_ms;
                state.prof_present += t_present_ms;
                if frame_ms < state.prof_frame_min {
                    state.prof_frame_min = frame_ms;
                }
                if frame_ms > state.prof_frame_max {
                    state.prof_frame_max = frame_ms;
                }
                state.frame_count += 1;

                const REPORT_EVERY: u64 = 100;
                if state.frame_count % REPORT_EVERY == 0 {
                    let n = REPORT_EVERY as f64;
                    let avg = state.prof_frame_total / n;
                    let fps = 1000.0 / avg;
                    let upd = state.prof_update / n;
                    let rnd = state.prof_render / n;
                    let pres = state.prof_present / n;
                    let other = avg - upd - rnd - pres;
                    eprintln!(
                        "[PROF #{:>6}] avg {:.2}ms ({:.0} fps) | min {:.2}ms max {:.2}ms",
                        state.frame_count, avg, fps, state.prof_frame_min, state.prof_frame_max
                    );
                    eprintln!(
                        "             update {:.2}ms  render {:.2}ms  present {:.2}ms  other {:.2}ms",
                        upd, rnd, pres, other
                    );
                    state.prof_frame_total = 0.0;
                    state.prof_update = 0.0;
                    state.prof_render = 0.0;
                    state.prof_present = 0.0;
                    state.prof_frame_min = f64::MAX;
                    state.prof_frame_max = 0.0;
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
