//! 1 Million Cubes — GPU-driven instancing stress test.
//!
//! Renders 1,000,000 cubes in a 100×100×100 grid, all sharing a single mesh
//! and one of 10 materials.  The renderer automatically batches objects with
//! the same (mesh, material) pair into instanced indirect draw calls, so these
//! 1M cubes become at most 10 GPU draw calls.
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
use helio_mats::texture_mips::{mip_chain, TexelKind};
use pulsar_scenedb::SceneDb;
use v3_demo_common::{
    cube_mesh, directional_light, make_material, new_scene_db_with_gpu_mirror, point_light,
    scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object_with_movability,
};

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

const BRICK_TEX_SIZE: u32 = 512;

fn hash2(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(374761393) ^ y.wrapping_mul(668265263);
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    ((h ^ (h >> 16)) & 0xffff) as f32 / 65535.0
}

/// Procedural running-bond brick pattern: (sRGB albedo, tangent-space GL normal map).
fn generate_brick_textures() -> (Vec<u8>, Vec<u8>) {
    let n = BRICK_TEX_SIZE as i32;
    let rows = 8;
    let per_row = 4;
    let (brick_h, brick_w) = (n / rows, n / per_row);
    let mortar = 6.0f32;
    let bevel = 5.0f32;

    let mut height = vec![0.0f32; (n * n) as usize];
    let mut albedo = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        let row = y / brick_h;
        let x_shift = if row % 2 == 1 { brick_w / 2 } else { 0 };
        for x in 0..n {
            let sx = (x + x_shift) % n;
            let col = sx / brick_w;
            let lx = (sx % brick_w) as f32;
            let ly = (y % brick_h) as f32;
            // Distance to the nearest brick edge, in texels.
            let d = lx
                .min(brick_w as f32 - 1.0 - lx)
                .min(ly)
                .min(brick_h as f32 - 1.0 - ly);
            let inside = ((d - mortar * 0.5) / bevel).clamp(0.0, 1.0);
            let grain = hash2(x as u32 / 2, y as u32 / 2) * 0.06 + hash2(x as u32, y as u32) * 0.03;
            let h = inside.sqrt() * 0.9 + grain * inside;
            height[(y * n + x) as usize] = if inside > 0.0 { h } else { grain * 0.3 };

            let id = (row * per_row + col) as u32;
            let tone = 0.75 + 0.5 * hash2(id, 7);
            let speckle = 0.9 + 0.2 * hash2(x as u32, y as u32 + 91);
            let (r, g, b) = if inside > 0.0 {
                (0.55 * tone, 0.22 * tone, 0.16 * tone)
            } else {
                (0.55, 0.53, 0.48)
            };
            let to_srgb = |v: f32| ((v * speckle).clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
            let i = ((y * n + x) * 4) as usize;
            albedo[i..i + 4].copy_from_slice(&[to_srgb(r), to_srgb(g), to_srgb(b), 255]);
        }
    }

    let at = |x: i32, y: i32| height[(y.rem_euclid(n) * n + x.rem_euclid(n)) as usize];
    let strength = 6.0f32;
    let mut normal = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let dx = (at(x + 1, y) - at(x - 1, y)) * 0.5 * strength;
            // Image rows run downward; GL convention wants +Y up.
            let dy = (at(x, y + 1) - at(x, y - 1)) * 0.5 * strength;
            let v = glam::Vec3::new(-dx, dy, 1.0).normalize();
            let i = ((y * n + x) * 4) as usize;
            for c in 0..3 {
                normal[i + c] = ((v[c] * 0.5 + 0.5) * 255.0).round() as u8;
            }
            normal[i + 3] = 255;
        }
    }
    (albedo, normal)
}

/// Uploads a `BRICK_TEX_SIZE`-square RGBA8 texture with a full mip chain
/// filtered as `texels` says (`helio_mats::texture_mips`).
fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    store: &mut pulsar_scenedb::gpu::TextureStore,
    rgba: &[u8],
    texels: TexelKind,
) -> u32 {
    let chain = mip_chain(rgba, BRICK_TEX_SIZE, BRICK_TEX_SIZE, texels);
    let mips: Vec<&[u8]> = chain.iter().map(|level| level.texels.as_slice()).collect();
    store
        .register_mips(
            device,
            queue,
            &wgpu::TextureDescriptor {
                label: Some("procedural brick"),
                size: wgpu::Extent3d {
                    width: BRICK_TEX_SIZE,
                    height: BRICK_TEX_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: mips.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: if texels.is_srgb() {
                    wgpu::TextureFormat::Rgba8UnormSrgb
                } else {
                    wgpu::TextureFormat::Rgba8Unorm
                },
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            &mips,
        )
        .expect("register brick texture")
}

fn main() {
    env_logger::init();
    log::info!("Starting 1 Million Cubes Instancing Demo");

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
    start_time: std::time::Instant,

    cam_pos: glam::Vec3,
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

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Helio – 1 Million Cubes")
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
                application_name: "One Million Cubes".into(),
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
        let mut texture_store = pulsar_scenedb::gpu::TextureStore::new(2);
        let (brick_albedo, brick_normal) = generate_brick_textures();
        let brick_base = upload_texture(
            &device,
            &queue,
            &mut texture_store,
            &brick_albedo,
            TexelKind::Colour,
        );
        let brick_nrm = upload_texture(
            &device,
            &queue,
            &mut texture_store,
            &brick_normal,
            TexelKind::Normal,
        );
        let scene_handle = scene_db_handle(&scene_db)
            .with_texture_store(Arc::new(RwLock::new(texture_store)))
            .unwrap();
        let mut renderer = RendererBuilder::new(config, scene_handle)
            .with_external_device()
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;
        renderer.set_editor_mode(true);
        renderer.set_feature(helio_mats::MaterialSampler(wgpu::SamplerDescriptor {
            label: Some("Brick repeat trilinear sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        }));

        let palette = [
            [0.91, 0.18, 0.18, 1.0],
            [0.18, 0.91, 0.18, 1.0],
            [0.18, 0.18, 0.91, 1.0],
            [0.91, 0.91, 0.18, 1.0],
            [0.91, 0.18, 0.91, 1.0],
            [0.18, 0.91, 0.91, 1.0],
            [0.91, 0.55, 0.18, 1.0],
            [0.55, 0.18, 0.91, 1.0],
            [0.91, 0.55, 0.55, 1.0],
            [0.55, 0.91, 0.55, 1.0],
        ];

        let materials: Vec<_> = palette
            .iter()
            .map(|&color| {
                // Soft tint multiplied over the brick albedo.
                let tint = color.map(|c| 0.6 + 0.4 * c);
                let mut material =
                    make_material([tint[0], tint[1], tint[2], 1.0], 0.8, 0.0, [0.0; 3], 0.0);
                material.tex_base_color = brick_base;
                material.tex_normal = brick_nrm;
                material.flags |= helio_mats::FLAG_HAS_NORMAL_MAP;
                spawn_material(&mut scene_db.world, material)
            })
            .collect();

        let cube_mesh_id = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.0, 0.0], 0.4));

        // 100×100×100 = 1,000,000 cubes in a centred grid
        let grid_size = 100i32;
        let spacing = 1.5;
        let half = grid_size as f32 * spacing * 0.5;

        let timer = std::time::Instant::now();
        let mat_count = materials.len();

        for x in 0..grid_size {
            for y in 0..grid_size {
                for z in 0..grid_size {
                    let pos = glam::Vec3::new(
                        x as f32 * spacing - half,
                        y as f32 * spacing - half,
                        z as f32 * spacing - half,
                    );

                    let mat_idx = (((y * grid_size + z) as f32 / (grid_size * grid_size) as f32)
                        * mat_count as f32) as usize
                        % mat_count;

                    let transform = glam::Mat4::from_translation(pos);
                    let _ = spawn_object_with_movability(
                        &mut scene_db.world,
                        cube_mesh_id,
                        materials[mat_idx],
                        transform,
                        0.5,
                        Some(helio::Movability::Static),
                    );
                }
            }
        }

        let elapsed = timer.elapsed();
        println!(
            "Created 1,000,000 cube instances in {:.2}s",
            elapsed.as_secs_f32()
        );

        spawn_light(
            &mut scene_db.world,
            directional_light([0.5, -0.8, 0.3], [1.0, 0.95, 0.85], 8.0),
        );

        spawn_light(
            &mut scene_db.world,
            point_light([-60.0, 40.0, -60.0], [0.3, 0.6, 1.0], 4.0, 120.0),
        );

        spawn_light(
            &mut scene_db.world,
            point_light([60.0, 40.0, 60.0], [1.0, 0.6, 0.3], 4.0, 120.0),
        );

        self.state = Some(AppState {
            window,
            surface,
            device,
            surface_format,
            renderer,
            scene_db,
            last_frame: std::time::Instant::now(),
            start_time: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 50.0, 120.0),
            cam_yaw: 0.0,
            cam_pitch: -0.35,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else {
            return;
        };

        match event {
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
                let sc = wgpu::SurfaceConfiguration {
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
                state.surface.configure(&state.device, &sc);
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
        let Some(state) = &mut self.state else {
            return;
        };
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
        const SPEED: f32 = 20.0;
        const LOOK_SENS: f32 = 0.002;

        self.cam_yaw += self.mouse_delta.0 * LOOK_SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * LOOK_SENS).clamp(-1.5, 1.5);
        self.mouse_delta = (0.0, 0.0);

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

        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            500.0,
        );

        // Upload this frame's SceneDB writes to the GPU mirror; without it they
        // are never visible to the renderer (Helio#266).
        v3_demo_common::flush_scene_db(&self.scene_db, self.renderer.queue());
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
    }
}
