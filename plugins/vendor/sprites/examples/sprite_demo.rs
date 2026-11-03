//! Minimal standalone 2D sprite-batch demo.
//!
//! Deliberately bypasses `helio::Renderer`/`Scene` — those are 3D-specific
//! (see `helio-pass-sprite-batch`'s module docs) — and drives
//! `helio_core::RenderGraph` directly against a `SpriteCullPass`
//! (GPU cull + radix sort) feeding a single `SpriteBatchPass`,
//! proving out the independent 2D render-graph path end to end: its own
//! device/window setup, its own orthographic camera, no 3D scene graph.
//!
//! Exercises every feature of the passes:
//! - A ring of spinning, color-cycled sprites alternating between two atlas
//!   layers (a filled disc and a ring), proving the texture-array atlas.
//!   Animated every frame via `update_sprite` on stable handles created once
//!   in `resumed()` — not a per-frame clear-and-rebuild.
//! - A stack of overlapping, differently-tinted, semi-transparent sprites at
//!   the center with different `depth` values, proving the GPU radix-sorted
//!   back-to-front draw order (wrong order would show through-blending in
//!   the wrong sequence).
//! - A field of off-screen decoy sprites outside the camera's view rect,
//!   inserted *once* and never updated again — proving view-frustum culling
//!   runs on the GPU (`SpriteCullPass`) and that the decoys' instance bytes
//!   are never re-uploaded after frame one.

use std::sync::Arc;
use std::time::Instant;

use helio_core::RenderGraph;
use helio_pass_sprite_batch::{SpriteBatchPass, SpriteHandle, SpriteInstance};
use helio_pass_sprite_cull::SpriteCullPass;

mod common;
mod sprite_scene_input;
use sprite_scene_input::SceneInputAdapter;

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run");
}

struct App {
    state: Option<AppState>,
}

impl App {
    fn new() -> Self {
        Self { state: None }
    }
}

struct AppState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,
    graph: RenderGraph,
    /// `RenderGraph::execute()` takes a `&dyn SceneInput` for its API shape
    /// only — `SpriteBatchPass` never reads it. Constructed once and
    /// otherwise ignored, so this demo can skip `helio::Renderer`/SceneDB
    /// entirely.
    scene: SceneInputAdapter,
    dummy_depth_view: wgpu::TextureView,
    start_time: Instant,
    last_stats_log: Instant,
    disc_layer: u32,
    ring_layer: u32,
    ring_handles: Vec<SpriteHandle>,
    stack_handles: Vec<SpriteHandle>,
}

const RING_COUNT: usize = 12;
const STACK_TINTS: [[f32; 4]; 3] = [
    [1.0, 0.2, 0.2, 0.6],
    [0.2, 1.0, 0.2, 0.6],
    [0.2, 0.4, 1.0, 0.6],
];
const DECOY_COUNT: usize = 200;

/// A 32×32 RGBA8 filled disc, straight alpha, on a transparent background.
fn make_disc_atlas() -> Vec<u8> {
    make_shape_atlas(|dist| if dist <= 0.9 { 1.0 } else { 0.0 })
}

/// A 32×32 RGBA8 ring (annulus), straight alpha, on a transparent background.
fn make_ring_atlas() -> Vec<u8> {
    make_shape_atlas(|dist| {
        if (0.55..=0.9).contains(&dist) {
            1.0
        } else {
            0.0
        }
    })
}

fn make_shape_atlas(alpha_at: impl Fn(f32) -> f32) -> Vec<u8> {
    const SIZE: usize = 32;
    let mut pixels = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let cx = (x as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
            let cy = (y as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
            let dist = (cx * cx + cy * cy).sqrt();
            let a = (alpha_at(dist) * 255.0) as u8;
            let i = (y * SIZE + x) * 4;
            pixels[i] = 255;
            pixels[i + 1] = 255;
            pixels[i + 2] = 255;
            pixels[i + 3] = a;
        }
    }
    pixels
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
                        .with_title("Helio – 2D Sprite Batch Demo")
                        .with_inner_size(winit::dpi::LogicalSize::new(960u32, 540u32)),
                )
                .expect("window"),
        );

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            flags: wgpu::InstanceFlags::empty(),
            ..helio_core::wgpu_instance_descriptor()
        });
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .expect("device");
        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: caps.alpha_modes[0],
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
                color_space: wgpu::SurfaceColorSpace::Auto,
            },
        );

        let mut graph = RenderGraph::new(&device, &queue);
        let mut sprite_pass = SpriteBatchPass::new(&device, &queue, format);
        let disc_layer = sprite_pass.add_atlas_layer(&device, &queue, 32, 32, &make_disc_atlas());
        let ring_layer = sprite_pass.add_atlas_layer(&device, &queue, 32, 32, &make_ring_atlas());

        // GPU cull/sort pass, wired in *before* the batch pass it feeds. It
        // binds the pool's instance/alive buffers once at construction, so
        // `reserve()` must fix the pool size before any sprite is inserted.
        // `max_visible` covers the worst case where every sprite is on screen
        // at once (a cheap upper bound: the whole pool).
        let pool_size = RING_COUNT + STACK_TINTS.len() + DECOY_COUNT;
        sprite_pass.reserve(&device, pool_size);
        let mut sprite_cull = SpriteCullPass::new(
            &device,
            &queue,
            sprite_pass.instances_buffer(),
            sprite_pass.alive_buffer(),
            pool_size as u32,
            pool_size as u32,
        );
        // Default camera (center [0,0], half-extent derived from the render
        // target size) — so the cull pass's view rect must track the window.
        sprite_cull.set_view_rect(
            [0.0, 0.0],
            [size.width as f32 * 0.5, size.height as f32 * 0.5],
        );
        sprite_pass.use_gpu_culling(
            sprite_cull.draw_order_buf.clone(),
            sprite_cull.indirect_buf.clone(),
        );

        // Ring + stack get stable handles because `RedrawRequested` animates
        // them via `update_sprite` every frame. The 200 decoys are inserted
        // once and never touched again — after frame one their instance
        // bytes are never re-uploaded, only re-considered by the GPU cull
        // pass's per-frame dispatch.
        let ring_handles: Vec<SpriteHandle> = (0..RING_COUNT)
            .map(|i| {
                let layer = if i % 2 == 0 { disc_layer } else { ring_layer };
                sprite_pass.insert_sprite(
                    SpriteInstance::new([0.0, 0.0], [48.0, 48.0]).with_atlas_layer(layer),
                )
            })
            .collect();
        let stack_handles: Vec<SpriteHandle> = STACK_TINTS
            .iter()
            .enumerate()
            .map(|(i, tint)| {
                let offset = i as f32 * 20.0 - 20.0;
                sprite_pass.insert_sprite(
                    SpriteInstance::new([offset, 0.0], [64.0, 64.0])
                        .with_depth(i as f32)
                        .with_color(*tint)
                        .with_atlas_layer(disc_layer),
                )
            })
            .collect();
        for i in 0..DECOY_COUNT {
            let t = i as f32 / DECOY_COUNT as f32;
            let pos = [5000.0 + t * 100.0, 5000.0 + t * 100.0];
            sprite_pass
                .insert_sprite(SpriteInstance::new(pos, [48.0, 48.0]).with_atlas_layer(disc_layer));
        }

        graph.add_pass(Box::new(sprite_cull));
        graph.add_pass(Box::new(sprite_pass));
        graph.lock(size.width.max(1), size.height.max(1));

        let scene = SceneInputAdapter::new(device.clone(), queue.clone());
        let dummy_depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Dummy Depth (unused by 2D passes)"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let dummy_depth_view = dummy_depth.create_view(&wgpu::TextureViewDescriptor::default());

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            graph,
            scene,
            dummy_depth_view,
            start_time: Instant::now(),
            last_stats_log: Instant::now(),
            disc_layer,
            ring_layer,
            ring_handles,
            stack_handles,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
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
                state.graph.set_render_size(s.width, s.height);
                // The default camera derives its view from the render target
                // size, so the cull pass's view rect must track it too.
                state
                    .graph
                    .find_pass_mut::<SpriteCullPass>()
                    .expect("sprite cull pass missing from graph")
                    .set_view_rect([0.0, 0.0], [s.width as f32 * 0.5, s.height as f32 * 0.5]);
            }
            WindowEvent::RedrawRequested => {
                let time = state.start_time.elapsed().as_secs_f32();

                let disc_layer = state.disc_layer;
                let ring_layer = state.ring_layer;
                let sprite = state
                    .graph
                    .find_pass_mut::<SpriteBatchPass>()
                    .expect("sprite batch pass missing from graph");

                // Ring of spinning sprites, alternating atlas layers — proves
                // the texture-array atlas (both shapes come from one bind
                // group / one draw call). Every handle's data genuinely
                // changes every frame here (that's the point of a spinning
                // ring), so this still uploads ~all of it — the decoys below
                // are what show the delta-upload win.
                for (i, &handle) in state.ring_handles.iter().enumerate() {
                    let t = i as f32 / RING_COUNT as f32;
                    let angle = t * std::f32::consts::TAU + time * 0.5;
                    let radius = 160.0;
                    let pos = [angle.cos() * radius, angle.sin() * radius];
                    let hue = (t + time * 0.1).fract();
                    let layer = if i % 2 == 0 { disc_layer } else { ring_layer };
                    sprite.update_sprite(
                        handle,
                        SpriteInstance::new(pos, [48.0, 48.0])
                            .with_rotation(time * 2.0 + t * std::f32::consts::TAU)
                            .with_depth(t)
                            .with_color(hsv_to_rgb(hue, 0.7, 1.0))
                            .with_atlas_layer(layer),
                    );
                }

                // Overlapping, semi-transparent discs at the center with
                // distinct depths — proves the radix-sorted back-to-front
                // draw order: drawn in the wrong order, the blended colors
                // at the overlap wouldn't match this depth sequence (red
                // furthest back, blue nearest). Static, but still re-set
                // every frame here just to exercise `update_sprite`'s dirty
                // tracking on a value that *doesn't* actually change (the
                // byte range still gets marked dirty and re-uploaded — dirty
                // tracking is a "don't upload if nothing calls update", not
                // a value-equality diff).
                for (i, (&handle, tint)) in state
                    .stack_handles
                    .iter()
                    .zip(STACK_TINTS.iter())
                    .enumerate()
                {
                    let offset = i as f32 * 20.0 - 20.0;
                    sprite.update_sprite(
                        handle,
                        SpriteInstance::new([offset, 0.0], [64.0, 64.0])
                            .with_depth(i as f32)
                            .with_color(*tint)
                            .with_atlas_layer(disc_layer),
                    );
                }

                if state.last_stats_log.elapsed().as_secs_f32() >= 1.0 {
                    state.last_stats_log = std::time::Instant::now();
                    // The visible/drawn count lives on the GPU now — the CPU
                    // only knows how many sprites are in the pool.
                    log::info!("[sprite_demo] pushed={}", sprite.sprite_count());
                }

                let output = match state.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(texture)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
                    _ => {
                        state.window.request_redraw();
                        return;
                    }
                };
                let view = output.texture.create_view(&Default::default());
                if let Err(e) = state
                    .graph
                    .execute(&state.scene, &view, &state.dummy_depth_view)
                {
                    log::error!("graph execute error: {e:?}");
                }
                state.queue.present(output);
                state.window.request_redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(s) = &self.state {
            s.window.request_redraw();
        }
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 4] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match (i as i32).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r, g, b, 1.0]
}
