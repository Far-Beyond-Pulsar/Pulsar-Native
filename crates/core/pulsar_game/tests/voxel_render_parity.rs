//! A played voxel world renders as the editor shows it: the example planet
//! (`assets/examples/voxel_planet.level`) drawn by the game's renderer (the
//! standalone window's and the PIE viewport's `SceneRenderer`) and by the
//! editor viewport's (`HelioRenderer`, as `voxel_editor_harness` drives it)
//! from the level's camera, both settled, differ by no more than temporal
//! noise. Before they shared one construction the game drew no terrain.
//!
//! Both run with the animation clock stopped and no projection jitter, so
//! what differs is the construction. The frames go to
//! `<temp>/pulsar_voxel_parity/` (the frames and their differences from the
//! editor's, x8). The noise floor is the editor rendering it twice.
//! Needs a GPU adapter; skips without one.

use std::path::Path;
use std::sync::Arc;

use engine_backend::scene::{RuntimeLevel, SharedScene};
use engine_backend::subsystems::render::{
    DeviceOwner, EditorCameraState, FrameProjection, FrameView, HelioRenderer, SceneRenderer,
    SceneRendererOptions,
};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
// The editor presents to an sRGB surface: the graph's final pass encodes into it.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Frames either renderer may take to settle (the world builds, chunks
/// stream in, the temporal history converges).
const SETTLE_LIMIT: usize = 6_000;
/// Frames the game renders after its voxel worlds finished, as the editor
/// keeps rendering after the last activity (`TEMPORAL_SETTLING_FRAMES`).
const TEMPORAL_FRAMES: usize = 120;
/// The animation clock's frame delta, for temporal filters.
const FRAME_DELTA: f32 = 1.0 / 60.0;

fn device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    // No ray queries on software adapters (#840): lavapipe loses the device on them.
    let features = helio::usable_adapter_features(&adapter);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: helio::required_wgpu_features(features),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .ok()?;
    Some((Arc::new(device), Arc::new(queue)))
}

struct Target {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Target {
    fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voxel-parity-target"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self {
            device,
            queue,
            texture,
            view,
        }
    }

    fn wait(&self) {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
    }

    fn read(&self) -> Vec<u8> {
        let row = WIDTH * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (padded * HEIGHT) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(HEIGHT),
                },
            },
            wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.wait();
        let data = buffer
            .slice(..)
            .get_mapped_range()
            .unwrap()
            .chunks_exact(padded as usize)
            .flat_map(|line| line[..row as usize].to_vec())
            .collect();
        buffer.unmap();
        data
    }
}

/// How two frames differ: the share of pixels whose RGB differ by more
/// than 48 in all, the mean difference per channel (0-255) and the PSNR.
struct Diff {
    changed: f64,
    mean: f64,
    psnr: f64,
}

fn diff(a: &[u8], b: &[u8]) -> Diff {
    let pixels = (a.len() / 4) as f64;
    let mut changed = 0usize;
    let (mut sum, mut squares) = (0.0f64, 0.0f64);
    for (a, b) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let d: Vec<f64> = (0..3).map(|i| f64::from(a[i].abs_diff(b[i]))).collect();
        if d.iter().sum::<f64>() > 48.0 {
            changed += 1;
        }
        sum += d.iter().sum::<f64>();
        squares += d.iter().map(|d| d * d).sum::<f64>();
    }
    let mse = squares / (pixels * 3.0);
    Diff {
        changed: changed as f64 / pixels,
        mean: sum / (pixels * 3.0),
        psnr: if mse == 0.0 {
            f64::INFINITY
        } else {
            10.0 * (255.0 * 255.0 / mse).log10()
        },
    }
}

/// Distinct colours, coarsely: a frame of one flat colour (nothing drawn)
/// has one.
fn colours(frame: &[u8]) -> usize {
    let mut seen: Vec<[u8; 3]> = frame
        .chunks_exact(4)
        .map(|p| [p[0] >> 4, p[1] >> 4, p[2] >> 4])
        .collect();
    seen.sort_unstable();
    seen.dedup();
    seen.len()
}

fn save(dir: &Path, name: &str, frame: &[u8]) {
    image::save_buffer(
        dir.join(format!("{name}.png")),
        frame,
        WIDTH,
        HEIGHT,
        image::ColorType::Rgba8,
    )
    .expect("write a frame");
}

fn load_level() -> SharedScene {
    let level = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../assets/examples/voxel_planet.level");
    RuntimeLevel::load(&level).expect("the example planet loads").scene()
}

/// The editor viewport at the level's camera, settled as the editor holds
/// it (`render_frame` idle); its frame and the camera it drew.
fn editor_frame(target: &Target) -> (Vec<u8>, FrameView, usize) {
    let level = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../assets/examples/voxel_planet.level");
    let scene: SharedScene =
        Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()));
    let extras = RuntimeLevel::load_into(&level, &mut scene.write().world)
        .expect("the example planet loads");
    let mut renderer = HelioRenderer::new(scene);
    let camera = extras.editor_camera.expect("the level saves its camera");
    renderer.set_editor_camera_state(EditorCameraState {
        position: camera.position.map(f64::from),
        yaw: camera.yaw,
        pitch: camera.pitch,
    });
    renderer.set_viewport_realtime(false);
    renderer.set_frame_delta_override(Some(FRAME_DELTA));
    renderer.set_camera_jitter_override(Some([0.0, 0.0]));
    let mut frames = 0;
    while renderer
        .render_frame(&target.device, &target.queue, &target.view, WIDTH, HEIGHT, FORMAT)
        .is_some()
    {
        target.wait();
        frames += 1;
        assert!(frames < SETTLE_LIMIT, "the editor never settled");
    }
    let state = renderer.editor_camera_state();
    let eye = glam::DVec3::from_array(state.position);
    let forward = renderer.camera_forward();
    let up = renderer.camera_up();
    let sun = renderer.sun_direction();
    println!(
        "VOXEL PARITY editor: settled in {frames} frames; eye {},{},{} forward {},{},{} up {},{},{} sun {sun:?}",
        eye.x, eye.y, eye.z, forward.x, forward.y, forward.z, up.x, up.y, up.z
    );
    let view = FrameView {
        eye,
        forward,
        up,
        aspect: WIDTH as f32 / HEIGHT as f32,
        // The editor viewport's perspective camera.
        projection: FrameProjection::Perspective {
            fov_y: std::f32::consts::FRAC_PI_4,
            near: 0.1,
            far: 10_000.0,
        },
    };
    (target.read(), view, frames)
}

/// The game's renderer (`owner`'s) on its own copy of the level, as the game
/// loop and the PIE tick drive it, from `view`: settled (its voxel worlds
/// finished, then the temporal history converged), and drawn `frames` frames
/// in all when it settled sooner (the editor's count: per-frame noise
/// patterns follow the frame index).
fn game_frame(
    target: &Target,
    owner: DeviceOwner,
    view: &FrameView,
    editor_frames: usize,
) -> Vec<u8> {
    let scene = load_level();
    let mut renderer = SceneRenderer::new(
        Arc::clone(&target.device),
        Arc::clone(&target.queue),
        &scene,
        WIDTH,
        HEIGHT,
        FORMAT,
        SceneRendererOptions::game(owner, false),
    );
    renderer.renderer.set_frame_clock_delta(Some(0.0));
    renderer.renderer.set_frame_delta_override(Some(FRAME_DELTA));
    renderer.renderer.set_camera_jitter_override(Some([0.0, 0.0]));
    let mut frames = 0;
    let camera_relative = loop {
        let relative = game_tick(target, &scene, &mut renderer, view);
        frames += 1;
        assert!(frames < SETTLE_LIMIT, "the game's voxel worlds never settled");
        // A world's first frames may not have asked for chunks yet.
        if frames > 8 && !renderer.needs_frame() {
            break relative;
        }
    };
    let settled = frames;
    let all = if settled + 30 <= editor_frames {
        editor_frames
    } else {
        settled + TEMPORAL_FRAMES
    };
    while frames < all {
        game_tick(target, &scene, &mut renderer, view);
        frames += 1;
    }
    let diagnostics = renderer.voxel_diagnostics().join(" | ");
    println!(
        "VOXEL PARITY game ({owner:?}): settled in {settled} frames, {frames} in all; camera-relative {camera_relative}; {diagnostics}"
    );
    assert!(camera_relative, "the planet renders camera-relative in the game");
    let resident = diagnostics
        .split(' ')
        .find_map(|word| word.strip_prefix("resident_chunks="))
        .and_then(|n| n.parse::<u64>().ok());
    assert!(
        resident.is_some_and(|n| n > 0),
        "the game's renderer streamed no terrain: {diagnostics}"
    );
    target.read()
}

/// One game frame, as the game loop and the PIE tick render it: bind the
/// loaded assets, step the world, place the camera, draw. Whether the frame
/// was camera-relative.
fn game_tick(
    target: &Target,
    scene: &SharedScene,
    renderer: &mut SceneRenderer,
    view: &FrameView,
) -> bool {
    engine_backend::scene::pump_assets(scene);
    renderer.step_scene(&mut scene.write());
    let prepared = renderer.prepare_frame(view, [WIDTH, HEIGHT]);
    assert!(
        prepared.new_errors.as_ref().is_none_or(Vec::is_empty),
        "voxel errors: {:?}",
        prepared.new_errors
    );
    renderer
        .render(&prepared.camera, &target.view)
        .expect("render");
    target.wait();
    prepared.camera_relative
}

/// Save `frame` as `name` with its difference from `editor` (x8), and print
/// how they differ.
fn report(out: &Path, name: &str, editor: &[u8], frame: &[u8]) -> Diff {
    save(out, name, frame);
    let difference: Vec<u8> = editor
        .chunks_exact(4)
        .zip(frame.chunks_exact(4))
        .flat_map(|(a, b)| {
            let d = |i: usize| a[i].abs_diff(b[i]).saturating_mul(8);
            [d(0), d(1), d(2), 255]
        })
        .collect();
    save(out, &format!("{name}_diff"), &difference);
    let d = diff(editor, frame);
    println!(
        "VOXEL PARITY {name} vs editor: {:.3} % pixels changed, mean {:.3}/255, PSNR {:.1} dB ({})",
        d.changed * 100.0,
        d.mean,
        d.psnr,
        out.display()
    );
    d
}

#[test]
fn a_played_voxel_world_renders_as_the_editor_shows_it() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let target = Target::new(device, queue);
    let out = std::env::temp_dir().join("pulsar_voxel_parity");
    std::fs::create_dir_all(&out).unwrap();

    let (editor, view, frames) = editor_frame(&target);
    save(&out, "editor", &editor);
    assert!(colours(&editor) > 32, "the editor drew nothing");
    // The noise floor: the editor again, the same construction run twice.
    let (again, _, _) = editor_frame(&target);
    let floor = report(&out, "editor_again", &editor, &again);
    let mut games = Vec::new();
    for (name, owner) in [
        ("standalone", DeviceOwner::Renderer),
        ("embedded", DeviceOwner::Host),
    ] {
        let game = game_frame(&target, owner, &view, frames);
        games.push((name, report(&out, name, &editor, &game)));
    }
    for (name, d) in games {
        assert!(
            d.changed <= floor.changed + 0.005 && d.mean <= floor.mean + 0.5,
            "the {name} game does not render the planet as the editor does"
        );
    }
}
