//! The editor's own viewport renderer, driven headless: what the Pulsar
//! editor shows, captured and timed, for voxel terrain work.
//!
//! It loads a level file into a SceneDB scene and renders it with
//! [`HelioRenderer`] exactly as the editor viewport does (same graph,
//! settings, post-process, TSR mode, idle and settling rules, pointer
//! queue), so a capture is what a user sees. A view is captured when the
//! renderer goes idle (`render_frame` returns `None`): residency and TSR
//! have settled and the editor would hold that frame on screen.
//!
//! ```text
//! cargo run --release -p engine_backend --example voxel_editor_harness -- \
//!     <level> <out dir> [width height] [steps...]
//! ```
//!
//! Steps run in order:
//!
//! - `view:<height m>:<pitch deg>[:<yaw deg>]`: the camera that high above
//!   the ground below the level's camera, settled, captured
//!   (`view_<n>.png`).
//! - `fly:<metres>:<seconds>`: fly forward that far at the camera's height,
//!   capturing every second (`fly_<n>_<t>.png`), then settle and capture.
//! - `drag:<op>:<radius m>:<frames>[:<material>[:<tool>]]`: a left-button
//!   drag stroke across the view through the editor's pointer queue (op:
//!   dig, build, paint; tool: stamp, flatten, smooth), then release, settle
//!   and capture; logs per-frame times and the edits the journal gained.
//! - `at:<x>:<y>:<z>`: later views are placed over this direction from the
//!   planet's centre (an editor session's `eye` from its logs).
//! - `down:<metres>`: move the camera that far down the local vertical (into
//!   a pit just dug: no lift to the generated ground), settled, captured.
//! - `profile:<seconds>`: the view held (the camera nudged a millimetre
//!   every other frame, so the editor keeps rendering): the graph's GPU
//!   time, and by pass.
//! - `stress:<ascent|descent|cruise|orbit>:<seconds>`: a flight in real
//!   time (the path follows the wall clock, as the editor's camera does:
//!   a slow frame moves it further) from the camera's ground: from 2 m to
//!   400 km or back, at 300 m/s 40 m over the ground, or in orbit at 400 km
//!   and 7.7 km/s. Every frame's time and the chunk pass's coarse share,
//!   largest cell and level switches go to `stress_<n>.csv`; a frame a
//!   second is captured (`stress_<n>_<s>.png`), then each view again once
//!   settled (`stress_<n>_<s>_ref.png`).
//!
//! Every frame's wall time (CPU and GPU, the queue drained) is written to
//! `frames.csv`, and a summary per step to stdout.

use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer, PendingPointerEvent, VoxelBrushRequest, VoxelBrushTool};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

// The editor presents to an sRGB surface: the graph's final pass encodes into it.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Frames recorded ahead of the GPU, as a game loop keeps it fed (waiting
/// for every frame lets the driver lower its clocks and inflates times).
const FRAMES_IN_FLIGHT: usize = 2;

struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: HelioRenderer,
    scene: engine_backend::scene::SharedScene,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
    out: PathBuf,
    csv: std::fs::File,
    step: String,
    frame: u64,
    /// Submitted frames not yet done on the GPU.
    in_flight: std::collections::VecDeque<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Harness {
    /// One editor frame; its wall time (at most `FRAMES_IN_FLIGHT` frames
    /// ahead of the GPU), or `None` when the editor was idle and rendered
    /// nothing.
    fn frame(&mut self) -> Option<f64> {
        use std::sync::atomic::{AtomicBool, Ordering};
        let started = Instant::now();
        let submitted = self.renderer.render_frame(&self.device, &self.queue, &self.view, self.size[0], self.size[1], FORMAT);
        let done = std::sync::Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        self.queue.on_submitted_work_done(move || flag.store(true, Ordering::Release));
        self.in_flight.push_back(done);
        while self.in_flight.len() > FRAMES_IN_FLIGHT {
            let oldest = self.in_flight.pop_front().unwrap();
            while !oldest.load(Ordering::Acquire) {
                let _ = self.device.poll(wgpu::PollType::Poll);
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
        }
        let ms = started.elapsed().as_secs_f64() * 1e3;
        self.frame += 1;
        writeln!(self.csv, "{},{},{:.3},{}", self.frame, self.step, ms, u8::from(submitted.is_some())).ok();
        submitted.map(|_| ms)
    }

    /// Render until the editor goes idle (at most `limit` frames); the
    /// frame times of the frames it rendered.
    fn settle(&mut self, limit: usize) -> Vec<f64> {
        let mut times = Vec::new();
        for _ in 0..limit {
            match self.frame() {
                Some(ms) => times.push(ms),
                None => return times,
            }
        }
        eprintln!("HARNESS {}: still rendering after {limit} frames", self.step);
        times
    }

    fn capture(&self, name: &str) {
        let [w, h] = self.size;
        let row = (w * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("harness capture"),
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("map capture"));
        self.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let data = buffer.slice(..).get_mapped_range().expect("mapped").to_vec();
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let start = (y * row) as usize;
            pixels.extend_from_slice(&data[start..start + (w * 4) as usize]);
        }
        let path = self.out.join(format!("{name}.png"));
        image::save_buffer(&path, &pixels, w, h, image::ColorType::Rgba8).expect("write capture");
        eprintln!("HARNESS captured {}", path.display());
    }

    fn camera(&self) -> EditorCameraState {
        self.renderer.editor_camera_state()
    }

    /// Place the camera `height` above the ground below `direction` (from
    /// the planet centre), looking at `pitch` / `yaw` in the local frame
    /// there (up the local vertical, as the editor's camera frame). The
    /// renderer reports the camera's altitude over the ground.
    fn place(&mut self, direction: glam::DVec3, height: f64, pitch: f32, yaw: f32) {
        // The camera state's yaw and pitch are world angles.
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let local = glam::Vec3::new(sy * cp, sp, -cy * cp);
        let forward = glam::Quat::from_rotation_arc(glam::Vec3::Y, direction.as_vec3().normalize()) * local;
        let (yaw, pitch) = (forward.x.atan2(-forward.z), forward.y.clamp(-1.0, 1.0).asin());
        let mailbox = self.renderer.editor_mailbox();
        let mut radius = self.camera().position.into_iter().map(|v| v * v).sum::<f64>().sqrt();
        for _ in 0..4 {
            let position = (direction * radius).to_array();
            mailbox.queue_camera(EditorCameraState { position, yaw, pitch });
            self.frame();
            self.frame();
            let Some(altitude) = self.renderer.voxel_altitude().filter(|a| a.is_finite()) else { continue };
            radius += height - altitude;
            if (height - altitude).abs() < 0.05 {
                break;
            }
        }
        mailbox.queue_camera(EditorCameraState { position: (direction * radius).to_array(), yaw, pitch });
    }

    fn report(&self, label: &str, times: &[f64]) {
        if times.is_empty() {
            println!("HARNESS {label}: no frames");
            return;
        }
        let mut sorted = times.to_vec();
        sorted.sort_by(f64::total_cmp);
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        println!(
            "HARNESS {label}: {} frames, mean {:.1} ms, p50 {:.1}, p95 {:.1}, max {:.1}",
            times.len(),
            times.iter().sum::<f64>() / times.len() as f64,
            at(0.5),
            at(0.95),
            at(1.0)
        );
    }

    /// The chunk pass's `name=value` from its diagnostics (NaN: none).
    fn voxel_stat(&self, name: &str) -> f64 {
        let key = format!(" {name}=");
        self.renderer
            .voxel_diagnostics()
            .iter()
            .find_map(|line| {
                let at = line.find(&key)? + key.len();
                line[at..].split(' ').next()?.parse().ok()
            })
            .unwrap_or(f64::NAN)
    }

    /// The editor camera at `position` looking along `forward` (world).
    fn look(&mut self, position: glam::DVec3, forward: glam::DVec3) {
        let f = forward.normalize().as_vec3();
        let (yaw, pitch) = (f.x.atan2(-f.z), f.y.clamp(-1.0, 1.0).asin());
        self.renderer.editor_mailbox().queue_camera(EditorCameraState { position: position.to_array(), yaw, pitch });
    }

    fn edits(&self) -> usize {
        let scene = self.scene.read();
        scene
            .world
            .query::<&helio_component::VoxelTerrainComponent>()
            .map(|(_, terrain)| terrain.edits.len())
            .sum()
    }
}

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).expect("a GPU adapter");
    // No ray queries on software adapters (#840): lavapipe loses the device on them.
    let features = helio::usable_adapter_features(&adapter);
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: helio::required_wgpu_features(features),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .expect("a GPU device")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let level = PathBuf::from(args.first().expect("<level> <out dir> [width height] [steps...]"));
    let out = PathBuf::from(args.get(1).expect("<out dir>"));
    std::fs::create_dir_all(&out).expect("create the output directory");
    let numeric = |i: usize| args.get(i).and_then(|v| v.parse::<u32>().ok());
    let (size, first_step) = match (numeric(2), numeric(3)) {
        (Some(w), Some(h)) => ([w, h], 4),
        _ => ([1196, 729], 2),
    };
    let steps = &args[first_step.min(args.len())..];

    // Engine logs (VOXEL_FRAME_PHASES, VOXEL_ACTIVITY, ...) to stderr;
    // RUST_LOG filters them (warnings by default).
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .with_writer(std::io::stderr)
        .init();
    let (device, queue) = device();
    let scene: engine_backend::scene::SharedScene =
        std::sync::Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()));
    let extras = {
        let mut scene = scene.write();
        engine_backend::scene::RuntimeLevel::load_into(Path::new(&level), &mut scene.world).expect("load the level")
    };
    let mut renderer = HelioRenderer::new(scene.clone());
    if let Some(camera) = extras.editor_camera {
        renderer.set_editor_camera_state(EditorCameraState { position: camera.position.map(f64::from), yaw: camera.yaw, pitch: camera.pitch });
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("harness viewport"),
        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let csv = std::fs::File::create(out.join("frames.csv")).expect("frames.csv");
    let mut h = Harness { device, queue, renderer, scene, texture, view, size, out, csv, step: "start".into(), frame: 0, in_flight: Default::default() };
    writeln!(h.csv, "frame,step,ms,rendered").ok();

    // Start: the level's own camera, as the editor opens it.
    let started = Instant::now();
    let times = h.settle(20_000);
    h.report(&format!("start ({:.1} s)", started.elapsed().as_secs_f64()), &times);
    h.capture("start");
    let home = h.camera();
    let mut direction = glam::DVec3::from_array(home.position).normalize();

    for (n, step) in steps.iter().enumerate() {
        let parts: Vec<&str> = step.split(':').collect();
        h.step = format!("{n}_{}", parts[0]);
        let number = |i: usize, default: f64| parts.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(default);
        match parts[0] {
            "view" => {
                let (height, pitch, yaw) = (number(1, 2.0), number(2, -10.0) as f32, number(3, home.yaw.to_degrees() as f64) as f32);
                h.place(direction, height, pitch.to_radians(), yaw.to_radians());
                let times = h.settle(20_000);
                h.report(&format!("view {height} m pitch {pitch}"), &times);
                h.capture(&format!("view_{n}"));
            }
            "fly" => {
                let (metres, seconds) = (number(1, 200.0), number(2, 4.0));
                let camera = h.camera();
                let frames = (seconds * 60.0) as usize;
                let start = glam::DVec3::from_array(camera.position);
                let forward = h.renderer.camera_forward().as_dvec3();
                let up = start.normalize();
                let ahead = (forward - up * forward.dot(up)).normalize();
                let mailbox = h.renderer.editor_mailbox();
                let mut times = Vec::new();
                for f in 0..frames {
                    let t = f as f64 / frames as f64;
                    let position = start + ahead * metres * t;
                    let position = position.normalize() * start.length();
                    mailbox.queue_camera(EditorCameraState { position: position.to_array(), ..camera });
                    if let Some(ms) = h.frame() {
                        times.push(ms);
                    }
                    if f % 60 == 30 {
                        h.capture(&format!("fly_{n}_{}", f / 60));
                    }
                }
                h.report(&format!("fly {metres} m in {seconds} s"), &times);
                let times = h.settle(20_000);
                h.report("fly settle", &times);
                h.capture(&format!("fly_{n}_end"));
            }
            "drag" => {
                let op = match parts.get(1).copied().unwrap_or("paint") {
                    "dig" => helio_voxel_data::VoxelBrushOp::Remove,
                    "build" => helio_voxel_data::VoxelBrushOp::Add,
                    _ => helio_voxel_data::VoxelBrushOp::Paint,
                };
                let (radius, frames, material) = (number(2, 1.0) as f32, number(3, 60.0) as usize, number(4, 3.0) as u32);
                let tool = match parts.get(5).copied() {
                    Some("flatten") => VoxelBrushTool::Flatten,
                    Some("smooth") => VoxelBrushTool::Smooth,
                    _ => VoxelBrushTool::Stamp,
                };
                let request = VoxelBrushRequest { op, shape: helio_voxel_data::VoxelBrushShape::Sphere, radius, material, single_block: false, level: Default::default(), tool };
                let before = h.edits();
                let queue = h.renderer.pending_pointer_events.clone();
                let mut times = Vec::new();
                for f in 0..frames {
                    // Across the lower half of the view, as a hand would.
                    let t = f as f32 / frames.max(1) as f32;
                    let (x, y) = (0.2 + 0.6 * t, 0.7 + 0.08 * (t * 9.0).sin());
                    queue.lock().expect("pointer queue").push(PendingPointerEvent::VoxelBrush { norm_x: x, norm_y: y, request, start: f == 0 });
                    if let Some(ms) = h.frame() {
                        times.push(ms);
                    }
                    // HARNESS_DRAG_FRAMES=1: every frame of the stroke (flicker).
                    if std::env::var_os("HARNESS_DRAG_FRAMES").is_some() {
                        h.capture(&format!("drag_{n}_{f:03}"));
                    }
                }
                queue.lock().expect("pointer queue").push(PendingPointerEvent::LeftRelease);
                h.report(&format!("drag {:?} {:?} r {radius} m", op, tool), &times);
                let times = h.settle(20_000);
                h.report("drag settle", &times);
                println!("HARNESS drag: journal {} -> {} edits", before, h.edits());
                h.capture(&format!("drag_{n}"));
            }
            "at" => {
                direction = glam::DVec3::new(number(1, 0.0), number(2, 1.0), number(3, 0.0)).normalize();
                println!("HARNESS at {direction:?}");
            }
            "profile" => {
                let seconds = number(1, 5.0);
                let camera = h.camera();
                let mailbox = h.renderer.editor_mailbox();
                let mut passes: std::collections::BTreeMap<&'static str, (f64, u32)> = Default::default();
                let mut totals = Vec::new();
                let started = Instant::now();
                let mut k = 0u32;
                while started.elapsed().as_secs_f64() < seconds {
                    let mut nudged = camera;
                    nudged.position[1] += f64::from(k % 2) * 1e-3;
                    k += 1;
                    mailbox.queue_camera(nudged);
                    h.frame();
                    if let Some((total, by)) = h.renderer.gpu_timings() {
                        totals.push(f64::from(total));
                        for (name, ms) in by {
                            let entry = passes.entry(name).or_default();
                            entry.0 += f64::from(ms);
                            entry.1 += 1;
                        }
                    }
                }
                totals.sort_by(f64::total_cmp);
                let mut by_pass: Vec<(&str, f64)> = passes.iter().map(|(name, (sum, n))| (*name, sum / f64::from((*n).max(1)))).collect();
                by_pass.sort_by(|a, b| b.1.total_cmp(&a.1));
                println!(
                    "HARNESS profile: graph gpu p50 {:.2} ms; by pass: {}",
                    totals.get(totals.len() / 2).copied().unwrap_or(f64::NAN),
                    by_pass.iter().filter(|p| p.1 >= 0.05).map(|(n, ms)| format!("{n} {ms:.2}")).collect::<Vec<_>>().join(", ")
                );
            }
            "stress" => {
                let mode = parts.get(1).copied().unwrap_or("descent").to_string();
                let seconds = number(2, 20.0);
                let camera = h.camera();
                let start = glam::DVec3::from_array(camera.position);
                let up = start.normalize();
                let forward = h.renderer.camera_forward().as_dvec3();
                let ahead = (forward - up * forward.dot(up)).normalize();
                let ground = start.length() - h.renderer.voxel_altitude().unwrap_or(2.0);
                let path = |t: f64| -> (glam::DVec3, glam::DVec3) {
                    let u = (t / seconds).clamp(0.0, 1.0);
                    let (altitude, along, pitch_down) = match mode.as_str() {
                        "cruise" => (40.0, 300.0 * t, 0.15),
                        "orbit" => (400_000.0, 7_700.0 * t, 0.8),
                        m => {
                            let altitude = 2.0 * 200_000.0f64.powf(if m == "ascent" { u } else { 1.0 - u });
                            (altitude, altitude * 0.3, 0.6)
                        }
                    };
                    // Along a great circle through the start.
                    let angle = along / ground;
                    let local_up = up * angle.cos() + ahead * angle.sin();
                    let local_ahead = ahead * angle.cos() - up * angle.sin();
                    (local_up * (ground + altitude), local_ahead - local_up * pitch_down)
                };
                // From the path's start, settled (a descent starts from a
                // settled orbit, not a jump from the ground).
                let (position, look) = path(0.0);
                h.look(position, look);
                h.settle(3000);
                let mut csv = String::from("t,altitude_m,ms,coarse,worst_px,switch_px\n");
                let (mut times, mut coarse, mut worst, mut switches) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                let mut samples = Vec::new();
                // The render thread's milliseconds by phase, over the flight.
                let mut phases: std::collections::BTreeMap<&'static str, Vec<f64>> = Default::default();
                let started = Instant::now();
                let mut captured = false;
                loop {
                    let t = started.elapsed().as_secs_f64();
                    if t > seconds {
                        break;
                    }
                    let (position, look) = path(t);
                    h.look(position, look);
                    let Some(ms) = h.frame() else { continue };
                    for &(name, phase_ms) in h.renderer.last_frame_phases() {
                        phases.entry(name).or_default().push(f64::from(phase_ms));
                    }
                    let (c, w, sw) = (h.voxel_stat("coarse"), h.voxel_stat("worst_px"), h.voxel_stat("switch_px"));
                    csv += &format!("{t:.4},{:.1},{ms:.2},{c:.4},{w:.3},{sw:.3}\n", position.length() - ground);
                    // A frame after a capture waited for its readback.
                    if !captured {
                        times.push(ms);
                    }
                    coarse.push(c);
                    worst.push(w);
                    if sw > 0.0 {
                        switches.push(sw);
                    }
                    captured = t >= samples.len() as f64;
                    if captured {
                        h.capture(&format!("stress_{n}_{:02}", samples.len()));
                        samples.push((position, look));
                    }
                }
                std::fs::write(h.out.join(format!("stress_{n}.csv")), csv).expect("stress csv");
                h.report(&format!("stress {mode} {seconds} s"), &times);
                let sorted = |v: &[f64]| {
                    let mut v: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
                    v.sort_by(f64::total_cmp);
                    v
                };
                let (w, sw) = (sorted(&worst), sorted(&switches));
                let at = |v: &[f64], q: f64| v.get(((v.len().max(1) - 1) as f64 * q).round() as usize).copied().unwrap_or(f64::NAN);
                println!(
                    "HARNESS stress {mode}: frames over 33 ms {} | coarse mean {:.4} | worst px p50 {:.2} p95 {:.2} max {:.2} | switches {}, px p95 {:.2} max {:.2}, over 1.5 px {}, over 3 px {}",
                    times.iter().filter(|&&ms| ms > 33.3).count(),
                    coarse.iter().filter(|c| c.is_finite()).sum::<f64>() / coarse.len().max(1) as f64,
                    at(&w, 0.5),
                    at(&w, 0.95),
                    at(&w, 1.0),
                    sw.len(),
                    at(&sw, 0.95),
                    at(&sw, 1.0),
                    sw.iter().filter(|&&x| x > 1.5).count(),
                    sw.iter().filter(|&&x| x > 3.0).count()
                );
                let mut by_phase: Vec<(&str, f64, f64)> = phases
                    .iter_mut()
                    .map(|(name, v)| {
                        v.sort_by(f64::total_cmp);
                        (*name, v.iter().sum::<f64>() / v.len().max(1) as f64, v[((v.len().max(1) - 1) as f64 * 0.95) as usize])
                    })
                    .collect();
                by_phase.sort_by(|a, b| b.1.total_cmp(&a.1));
                println!(
                    "HARNESS stress {mode} phases (mean/p95 ms): {}",
                    by_phase.iter().filter(|p| p.1 >= 0.1).map(|(n, mean, p95)| format!("{n} {mean:.2}/{p95:.2}")).collect::<Vec<_>>().join(", ")
                );
                for (k, (position, look)) in samples.iter().enumerate() {
                    h.look(*position, *look);
                    h.settle(20_000);
                    h.capture(&format!("stress_{n}_{k:02}_ref"));
                }
            }
            "down" => {
                let camera = h.camera();
                let position = glam::DVec3::from_array(camera.position);
                let position = position - position.normalize() * number(1, 5.0);
                h.renderer.editor_mailbox().queue_camera(EditorCameraState { position: position.to_array(), ..camera });
                let times = h.settle(20_000);
                h.report(&format!("down {} m, altitude {:?}", number(1, 5.0), h.renderer.voxel_altitude()), &times);
                h.capture(&format!("down_{n}"));
            }
            other => panic!("unknown step {other}"),
        }
    }
}
