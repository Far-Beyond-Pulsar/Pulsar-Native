//! Sprite batch stress test — thousands of independently-moving, bouncing
//! sprites ("bunnymark"-style), to exercise the GPU cull/sort path
//! (`SpriteCullPass` → `SpriteBatchPass`) and draw throughput at scale. All
//! culling and depth sorting happens on the GPU; the CPU only re-uploads the
//! moved sprites' instance bytes each frame.
//!
//! Every sprite gets a random position/velocity/tint and bounces inside a
//! fixed 1280×720 world-space box (independent of window size, so resizing
//! the window doesn't change the simulation). `depth` is set to world Y, so
//! the sort is doing real (non-trivial, order-changing) work every frame as
//! sprites move past each other — not just re-sorting an already-sorted list.
//!
//! `HELIO_SPRITE_STRESS_COUNT` overrides the sprite count (default 20000).
//! Prints a rolling FPS average, plus the pool size, once a second.

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

/// Half-extent of the bounce area, in world units.
const BOUNDS_HALF: [f32; 2] = [640.0, 360.0];
const SPRITE_SIZE: [f32; 2] = [8.0, 8.0];

/// Small deterministic PRNG (PCG-style LCG) — avoids pulling in `rand` for
/// what's just "scatter N points with random velocities".
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    fn next_f32(&mut self) -> f32 {
        self.next_u32() as f32 / u32::MAX as f32
    }
    fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next_f32() * (hi - lo)
    }
}

struct Sprite {
    pos: [f32; 2],
    vel: [f32; 2],
    color: [f32; 4],
}

fn spawn_sprites(count: usize) -> Vec<Sprite> {
    let mut rng = Rng::new(0xB00B_5711_D00D_5EED);
    (0..count)
        .map(|_| {
            let hue = rng.next_f32();
            Sprite {
                pos: [
                    rng.range_f32(-BOUNDS_HALF[0], BOUNDS_HALF[0]),
                    rng.range_f32(-BOUNDS_HALF[1], BOUNDS_HALF[1]),
                ],
                vel: [rng.range_f32(-120.0, 120.0), rng.range_f32(-120.0, 120.0)],
                color: hsv_to_rgb(hue, 0.75, 1.0),
            }
        })
        .collect()
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
    /// only — `SpriteBatchPass` never reads it.
    scene: SceneInputAdapter,
    dummy_depth_view: wgpu::TextureView,
    dot_layer: u32,

    sprites: Vec<Sprite>,
    sprite_handles: Vec<SpriteHandle>,
    last_frame: Instant,
    fps_frames: u32,
    fps_last_print: Instant,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let count = std::env::var("HELIO_SPRITE_STRESS_COUNT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(20_000);
        eprintln!("[sprite_stress_test] {count} sprites");

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Helio – 2D Sprite Batch Stress Test")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 720u32)),
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
        // Fixed world-space bounds regardless of window size, so the bounce
        // simulation and the visible area agree no matter how the window is
        // resized.
        sprite_pass.set_camera([0.0, 0.0], Some(BOUNDS_HALF));
        sprite_pass.set_clear_color(Some(wgpu::Color {
            r: 0.02,
            g: 0.02,
            b: 0.04,
            a: 1.0,
        }));
        let dot_layer = sprite_pass.add_atlas_layer(&device, &queue, 8, 8, &make_dot_atlas());
        let sprites = spawn_sprites(count);

        // GPU cull/sort pass, wired in *before* the batch pass it feeds. It
        // binds the pool's instance/alive buffers once at construction, so
        // `reserve()` must fix the pool size before any sprite is inserted.
        // `max_visible = count` because every sprite can be on screen at once
        // (the bounce area is exactly the view area).
        sprite_pass.reserve(&device, count);
        let mut sprite_cull = SpriteCullPass::new(
            &device,
            &queue,
            sprite_pass.instances_buffer(),
            sprite_pass.alive_buffer(),
            count as u32,
            count as u32,
        );
        sprite_cull.set_view_rect([0.0, 0.0], BOUNDS_HALF);
        sprite_pass.use_gpu_culling(
            sprite_cull.draw_order_buf.clone(),
            sprite_cull.indirect_buf.clone(),
        );

        let sprite_handles: Vec<SpriteHandle> = sprites
            .iter()
            .map(|s| {
                sprite_pass.insert_sprite(
                    SpriteInstance::new(s.pos, SPRITE_SIZE)
                        // World-Y depth: as sprites cross each other vertically
                        // the correct draw order actually changes frame to
                        // frame, so this exercises a real (non-trivial) sort.
                        .with_depth(s.pos[1])
                        .with_color(s.color)
                        .with_atlas_layer(dot_layer),
                )
            })
            .collect();
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
            dot_layer,
            sprites,
            sprite_handles,
            last_frame: Instant::now(),
            fps_frames: 0,
            fps_last_print: Instant::now(),
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
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - state.last_frame).as_secs_f32().min(0.05);
                state.last_frame = now;

                for s in &mut state.sprites {
                    s.pos[0] += s.vel[0] * dt;
                    s.pos[1] += s.vel[1] * dt;
                    if s.pos[0] < -BOUNDS_HALF[0] || s.pos[0] > BOUNDS_HALF[0] {
                        s.vel[0] = -s.vel[0];
                        s.pos[0] = s.pos[0].clamp(-BOUNDS_HALF[0], BOUNDS_HALF[0]);
                    }
                    if s.pos[1] < -BOUNDS_HALF[1] || s.pos[1] > BOUNDS_HALF[1] {
                        s.vel[1] = -s.vel[1];
                        s.pos[1] = s.pos[1].clamp(-BOUNDS_HALF[1], BOUNDS_HALF[1]);
                    }
                }

                let dot_layer = state.dot_layer;
                let sprite_pass = state
                    .graph
                    .find_pass_mut::<SpriteBatchPass>()
                    .expect("sprite batch pass missing from graph");
                for (s, &handle) in state.sprites.iter().zip(state.sprite_handles.iter()) {
                    sprite_pass.update_sprite(
                        handle,
                        SpriteInstance::new(s.pos, SPRITE_SIZE)
                            // World-Y depth: as sprites cross each other vertically
                            // the correct draw order actually changes frame to
                            // frame, so this exercises a real (non-trivial) sort.
                            .with_depth(s.pos[1])
                            .with_color(s.color)
                            .with_atlas_layer(dot_layer),
                    );
                }

                state.fps_frames += 1;
                if state.fps_last_print.elapsed().as_secs_f32() >= 1.0 {
                    let elapsed = state.fps_last_print.elapsed().as_secs_f32();
                    // The drawn count lives on the GPU now (cull pass) — the
                    // CPU only knows how many sprites are in the pool.
                    log::info!(
                        "[sprite_stress_test] {:.0} fps | pushed={}",
                        state.fps_frames as f32 / elapsed,
                        sprite_pass.sprite_count(),
                    );
                    state.fps_frames = 0;
                    state.fps_last_print = Instant::now();
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

/// A small soft-edged 8×8 dot, straight alpha, on a transparent background.
fn make_dot_atlas() -> Vec<u8> {
    const SIZE: usize = 8;
    let mut pixels = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let cx = (x as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
            let cy = (y as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
            let dist = (cx * cx + cy * cy).sqrt();
            let a = ((1.0 - dist).clamp(0.0, 1.0) * 255.0) as u8;
            let i = (y * SIZE + x) * 4;
            pixels[i] = 255;
            pixels[i + 1] = 255;
            pixels[i + 2] = 255;
            pixels[i + 3] = a;
        }
    }
    pixels
}
