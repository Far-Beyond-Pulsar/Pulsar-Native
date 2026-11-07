//! Space Station — the most complex Helio example scene.
//!
//! A massive orbital station assembled from hundreds of axis-aligned primitives
//! arranged with trigonometry into rings, spokes, solar arrays, and engine pods.
//!
//! Controls:
//!   WASD / Space / Shift  — fly  (speed 40 m/s)
//!   Mouse drag            — look (click to grab cursor)
//!   Escape                — release cursor / exit

mod v3_demo_common;
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use pulsar_scenedb::{Entity, SceneDb, World};
use v3_demo_common::{
    box_mesh, directional_light, make_material, new_scene_db_with_gpu_mirror, point_light,
    scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object, update_light,
};

use std::collections::HashSet;
use std::sync::Arc;
use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

const PI: f32 = std::f32::consts::PI;
const TAU: f32 = std::f32::consts::TAU;

fn main() {
    env_logger::init();
    log::info!("Starting Space Station example");
    EventLoop::new()
        .expect("event loop")
        .run_app(&mut App { state: None })
        .expect("event loop run");
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
    scene_db: SceneDb,
    last_frame: std::time::Instant,
    frame_count: u64,
    // camera
    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),
    // animated lights
    hub_light_ids: [Entity; 2],
    hab_ring_light_ids: [Entity; 4],
    engine_light_ids: [Entity; 4],
    docking_light_id: Entity,
    beacon_light_ids: [Entity; 2],
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
                        .with_title("Helio — Space Station  |  WASD fly  |  Click: grab cursor")
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
                application_name: "Space Station".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let surface_format = format;
        let fmt = format;

        let config = RendererConfig::new(size.width, size.height, fmt);
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let mut renderer = RendererBuilder::new(config, scene_db_handle(&scene_db))
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;

        renderer.set_feature_live(helio::AmbientLight {
            color: [0.08, 0.10, 0.18],
            intensity: 0.035,
        });

        // Single material for the whole station (cool grey metal)
        let mat = spawn_material(
            &mut scene_db.world,
            make_material([0.62, 0.63, 0.66, 1.0], 0.55, 0.35, [0.0; 3], 0.0),
        );

        build_station(&mut scene_db.world, mat);

        // Static directional (sunlight)
        spawn_light(
            &mut scene_db.world,
            directional_light([0.35, -0.65, 0.25], [0.72, 0.82, 1.0], 0.10),
        );

        let hub_light_ids = [
            spawn_light(
                &mut scene_db.world,
                point_light([0.0, 14.0, 0.0], [0.82, 0.90, 1.0], 8.0, 28.0),
            ),
            spawn_light(
                &mut scene_db.world,
                point_light([0.0, -9.0, 0.0], [0.70, 0.80, 1.0], 6.0, 22.0),
            ),
        ];
        let hab_ring_light_ids = [
            spawn_light(
                &mut scene_db.world,
                point_light([35.0, 6.0, 0.0], [0.78, 0.88, 1.0], 5.5, 20.0),
            ),
            spawn_light(
                &mut scene_db.world,
                point_light([-35.0, 6.0, 0.0], [0.78, 0.88, 1.0], 5.5, 20.0),
            ),
            spawn_light(
                &mut scene_db.world,
                point_light([0.0, 6.0, 35.0], [0.78, 0.88, 1.0], 5.5, 20.0),
            ),
            spawn_light(
                &mut scene_db.world,
                point_light([0.0, 6.0, -35.0], [0.78, 0.88, 1.0], 5.5, 20.0),
            ),
        ];
        let engine_light_ids = [
            spawn_light(
                &mut scene_db.world,
                point_light([5.0, 5.0, 58.0], [1.0, 0.42, 0.06], 10.0, 22.0),
            ),
            spawn_light(
                &mut scene_db.world,
                point_light([-5.0, 5.0, 58.0], [1.0, 0.42, 0.06], 10.0, 22.0),
            ),
            spawn_light(
                &mut scene_db.world,
                point_light([5.0, -5.0, 58.0], [1.0, 0.42, 0.06], 10.0, 22.0),
            ),
            spawn_light(
                &mut scene_db.world,
                point_light([-5.0, -5.0, 58.0], [1.0, 0.42, 0.06], 10.0, 22.0),
            ),
        ];
        let docking_light_id = spawn_light(
            &mut scene_db.world,
            point_light([0.0, 0.0, -54.0], [1.0, 1.0, 0.92], 7.5, 26.0),
        );
        let beacon_light_ids = [
            spawn_light(
                &mut scene_db.world,
                point_light([0.0, 6.0, 65.0], [1.0, 0.04, 0.04], 0.0, 14.0),
            ),
            spawn_light(
                &mut scene_db.world,
                point_light([0.0, 6.0, -65.0], [1.0, 0.04, 0.04], 0.0, 14.0),
            ),
        ];

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: fmt,
            renderer,
            scene_db,
            last_frame: std::time::Instant::now(),
            frame_count: 0,
            cam_pos: glam::Vec3::new(0.0, 55.0, 175.0),
            cam_yaw: 0.0,
            cam_pitch: -0.18,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            hub_light_ids,
            hab_ring_light_ids,
            engine_light_ids,
            docking_light_id,
            beacon_light_ids,
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

            WindowEvent::Resized(sz) if sz.width > 0 && sz.height > 0 => {
                state.surface.configure(
                    &state.device,
                    &wgpu::SurfaceConfiguration {
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        format: state.surface_format,
                        width: sz.width,
                        height: sz.height,
                        present_mode: wgpu::PresentMode::Fifo,
                        alpha_mode: wgpu::CompositeAlphaMode::Auto,
                        view_formats: vec![],
                        desired_maximum_frame_latency: 2,
                        color_space: wgpu::SurfaceColorSpace::Auto,
                    },
                );
                state.renderer.set_render_size(sz.width, sz.height);
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
        let Some(s) = &mut self.state else { return };
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if s.cursor_grabbed {
                s.mouse_delta.0 += dx as f32;
                s.mouse_delta.1 += dy as f32;
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
        const SPEED: f32 = 40.0;
        const SENS: f32 = 0.002;
        self.cam_yaw += self.mouse_delta.0 * SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * SENS).clamp(-1.5, 1.5);
        self.mouse_delta = (0.0, 0.0);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let fwd = glam::Vec3::new(sy * cp, sp, -cy * cp);
        let right = glam::Vec3::new(cy, 0.0, sy);
        if self.keys.contains(&KeyCode::KeyW) {
            self.cam_pos += fwd * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyS) {
            self.cam_pos -= fwd * SPEED * dt;
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

        let sz = self.window.inner_size();
        let aspect = sz.width as f32 / sz.height.max(1) as f32;
        let time = self.frame_count as f32 * 0.016;

        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.5,
            1000.0,
        );

        // Animated engine flicker
        let flicker = 1.0 + 0.18 * (time * 8.7).sin() * (time * 13.3).cos();
        // Slow station-wide pulse
        let pulse = 1.0 + 0.06 * (time * 0.7).sin();
        // Red warning beacon (1 Hz strobe)
        let beacon = (0.5 + 0.5 * (time * TAU).sin()).max(0.0_f32);

        let _ = update_light(
            &mut self.scene_db.world,
            self.hub_light_ids[0],
            point_light([0.0, 14.0, 0.0], [0.82, 0.90, 1.0], 8.0 * pulse, 28.0),
        );
        let _ = update_light(
            &mut self.scene_db.world,
            self.hub_light_ids[1],
            point_light([0.0, -9.0, 0.0], [0.70, 0.80, 1.0], 6.0 * pulse, 22.0),
        );

        let hab_pos = [
            [35.0_f32, 6.0, 0.0],
            [-35.0, 6.0, 0.0],
            [0.0, 6.0, 35.0],
            [0.0, 6.0, -35.0],
        ];
        for (i, &id) in self.hab_ring_light_ids.iter().enumerate() {
            let _ = update_light(
                &mut self.scene_db.world,
                id,
                point_light(hab_pos[i], [0.78, 0.88, 1.0], 5.5 * pulse, 20.0),
            );
        }

        let eng_pos = [
            [5.0_f32, 5.0, 58.0],
            [-5.0, 5.0, 58.0],
            [5.0, -5.0, 58.0],
            [-5.0, -5.0, 58.0],
        ];
        for (i, &id) in self.engine_light_ids.iter().enumerate() {
            let _ = update_light(
                &mut self.scene_db.world,
                id,
                point_light(eng_pos[i], [1.0, 0.42, 0.06], 10.0 * flicker, 22.0),
            );
        }

        let _ = update_light(
            &mut self.scene_db.world,
            self.beacon_light_ids[0],
            point_light([0.0, 6.0, 65.0], [1.0, 0.04, 0.04], 6.0 * beacon, 14.0),
        );
        let _ = update_light(
            &mut self.scene_db.world,
            self.beacon_light_ids[1],
            point_light([0.0, 6.0, -65.0], [1.0, 0.04, 0.04], 6.0 * beacon, 14.0),
        );

        // docking light steady
        let _ = update_light(
            &mut self.scene_db.world,
            self.docking_light_id,
            point_light([0.0, 0.0, -54.0], [1.0, 1.0, 0.92], 7.5, 26.0),
        );

        // Upload this frame's SceneDB writes to the GPU mirror; without it they
        // are never visible to the renderer (Helio#266).
        v3_demo_common::flush_scene_db(&self.scene_db, self.renderer.queue());
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
        self.frame_count += 1;
    }
}

// ── Station geometry builder ──────────────────────────────────────────────────

fn build_station(world: &mut World, mat: Entity) {
    macro_rules! add {
        ($cx:expr, $cy:expr, $cz:expr, $hx:expr, $hy:expr, $hz:expr) => {{
            let _mesh = spawn_mesh(world, box_mesh([$cx, $cy, $cz], [$hx, $hy, $hz]));
            let _ = spawn_object(
                world,
                _mesh,
                mat,
                glam::Mat4::IDENTITY,
                ((($hx) * ($hx) + ($hy) * ($hy) + ($hz) * ($hz)) as f32).sqrt(),
            );
        }};
    }

    // 1. PRIMARY HUB CYLINDER (r=7, Y: -15..+25)
    let hub_r = 7.0_f32;
    let hub_mid_y = 5.0_f32;
    let hub_half_h = 20.0_f32;
    let n_hub = 16_u32;
    for i in 0..n_hub {
        let a = i as f32 * TAU / n_hub as f32;
        let cx = hub_r * a.cos();
        let cz = hub_r * a.sin();
        let chord = 2.0 * hub_r * (PI / n_hub as f32).sin();
        add!(cx, hub_mid_y, cz, chord * 0.5 + 0.2, hub_half_h, 0.55);
    }
    add!(0.0, hub_mid_y + hub_half_h + 0.6, 0.0, 6.5, 0.6, 6.5);
    add!(0.0, hub_mid_y - hub_half_h - 0.6, 0.0, 7.5, 0.6, 7.5);
    for deck in 0..4_i32 {
        let dy = -12.0 + deck as f32 * 9.5;
        add!(0.0, dy, 0.0, 5.2, 0.18, 5.2);
    }

    // 2. COMMAND TOWER (Y=25 upward)
    let cmd_y0 = hub_mid_y + hub_half_h;
    for i in 0..4_u32 {
        let s = 4.0 - i as f32 * 0.75;
        let cy = cmd_y0 + 4.5 + i as f32 * 5.5;
        add!(0.0, cy, 0.0, s, 2.2, s);
        add!(s * 0.7, cy, s * 0.7, s * 0.25, 2.0, s * 0.25);
        add!(-s * 0.7, cy, s * 0.7, s * 0.25, 2.0, s * 0.25);
    }
    add!(0.0, cmd_y0 + 38.0, 0.0, 0.22, 7.0, 0.22);
    add!(0.0, cmd_y0 + 46.0, 0.0, 5.0, 0.22, 5.0);
    for i in 0..8_u32 {
        let a = i as f32 * TAU / 8.0;
        let dr = 4.0_f32;
        add!(dr * a.cos(), cmd_y0 + 45.8, dr * a.sin(), 0.28, 0.28, 0.28);
    }
    for i in 0..4_u32 {
        let a = i as f32 * PI * 0.5;
        let ax = 2.8 * a.cos();
        let az = 2.8 * a.sin();
        add!(ax, cmd_y0 + 16.0, az, 0.12, 5.5, 0.12);
        add!(4.5 * a.cos(), cmd_y0 + 18.0, 4.5 * a.sin(), 1.5, 0.12, 0.12);
    }

    // 3. ENGINEERING SECTION (below hub, Y <= -15)
    let bot_y = hub_mid_y - hub_half_h;
    add!(0.0, bot_y - 5.0, 0.0, 11.5, 4.5, 11.5);
    for i in 0..4_u32 {
        let a = i as f32 * PI * 0.5 + PI * 0.25;
        let rx = 15.5 * a.cos();
        let rz = 15.5 * a.sin();
        add!(rx, bot_y - 5.0, rz, 0.16, 4.5, 7.0);
    }
    for ring in 0..3_u32 {
        let rr = 8.5 + ring as f32 * 2.0;
        let ry = bot_y - 2.0 - ring as f32 * 3.5;
        let n = 12_u32;
        let half = 2.0 * rr * (PI / n as f32).sin() * 0.45;
        for i in 0..n {
            let a = i as f32 * TAU / n as f32;
            let cx = rr * a.cos();
            let cz = rr * a.sin();
            add!(cx, ry, cz, half, 0.4, 0.4);
        }
    }

    // 4. ATTITUDE-CONTROL PODS on hub equator (8 pods)
    for i in 0..8_u32 {
        let a = i as f32 * TAU / 8.0;
        let r = hub_r + 1.5;
        let px = r * a.cos();
        let pz = r * a.sin();
        add!(px, hub_mid_y + 2.0, pz, 1.0, 0.8, 1.0);
        add!(px, hub_mid_y - 2.0, pz, 1.0, 0.8, 1.0);
        let nx = (r + 1.2) * a.cos();
        let nz = (r + 1.2) * a.sin();
        add!(nx, hub_mid_y + 2.0, nz, 0.35, 0.35, 0.35);
        add!(nx, hub_mid_y - 2.0, nz, 0.35, 0.35, 0.35);
    }

    // 5. HABITAT RING A (r=35, Y=5, 20 modules)
    let hab_a_r = 35.0_f32;
    let hab_a_y = 5.0_f32;
    let n_a = 20_u32;
    for i in 0..n_a {
        let a = i as f32 * TAU / n_a as f32;
        let cx = hab_a_r * a.cos();
        let cz = hab_a_r * a.sin();
        add!(cx, hab_a_y, cz, 3.0, 2.4, 3.0);
        let wo = hab_a_r + 3.4;
        add!(wo * a.cos(), hab_a_y + 0.4, wo * a.sin(), 1.0, 0.9, 1.0);
        add!(cx, hab_a_y - 3.0, cz, 1.6, 0.4, 1.6);
    }
    let rail_a = hab_a_r + 5.2;
    for i in 0..n_a {
        let a0 = i as f32 * TAU / n_a as f32;
        let a1 = (i + 1) as f32 * TAU / n_a as f32;
        let am = (a0 + a1) * 0.5;
        let cx = rail_a * am.cos();
        let cz = rail_a * am.sin();
        let len = ((rail_a * a1.cos() - rail_a * a0.cos()).powi(2)
            + (rail_a * a1.sin() - rail_a * a0.sin()).powi(2))
        .sqrt()
            * 0.45;
        add!(cx, hab_a_y, cz, len, 0.25, 0.25);
    }
    for i in 0..8_u32 {
        let a = i as f32 * TAU / 8.0;
        for node in 1..4_u32 {
            let r = node as f32 * hab_a_r / 3.6;
            let s = 0.65 - node as f32 * 0.12;
            add!(r * a.cos(), hab_a_y, r * a.sin(), s, s, s);
        }
        let mr = hab_a_r * 0.55;
        let mx = mr * a.cos();
        let mz = mr * a.sin();
        let perp = a + PI * 0.5;
        for &sign in &[-2.2_f32, 2.2] {
            add!(
                mx + sign * perp.cos(),
                hab_a_y + 1.8,
                mz + sign * perp.sin(),
                0.22,
                1.8,
                0.22
            );
        }
    }

    // 6. INDUSTRIAL RING B (r=62, Y=-5, 16 modules)
    let ind_r = 62.0_f32;
    let ind_y = -5.0_f32;
    let n_b = 16_u32;
    for i in 0..n_b {
        let a = i as f32 * TAU / n_b as f32;
        let cx = ind_r * a.cos();
        let cz = ind_r * a.sin();
        add!(cx, ind_y, cz, 5.5, 3.5, 5.5);
        let ox = (ind_r + 7.0) * a.cos();
        let oz = (ind_r + 7.0) * a.sin();
        add!(ox, ind_y + 1.0, oz, 2.5, 2.5, 2.5);
        add!(cx, ind_y + 4.8, cz, 2.8, 0.9, 2.8);
        add!(cx, ind_y - 4.2, cz, 3.2, 0.4, 3.2);
    }
    for i in 0..4_u32 {
        let a = i as f32 * PI * 0.5;
        for node in 1..6_u32 {
            let r = node as f32 * ind_r / 5.8;
            add!(r * a.cos(), ind_y, r * a.sin(), 1.05, 1.05, 1.05);
            if node % 2 == 0 {
                let perp = a + PI * 0.5;
                let bx = r * a.cos();
                let bz = r * a.sin();
                for &sign in &[-2.8_f32, 2.8] {
                    add!(
                        bx + sign * perp.cos(),
                        ind_y + 2.2,
                        bz + sign * perp.sin(),
                        0.28,
                        2.2,
                        0.28
                    );
                }
            }
        }
    }
    let rail_b = ind_r + 6.5;
    for i in 0..n_b {
        let a0 = i as f32 * TAU / n_b as f32;
        let a1 = (i + 1) as f32 * TAU / n_b as f32;
        let am = (a0 + a1) * 0.5;
        let len = ((rail_b * a1.cos() - rail_b * a0.cos()).powi(2)
            + (rail_b * a1.sin() - rail_b * a0.sin()).powi(2))
        .sqrt()
            * 0.44;
        add!(rail_b * am.cos(), ind_y, rail_b * am.sin(), len, 0.32, 0.32);
    }

    // 7. SOLAR POWER ARRAYS (4 arms at 45, 135, 225, 315 deg)
    let sol_base = ind_r + 8.0;
    let n_seg = 5_u32;
    let seg_step = 14.0_f32;
    for arm in 0..4_u32 {
        let arm_a = arm as f32 * PI * 0.5 + PI * 0.25;
        let perp = arm_a + PI * 0.5;
        for seg in 0..n_seg {
            let r = sol_base + seg as f32 * seg_step;
            let cx = r * arm_a.cos();
            let cz = r * arm_a.sin();
            add!(cx, 0.0, cz, 1.3, 0.55, 1.3);
            add!(cx, 3.5, cz, 0.35, 0.35, 0.35);
            add!(cx, -3.5, cz, 0.35, 0.35, 0.35);
            for panel_i in -1_i32..=1 {
                let pd = panel_i as f32 * 9.5;
                let px = cx + pd * perp.cos();
                let pz = cz + pd * perp.sin();
                add!(px, 6.5, pz, 6.5, 0.11, 3.8);
                add!(px, -6.5, pz, 6.5, 0.11, 3.8);
            }
        }
        for seg in 0..(n_seg - 1) {
            let r0 = sol_base + seg as f32 * seg_step + seg_step * 0.5;
            let cx = r0 * arm_a.cos();
            let cz = r0 * arm_a.sin();
            add!(cx, 0.0, cz, 0.45, 0.45, 0.45);
            add!(cx, 3.5, cz, 0.28, 0.28, 0.28);
            add!(cx, -3.5, cz, 0.28, 0.28, 0.28);
        }
        for seg in 0..4_u32 {
            let r = sol_base + seg as f32 * seg_step + seg_step * 0.5;
            let cx = r * arm_a.cos();
            let cz = r * arm_a.sin();
            for &sign in &[-13.0_f32, 13.0] {
                add!(
                    cx + sign * perp.cos(),
                    -10.0,
                    cz + sign * perp.sin(),
                    0.14,
                    8.0,
                    5.5
                );
            }
        }
        let tip_r = sol_base + n_seg as f32 * seg_step;
        let tx = tip_r * arm_a.cos();
        let tz = tip_r * arm_a.sin();
        add!(tx, 0.0, tz, 2.2, 2.2, 2.2);
        add!(tx, 4.5, tz, 0.14, 3.0, 0.14);
    }

    // 8. FORWARD DOCKING ARM & NODE (along -Z)
    for seg in 0..3_u32 {
        let z = -(13.0 + seg as f32 * 10.5);
        add!(0.0, 0.0, z, 1.9, 1.9, 4.8);
    }
    add!(0.0, 0.0, -50.0, 9.5, 9.5, 6.5);
    for i in 0..4_u32 {
        let a = i as f32 * PI * 0.5;
        let px = 11.5 * a.cos();
        let py = 11.5 * a.sin();
        add!(px, py, -50.0, 2.4, 2.4, 4.5);
        add!(px, py, -55.0, 3.2, 3.2, 0.55);
        add!(px * 1.15, py * 1.15, -55.5, 0.3, 0.3, 0.3);
    }
    add!(0.0, 0.0, -59.5, 0.55, 0.55, 5.5);
    add!(0.0, 0.0, -65.5, 2.8, 0.18, 2.8);

    // 9. ENGINE SECTION (along +Z)
    add!(0.0, 0.0, 16.0, 2.8, 2.8, 6.5);
    add!(0.0, 0.0, 28.5, 3.8, 3.8, 6.0);
    add!(0.0, 0.0, 38.0, 9.5, 9.5, 3.8);
    const ENG: [[f32; 2]; 4] = [[5.2, 5.2], [-5.2, 5.2], [5.2, -5.2], [-5.2, -5.2]];
    for [ex, ey] in ENG {
        add!(ex, ey, 46.5, 3.2, 3.2, 6.0);
        add!(ex, ey, 53.5, 4.5, 4.5, 2.5);
        for tr in 0..6_u32 {
            let ta = tr as f32 * TAU / 6.0;
            let tx = ex + 5.2 * ta.cos();
            let ty = ey + 5.2 * ta.sin();
            add!(tx, ty, 49.5, 0.55, 0.55, 2.5);
        }
        add!(ex * 0.55, ey * 0.55, 40.5, 1.6, 1.6, 1.0);
    }
    for ring in 0..4_u32 {
        let rz = 42.0 + ring as f32 * 2.5;
        let rr = 8.5 + ring as f32 * 0.8;
        let n = 16_u32;
        let half = 2.0 * rr * (PI / n as f32).sin() * 0.44;
        for i in 0..n {
            let a = i as f32 * TAU / n as f32;
            let cx = rr * a.cos();
            let cz = rr * a.sin();
            add!(cx, cz, rz, half, 0.32, 0.32);
        }
    }

    // 10. SPINE TRUSS (long Z-axis structural backbone)
    for seg in 0..6_u32 {
        let sz = -8.0 + seg as f32 * 4.5;
        add!(3.5, 0.0, sz, 0.35, 0.35, 0.35);
        add!(-3.5, 0.0, sz, 0.35, 0.35, 0.35);
        add!(0.0, 3.5, sz, 0.35, 0.35, 0.35);
        add!(0.0, -3.5, sz, 0.35, 0.35, 0.35);
    }

    // 11. OBSERVATION DECK (top of command tower)
    let obs_y = cmd_y0 + 29.0;
    add!(0.0, obs_y, 0.0, 5.0, 0.9, 5.0);
    for i in 0..8_u32 {
        let a = i as f32 * TAU / 8.0;
        let vr = 5.2_f32;
        add!(vr * a.cos(), obs_y + 0.9, vr * a.sin(), 0.65, 0.55, 0.65);
    }
}
