//! A played voxel world renders as the editor shows it: the example planet
//! (`assets/examples/voxel_planet.level`, an Earth-size planet) drawn by the
//! game's renderer (the standalone window's and the PIE viewport's
//! `SceneRenderer`) and by the editor viewport's (`HelioRenderer`, as
//! `voxel_editor_harness` drives it), both settled, differ by no more than
//! temporal noise. Before they shared one construction the game drew no
//! terrain.
//!
//! The game is placed as it places itself: its free camera starts at the
//! editor's pose, follows the voxel ground's local vertical and hands the
//! renderer a `RenderCamera` (f64 eye). Two poses: the level's camera, and
//! one 2 m above land far from the pole, where the local vertical is far
//! from +Y and an f32 eye would be off by decimetres. Settled still frames
//! of the game do not move.
//!
//! Both run with the animation clock stopped and no projection jitter, so
//! what differs is the construction. The frames go to
//! `<temp>/pulsar_voxel_parity/` (the frames and their differences from the
//! editor's, x8). The noise floor is the editor rendering it twice.
//! Needs a GPU adapter; skips without one.

use std::path::Path;
use std::sync::Arc;

use engine_backend::scene::{RuntimeLevel, SharedScene};
use engine_backend::subsystems::render::helio_renderer::fly_camera::yaw_pitch;
use engine_backend::subsystems::render::{
    DeviceOwner, EditorCameraState, FrameView, HelioRenderer, SceneRenderer, SceneRendererOptions,
};
use glam::{DVec3, Vec3};
use pulsar_game::freecam::FreeCam;

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
/// The example planet's radius.
const PLANET_RADIUS: f64 = 6_371_000.0;
/// How high above the ground the near-ground pose is.
const EYE_HEIGHT: f64 = 2.0;

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


fn level_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets/examples/voxel_planet.level")
}

fn load_level() -> SharedScene {
    RuntimeLevel::load(&level_path())
        .expect("the example planet loads")
        .scene()
}

/// An editor viewport over its own copy of the level, as the harness drives
/// it: clock stopped, no jitter, rendering only while it is not idle.
fn editor() -> HelioRenderer {
    let scene: SharedScene =
        Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()));
    RuntimeLevel::load_into(&level_path(), &mut scene.write().world)
        .expect("the example planet loads");
    let mut renderer = HelioRenderer::new(scene);
    renderer.set_viewport_realtime(false);
    renderer.set_frame_delta_override(Some(FRAME_DELTA));
    renderer.set_camera_jitter_override(Some([0.0, 0.0]));
    renderer
}

/// The level's saved camera.
fn level_pose() -> EditorCameraState {
    let extras = RuntimeLevel::load_into(
        &level_path(),
        &mut engine_backend::scene::new_scene().world,
    )
    .expect("the example planet loads");
    let camera = extras.editor_camera.expect("the level saves its camera");
    EditorCameraState {
        position: camera.position,
        yaw: camera.yaw,
        pitch: camera.pitch,
    }
}

/// A pose `EYE_HEIGHT` above land in daylight, 40 degrees from the equator
/// (the local vertical is about 50 degrees from +Y), looking along the
/// horizon and a little down: of spots every 15 degrees of longitude, the
/// first with the sun high over it that is land well above sea level.
fn near_ground_pose(target: &Target) -> EditorCameraState {
    let mut renderer = editor();
    renderer.set_editor_camera_state(level_pose());
    let mut sun = None;
    for _ in 0..240 {
        renderer.render_frame(&target.device, &target.queue, &target.view, WIDTH, HEIGHT, FORMAT);
        target.wait();
        sun = renderer.sun_direction();
        if sun.is_some() {
            break;
        }
    }
    let sun = Vec3::from_array(sun.expect("the planet has a sun")).as_dvec3();
    let latitude = 40.0f64.to_radians();
    let mut spots: Vec<DVec3> = (0..24)
        .map(|step| {
            let longitude = f64::from(step * 15).to_radians();
            DVec3::new(
                latitude.cos() * longitude.cos(),
                latitude.sin(),
                latitude.cos() * longitude.sin(),
            )
        })
        .filter(|up| up.dot(sun) > 0.5)
        .collect();
    spots.sort_by(|a, b| b.dot(sun).total_cmp(&a.dot(sun)));
    for up in spots {
        let probe = up * (PLANET_RADIUS + 9_000.0);
        renderer.set_editor_camera_state(EditorCameraState {
            position: probe.to_array(),
            yaw: 0.0,
            pitch: 0.0,
        });
        // The ground height is the generator's: known once the world is
        // built, at any detail.
        let mut altitude = None;
        for _ in 0..240 {
            renderer.render_frame(&target.device, &target.queue, &target.view, WIDTH, HEIGHT, FORMAT);
            target.wait();
            altitude = renderer.voxel_altitude();
            if altitude.is_some() {
                break;
            }
        }
        let altitude = altitude.expect("the planet reports the camera's altitude");
        let ground = probe.length() - altitude;
        println!(
            "VOXEL PARITY probe up {:?} (sun {:.0} degrees up): ground {:.1} m above the radius",
            up.to_array(),
            90.0 - up.angle_between(sun).to_degrees(),
            ground - PLANET_RADIUS
        );
        if ground < PLANET_RADIUS + 30.0 {
            continue;
        }
        // Off the f32 grid (0.5 m out here), as an eye a player walked to is.
        let eye = up * (ground + EYE_HEIGHT) + up.any_orthonormal_vector() * 0.3;
        let east = DVec3::Y.cross(up).normalize().as_vec3();
        let forward = (east * 0.15f32.cos() - up.as_vec3() * 0.15f32.sin()).normalize();
        let (yaw, pitch) = yaw_pitch(forward);
        return EditorCameraState {
            position: eye.to_array(),
            yaw,
            pitch,
        };
    }
    panic!("no land among the probed spots");
}

/// What the editor drew from a pose, settled.
struct EditorShot {
    frame: Vec<u8>,
    frames: usize,
    eye: DVec3,
    forward: Vec3,
    up: Vec3,
    altitude: Option<f64>,
}

/// The editor viewport at `pose`, settled as the editor holds it
/// (`render_frame` idle).
fn editor_frame(target: &Target, pose: EditorCameraState) -> EditorShot {
    let mut renderer = editor();
    renderer.set_editor_camera_state(pose);
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
    let shot = EditorShot {
        frame: target.read(),
        frames,
        eye: DVec3::from_array(state.position),
        forward: renderer.camera_forward(),
        up: renderer.camera_up(),
        altitude: renderer.voxel_altitude(),
    };
    println!(
        "VOXEL PARITY editor: settled in {frames} frames; eye {:?} forward {:?} up {:?} altitude {:?} sun {:?}",
        shot.eye.to_array(),
        shot.forward.to_array(),
        shot.up.to_array(),
        shot.altitude,
        renderer.sun_direction()
    );
    assert_eq!(
        shot.eye,
        DVec3::from_array(pose.position),
        "the editor camera stayed where it was put"
    );
    shot
}

/// What the game drew, settled, and how much its last two still frames
/// differ.
struct GameShot {
    frame: Vec<u8>,
    still: Diff,
}

/// The game's renderer (`owner`'s) on its own copy of the level, as the game
/// loop and the PIE tick drive it, its free camera started at the editor's
/// pose: settled (its voxel worlds finished, then the temporal history
/// converged), and drawn `editor.frames` frames in all when it settled
/// sooner (per-frame noise patterns follow the frame index). Its view
/// matches the editor's.
fn game_frame(
    target: &Target,
    owner: DeviceOwner,
    pose: EditorCameraState,
    editor: &EditorShot,
) -> GameShot {
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
    let mut camera =
        FreeCam::default().place(DVec3::from_array(pose.position), pose.yaw, pose.pitch);
    let mut frames = 0;
    let view = loop {
        let tick = game_tick(target, &scene, &mut renderer, &mut camera);
        frames += 1;
        assert!(frames < SETTLE_LIMIT, "the game's voxel worlds never settled");
        // A world's first frames may not have asked for chunks yet.
        if frames > 8 && !renderer.needs_frame() {
            break tick;
        }
    };
    let settled = frames;
    let all = if settled + 30 <= editor.frames {
        editor.frames
    } else {
        settled + TEMPORAL_FRAMES
    };
    let mut previous = None;
    while frames < all {
        game_tick(target, &scene, &mut renderer, &mut camera);
        frames += 1;
        if frames + 1 == all {
            previous = Some(target.read());
        }
    }
    let frame = target.read();
    let diagnostics = renderer.voxel_diagnostics().join(" | ");
    println!(
        "VOXEL PARITY game ({owner:?}): settled in {settled} frames, {frames} in all; eye {:?} forward {:?} up {:?}; {diagnostics}",
        view.eye.to_array(),
        view.forward.to_array(),
        view.up.to_array(),
    );
    assert_eq!(view.eye, editor.eye, "the game's eye is the editor's, in f64");
    assert!(
        view.forward.abs_diff_eq(editor.forward, 1e-5) && view.up.abs_diff_eq(editor.up, 1e-5),
        "the game's free camera looks as the editor's does"
    );
    let resident = diagnostics
        .split(' ')
        .find_map(|word| word.strip_prefix("resident_chunks="))
        .and_then(|n| n.parse::<u64>().ok());
    assert!(
        resident.is_some_and(|n| n > 0),
        "the game's renderer streamed no terrain: {diagnostics}"
    );
    let still = diff(previous.as_deref().expect("two settled frames"), &frame);
    GameShot { frame, still }
}

/// One game frame, as the game loop and the PIE tick render it: bind the
/// loaded assets, step the world, place the free camera on the ground under
/// it, draw. The frame's view.
fn game_tick(
    target: &Target,
    scene: &SharedScene,
    renderer: &mut SceneRenderer,
    camera: &mut FreeCam,
) -> FrameView {
    engine_backend::scene::pump_assets(scene);
    renderer.step_scene(&mut scene.write());
    camera.follow_ground(renderer.voxel_ground(camera.position()));
    let view = camera
        .to_render_camera()
        .frame_view(WIDTH as f32 / HEIGHT as f32);
    let prepared = renderer.prepare_frame(&view, [WIDTH, HEIGHT]);
    assert!(
        prepared.new_errors.as_ref().is_none_or(Vec::is_empty),
        "voxel errors: {:?}",
        prepared.new_errors
    );
    renderer
        .render(&prepared.camera, &target.view)
        .expect("render");
    target.wait();
    view
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

/// The editor twice (the noise floor) and both games from `pose`, named
/// `name`; every game within the floor and still when settled.
fn check_pose(target: &Target, out: &Path, name: &str, pose: EditorCameraState) {
    let editor = editor_frame(target, pose);
    save(out, &format!("{name}_editor"), &editor.frame);
    assert!(colours(&editor.frame) > 32, "the editor drew nothing at {name}");
    // The noise floor: the editor again, the same construction run twice.
    let again = editor_frame(target, pose);
    let floor = report(out, &format!("{name}_editor_again"), &editor.frame, &again.frame);
    for (runtime, owner) in [
        ("standalone", DeviceOwner::Renderer),
        ("embedded", DeviceOwner::Host),
    ] {
        let game = game_frame(target, owner, pose, &editor);
        let d = report(out, &format!("{name}_{runtime}"), &editor.frame, &game.frame);
        println!(
            "VOXEL PARITY {name}_{runtime} still frames: {:.3} % pixels changed, mean {:.3}/255, PSNR {:.1} dB",
            game.still.changed * 100.0,
            game.still.mean,
            game.still.psnr
        );
        assert!(
            d.changed <= floor.changed + 0.005 && d.mean <= floor.mean + 0.5,
            "the {runtime} game does not render the planet as the editor does at {name}"
        );
        assert!(
            game.still.changed <= 0.01 && game.still.mean <= 0.5,
            "the {runtime} game's still frames move at {name}"
        );
    }
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

    check_pose(&target, &out, "level", level_pose());

    let pose = near_ground_pose(&target);
    let eye = DVec3::from_array(pose.position);
    println!(
        "VOXEL PARITY near-ground pose: eye {:?}, {:.1} degrees from +Y; an f32 eye is {:.3} m off",
        pose.position,
        eye.normalize().angle_between(DVec3::Y).to_degrees(),
        (eye.as_vec3().as_dvec3() - eye).length()
    );
    check_pose(&target, &out, "near_ground", pose);
}
