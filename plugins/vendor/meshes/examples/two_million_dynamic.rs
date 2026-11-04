//! Two million objects transitioning from static to dynamic.
//!
//! The run begins with all objects tagged `Static`, collects a baseline for
//! `HELIO_INITIAL_FRAMES`, then promotes `HELIO_DYNAMIC_STEP` objects to `Dynamic`
//! at each checkpoint. Each checkpoint waits `HELIO_STABILIZE_SECONDS` before recording
//! the next measurement window. The final checkpoint is always 2,000,000
//! dynamic objects. Results are written to a SQLite database suitable for
//! querying by an AI agent.
//! Dynamic sets both the CPU mobility component and the GPU movable flag; it
//! does not animate transforms or deform vertices every frame. All frames are
//! logged; frame_context.is_sample selects baseline/checkpoint measurements.

mod v3_demo_common;

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    Movability, Renderer, RendererConfig,
};
use pulsar_scenedb::{Entity, SceneDb};
use rusqlite::{params, Connection};
use std::{
    collections::HashSet,
    env,
    sync::Arc,
    time::{Duration, Instant},
};
use v3_demo_common::{
    build_default_renderer_with_context, cube_mesh, directional_light, flush_scene_db,
    make_material, new_scene_db_with_gpu_mirror, set_object_movability, spawn_light,
    spawn_material, spawn_mesh, spawn_object_with_movability,
};
use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

const OBJECTS: usize = 2_000_000;

#[derive(Clone, Copy)]
struct Options {
    initial_frames: u64,
    step: usize,
    stabilize: Duration,
    sample_frames: u64,
    out: &'static str,
}

fn options() -> Options {
    let value = |name: &str| env::var(name).ok();
    Options {
        initial_frames: value("HELIO_INITIAL_FRAMES")
            .and_then(|v| v.parse().ok())
            .unwrap_or(300)
            .max(1),
        step: value("HELIO_DYNAMIC_STEP")
            .and_then(|v| v.parse().ok())
            .unwrap_or(100_000)
            .clamp(1, OBJECTS),
        stabilize: Duration::from_secs(
            value("HELIO_STABILIZE_SECONDS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
        ),
        sample_frames: value("HELIO_SAMPLE_FRAMES")
            .and_then(|v| v.parse().ok())
            .unwrap_or(120)
            .max(1),
        out: Box::leak(
            value("HELIO_PERF_DB")
                .unwrap_or_else(|| "two_million_dynamic.sqlite".into())
                .into_boxed_str(),
        ),
    }
}

struct Db {
    conn: Connection,
    run: i64,
}
impl Db {
    fn open(path: &str, opts: Options) -> Self {
        let conn = Connection::open(path).expect("open performance database");
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
            CREATE TABLE IF NOT EXISTS runs (id INTEGER PRIMARY KEY, started_at TEXT NOT NULL,
              object_count INTEGER NOT NULL, initial_frames INTEGER NOT NULL, step INTEGER NOT NULL,
              stabilize_seconds REAL NOT NULL, sample_frames INTEGER NOT NULL, completed INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS checkpoints (id INTEGER PRIMARY KEY, run_id INTEGER NOT NULL,
              sequence INTEGER NOT NULL, dynamic_objects INTEGER NOT NULL, static_objects INTEGER NOT NULL,
              phase TEXT NOT NULL, stabilized_for_ms REAL NOT NULL, started_at_frame INTEGER NOT NULL,
              FOREIGN KEY(run_id) REFERENCES runs(id));
            CREATE TABLE IF NOT EXISTS frames (id INTEGER PRIMARY KEY, run_id INTEGER NOT NULL,
              checkpoint_id INTEGER NOT NULL, frame INTEGER NOT NULL, frame_ms REAL NOT NULL,
              fps REAL NOT NULL, dynamic_objects INTEGER NOT NULL, static_objects INTEGER NOT NULL,
              FOREIGN KEY(run_id) REFERENCES runs(id), FOREIGN KEY(checkpoint_id) REFERENCES checkpoints(id));
            CREATE TABLE IF NOT EXISTS profiler_frames (id INTEGER PRIMARY KEY, frame_id INTEGER NOT NULL,
              generation INTEGER NOT NULL, cpu_frame_index INTEGER NOT NULL, gpu_frame_index INTEGER,
              gpu_lag_frames INTEGER, gpu_availability TEXT NOT NULL, total_cpu_ms REAL, total_gpu_ms REAL,
              readback_drops INTEGER NOT NULL, query_overflows INTEGER NOT NULL,
              FOREIGN KEY(frame_id) REFERENCES frames(id));
            CREATE TABLE IF NOT EXISTS pass_timings (id INTEGER PRIMARY KEY, profiler_frame_id INTEGER NOT NULL,
              pass_name TEXT NOT NULL, cpu_ms REAL, gpu_ms REAL,
              FOREIGN KEY(profiler_frame_id) REFERENCES profiler_frames(id));
            CREATE TABLE IF NOT EXISTS frame_stages (id INTEGER PRIMARY KEY, frame_id INTEGER NOT NULL,
              stage_name TEXT NOT NULL, duration_ms REAL NOT NULL,
              FOREIGN KEY(frame_id) REFERENCES frames(id));
            CREATE TABLE IF NOT EXISTS scene_uploads (id INTEGER PRIMARY KEY, frame_id INTEGER NOT NULL,
              ranges INTEGER NOT NULL, bytes INTEGER NOT NULL,
              FOREIGN KEY(frame_id) REFERENCES frames(id));
            CREATE TABLE IF NOT EXISTS startup_uploads (id INTEGER PRIMARY KEY, run_id INTEGER NOT NULL,
              ranges INTEGER NOT NULL, bytes INTEGER NOT NULL,
              FOREIGN KEY(run_id) REFERENCES runs(id));
            CREATE TABLE IF NOT EXISTS frame_context (frame_id INTEGER PRIMARY KEY,
              is_sample INTEGER NOT NULL, profiler_epoch INTEGER NOT NULL,
              width INTEGER NOT NULL, height INTEGER NOT NULL,
              FOREIGN KEY(frame_id) REFERENCES frames(id));
            CREATE TABLE IF NOT EXISTS run_metadata (run_id INTEGER NOT NULL,
              key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(run_id,key));
            CREATE INDEX IF NOT EXISTS frames_run_checkpoint ON frames(run_id, checkpoint_id, frame);
            CREATE INDEX IF NOT EXISTS profiler_frames_frame ON profiler_frames(frame_id);
            CREATE INDEX IF NOT EXISTS profiler_frames_source ON profiler_frames(cpu_frame_index);
            CREATE INDEX IF NOT EXISTS pass_timings_frame ON pass_timings(profiler_frame_id);
            CREATE INDEX IF NOT EXISTS pass_timings_name ON pass_timings(pass_name);
            CREATE INDEX IF NOT EXISTS frame_stages_frame ON frame_stages(frame_id);
            CREATE INDEX IF NOT EXISTS scene_uploads_frame ON scene_uploads(frame_id);
            CREATE INDEX IF NOT EXISTS checkpoints_run_dynamic ON checkpoints(run_id, dynamic_objects);
            CREATE VIEW IF NOT EXISTS gpu_frame_samples AS
              WITH samples AS (
                SELECT p.*, f.run_id, x.profiler_epoch,
                  row_number() OVER (PARTITION BY f.run_id,x.profiler_epoch,p.gpu_frame_index ORDER BY p.id) AS sample_rank
                FROM profiler_frames p JOIN frames f ON f.id=p.frame_id
                  JOIN frame_context x ON x.frame_id=f.id WHERE p.gpu_frame_index IS NOT NULL)
              SELECT p.id AS profiler_frame_id, p.run_id, p.profiler_epoch, p.gpu_frame_index,
                source.id AS source_frame_id, source.frame AS source_frame,
                source.checkpoint_id, source.dynamic_objects, sx.is_sample,
                p.total_gpu_ms, p.gpu_lag_frames
              FROM samples p JOIN profiler_frames sp ON sp.cpu_frame_index=p.gpu_frame_index
                JOIN frames source ON source.id=sp.frame_id AND source.run_id=p.run_id
                JOIN frame_context sx ON sx.frame_id=source.id AND sx.profiler_epoch=p.profiler_epoch
              WHERE p.sample_rank=1;
            CREATE VIEW IF NOT EXISTS gpu_pass_samples AS
              SELECT g.*, t.pass_name,t.gpu_ms FROM gpu_frame_samples g
                JOIN pass_timings t ON t.profiler_frame_id=g.profiler_frame_id;")
            .expect("create performance schema");
        conn.execute("INSERT INTO runs (started_at, object_count, initial_frames, step, stabilize_seconds, sample_frames) VALUES (datetime('now'), ?, ?, ?, ?, ?)",
            params![OBJECTS as i64, opts.initial_frames as i64, opts.step as i64, opts.stabilize.as_secs_f64(), opts.sample_frames as i64]).unwrap();
        Self {
            run: conn.last_insert_rowid(),
            conn,
        }
    }
    fn checkpoint(
        &self,
        seq: usize,
        dynamic: usize,
        phase: &str,
        stable_ms: f64,
        frame: u64,
    ) -> i64 {
        self.conn.execute("INSERT INTO checkpoints (run_id, sequence, dynamic_objects, static_objects, phase, stabilized_for_ms, started_at_frame) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![self.run, seq as i64, dynamic as i64, (OBJECTS - dynamic) as i64, phase, stable_ms, frame as i64]).unwrap();
        self.conn.last_insert_rowid()
    }
    fn frame(&self, checkpoint: i64, frame: u64, ms: f64, dynamic: usize) -> i64 {
        self.conn.execute("INSERT INTO frames (run_id, checkpoint_id, frame, frame_ms, fps, dynamic_objects, static_objects) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![self.run, checkpoint, frame as i64, ms, 1000.0 / ms.max(0.001), dynamic as i64, (OBJECTS - dynamic) as i64]).unwrap();
        self.conn.last_insert_rowid()
    }
    fn profiler_frame(&self, frame_id: i64, snapshot: &helio_core::RenderTimingSnapshot) {
        self.conn.execute(
            "INSERT INTO profiler_frames (frame_id, generation, cpu_frame_index, gpu_frame_index,
             gpu_lag_frames, gpu_availability, total_cpu_ms, total_gpu_ms, readback_drops, query_overflows)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                frame_id,
                snapshot.generation as i64,
                snapshot.cpu_frame_index as i64,
                snapshot.gpu_frame_index.map(|v| v as i64),
                snapshot.gpu_lag_frames.map(|v| v as i64),
                format!("{:?}", snapshot.gpu_availability),
                snapshot.total_cpu_ms,
                snapshot.total_gpu_ms,
                snapshot.readback_drops as i64,
                snapshot.query_overflows as i64,
            ],
        )
        .unwrap();
        let profiler_frame_id = self.conn.last_insert_rowid();
        for pass in &snapshot.passes {
            self.conn
                .execute(
                    "INSERT INTO pass_timings (profiler_frame_id, pass_name, cpu_ms, gpu_ms)
                 VALUES (?, ?, ?, ?)",
                    params![profiler_frame_id, pass.name, pass.cpu_ms, pass.gpu_ms],
                )
                .unwrap();
        }
    }
    fn stage(&self, frame_id: i64, name: &str, duration_ms: f64) {
        self.conn
            .execute(
                "INSERT INTO frame_stages (frame_id, stage_name, duration_ms) VALUES (?, ?, ?)",
                params![frame_id, name, duration_ms],
            )
            .unwrap();
    }
    fn scene_upload(&self, frame_id: i64, ranges: u64, bytes: u64) {
        self.conn
            .execute(
                "INSERT INTO scene_uploads (frame_id, ranges, bytes) VALUES (?, ?, ?)",
                params![frame_id, ranges as i64, bytes as i64],
            )
            .unwrap();
    }
    fn startup_upload(&self, ranges: u64, bytes: u64) {
        self.conn
            .execute(
                "INSERT INTO startup_uploads (run_id, ranges, bytes) VALUES (?, ?, ?)",
                params![self.run, ranges as i64, bytes as i64],
            )
            .unwrap();
    }
    fn complete(&self) {
        self.conn
            .execute("UPDATE runs SET completed = 1 WHERE id = ?", [self.run])
            .unwrap();
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
    }
}

struct App {
    state: Option<State>,
    opts: Options,
}
struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    format: wgpu::TextureFormat,
    present_mode: wgpu::PresentMode,
    renderer: Renderer,
    scene: SceneDb,
    objects: Vec<Entity>,
    camera: Camera,
    last: Instant,
    frame: u64,
    dynamic: usize,
    stabilize_until: Instant,
    sampling: bool,
    transition_ready: bool,
    finished: bool,
    drain_left: u8,
    profiler_epoch: u64,
    previous_cpu_frame: Option<u64>,
    checkpoint: i64,
    seq: usize,
    sample_left: u64,
    db: Db,
    keys: HashSet<KeyCode>,
    pos: glam::Vec3,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let window = Arc::new(
            el.create_window(
                Window::default_attributes().with_title("Helio - 2M Static to Dynamic"),
            )
            .unwrap(),
        );
        let size = window.inner_size();
        let mut graphics = pollster::block_on(helio::HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            helio::HelioGraphicsConfig {
                vr_support: true,
                application_name: "Two Million Dynamic".into(),
                present_mode: wgpu::PresentMode::Immediate,
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let adapter = graphics.adapter.clone();
        let format = graphics.surface_config.format;
        let present_mode = graphics.surface_config.present_mode;
        let mut scene = new_scene_db_with_gpu_mirror(&device, &queue);
        let renderer = build_default_renderer_with_context(
            &scene,
            &mut graphics,
            RendererConfig::new(size.width, size.height, format),
        );
        let surface = graphics.surface;
        let mat = spawn_material(
            &mut scene.world,
            make_material([0.25, 0.65, 1.0, 1.0], 0.5, 0.1, [0.0; 3], 0.0),
        );
        let mesh = spawn_mesh(&mut scene.world, cube_mesh([0.0; 3], 0.35));
        let side = (OBJECTS as f64).cbrt().ceil() as i32;
        let half = side as f32 * 1.1 * 0.5;
        let timer = Instant::now();
        let mut objects = Vec::with_capacity(OBJECTS);
        for i in 0..OBJECTS {
            let i = i as i32;
            let x = i % side;
            let y = (i / side) % side;
            let z = i / (side * side);
            let p = glam::Vec3::new(
                x as f32 * 1.1 - half,
                y as f32 * 1.1 - half,
                z as f32 * 1.1 - half,
            );
            objects.push(
                spawn_object_with_movability(
                    &mut scene.world,
                    mesh,
                    mat,
                    glam::Mat4::from_translation(p),
                    0.5,
                    Some(Movability::Static),
                )
                .unwrap(),
            );
        }
        println!(
            "Created {OBJECTS} static objects in {:.2}s",
            timer.elapsed().as_secs_f32()
        );
        spawn_light(
            &mut scene.world,
            directional_light([0.5, -0.8, 0.3], [1.0, 0.95, 0.85], 8.0),
        );
        // The initial 2M-row mirror upload is scene setup, not a frame. Drain
        // it before frame 1 so swapchain acquisition cannot charge startup
        // transfer work to the render loop.
        let startup_upload = flush_scene_db(&scene, &queue);
        // write_buffer schedules transfers; poll alone does not submit them.
        queue.submit(std::iter::empty());
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("wait for initial SceneDB GPU upload");
        let opts = self.opts;
        let db = Db::open(opts.out, opts);
        for (key, value) in [
            ("adapter", format!("{:?}", adapter.get_info())),
            ("present_mode", format!("{:?}", present_mode)),
            ("measurement", "v2: frame_ms is present-to-present wall time including DB/event-loop overhead; cpu_tick is work inside tick; GPU indices are local to profiler_epoch; is_sample excludes settling/drain".into()),
            ("workload", "2M cubes; dynamic means GPU movable classification, not per-frame transform or vertex animation".into()),
        ] {
            db.conn.execute("INSERT INTO run_metadata VALUES (?, ?, ?)", params![db.run, key, value]).unwrap();
        }
        if let Some(stats) = startup_upload {
            db.startup_upload(stats.ranges as u64, stats.bytes);
        }
        let checkpoint = db.checkpoint(0, 0, "baseline", 0.0, 0);
        self.state = Some(State {
            window,
            surface,
            device,
            format,
            present_mode,
            renderer,
            scene,
            objects,
            camera: Camera::perspective_look_at(
                glam::Vec3::new(0.0, 80.0, 180.0),
                glam::Vec3::ZERO,
                glam::Vec3::Y,
                0.7,
                size.width as f32 / size.height.max(1) as f32,
                0.1,
                1000.0,
            ),
            last: Instant::now(),
            frame: 0,
            dynamic: 0,
            stabilize_until: Instant::now(),
            sampling: false,
            transition_ready: false,
            finished: false,
            drain_left: 8,
            profiler_epoch: 0,
            previous_cpu_frame: None,
            checkpoint,
            seq: 0,
            sample_left: opts.initial_frames,
            db,
            keys: HashSet::new(),
            pos: glam::Vec3::new(0.0, 80.0, 180.0),
        });
        println!(
            "Transition begins after {} frames; step={} stabilize={}s output={}",
            opts.initial_frames,
            opts.step,
            opts.stabilize.as_secs(),
            opts.out
        );
    }
    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ks,
                        physical_key: PhysicalKey::Code(k),
                        ..
                    },
                ..
            } => {
                if let Some(s) = &mut self.state {
                    if ks == ElementState::Pressed && k == KeyCode::Escape {
                        el.exit();
                    }
                    if ks == ElementState::Pressed {
                        s.keys.insert(k);
                    } else {
                        s.keys.remove(&k);
                    }
                }
            }
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                if let Some(s) = &mut self.state {
                    s.surface.configure(
                        &s.device,
                        &wgpu::SurfaceConfiguration {
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                            format: s.format,
                            width: size.width,
                            height: size.height,
                            present_mode: s.present_mode,
                            alpha_mode: wgpu::CompositeAlphaMode::Auto,
                            view_formats: vec![],
                            desired_maximum_frame_latency: 2,
                            color_space: wgpu::SurfaceColorSpace::Auto,
                        },
                    );
                    s.renderer.set_render_size(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                self.tick();
                if let Some(s) = &self.state {
                    if s.finished && s.drain_left == 0 {
                        s.db.complete();
                        println!("Final measurements written to {}", self.opts.out);
                        el.exit();
                        return;
                    }
                    s.window.request_redraw();
                }
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

impl App {
    fn tick(&mut self) {
        let Some(s) = &mut self.state else {
            return;
        };
        let frame_start = Instant::now();
        let now = Instant::now();
        s.frame += 1;
        let initial_transition = s.dynamic == 0 && s.frame > self.opts.initial_frames;
        if s.dynamic < OBJECTS && (initial_transition || (s.transition_ready && !s.sampling)) {
            let add = self.opts.step.min(OBJECTS - s.dynamic);
            for &e in &s.objects[s.dynamic..s.dynamic + add] {
                set_object_movability(&mut s.scene.world, e, Movability::Dynamic).unwrap();
            }
            s.dynamic += add;
            s.seq += 1;
            s.stabilize_until = now + self.opts.stabilize;
            s.sampling = false;
            s.transition_ready = false;
            s.sample_left = 0;
            s.checkpoint = s.db.checkpoint(
                s.seq,
                s.dynamic,
                if s.dynamic == OBJECTS {
                    "final"
                } else {
                    "transition"
                },
                self.opts.stabilize.as_secs_f64() * 1000.0,
                s.frame,
            );
            println!(
                "checkpoint {}: {}/{} dynamic; stabilizing",
                s.seq, s.dynamic, OBJECTS
            );
        }
        if s.dynamic > 0
            && !s.sampling
            && !s.finished
            && !s.transition_ready
            && now >= s.stabilize_until
        {
            s.sampling = true;
            s.sample_left = self.opts.sample_frames;
            println!(
                "checkpoint {}: stabilization complete; sampling {} frames",
                s.seq, s.sample_left
            );
        }
        let record_frame = if s.dynamic == 0 && s.frame <= self.opts.initial_frames {
            true
        } else if s.sampling {
            s.sample_left -= 1;
            if s.sample_left == 0 {
                s.sampling = false;
                if s.dynamic == OBJECTS {
                    s.finished = true;
                } else {
                    s.transition_ready = true;
                }
            }
            true
        } else {
            false
        };
        let flush_start = Instant::now();
        let upload_stats = flush_scene_db(&s.scene, s.renderer.queue());
        let flush_ms = flush_start.elapsed().as_secs_f64() * 1000.0;
        let size = s.window.inner_size();
        s.camera = Camera::perspective_look_at(
            s.pos,
            glam::Vec3::ZERO,
            glam::Vec3::Y,
            0.7,
            size.width as f32 / size.height.max(1) as f32,
            0.1,
            1000.0,
        );
        // Surface acquisition happens inside Helio after XR frame pacing.
        let acquire_ms = 0.0;
        let render_start = Instant::now();
        s.renderer
            .render_window_frame(&s.camera, &s.surface)
            .expect("benchmark render failed");
        let render_ms = render_start.elapsed().as_secs_f64() * 1000.0;
        let present_start = Instant::now();
        let present_ms = present_start.elapsed().as_secs_f64() * 1000.0;
        {
            let present_end = Instant::now();
            let frame_ms = present_end.duration_since(s.last).as_secs_f64() * 1000.0;
            s.last = present_end;
            // One transaction per frame prevents per-pass SQL commit overhead
            // from dominating the benchmark. Cadence still includes this work.
            s.db.conn.execute_batch("BEGIN;").unwrap();
            let frame_id = s.db.frame(s.checkpoint, s.frame, frame_ms, s.dynamic);
            let snapshot = s.renderer.timing_snapshot().clone();
            if s.previous_cpu_frame
                .is_some_and(|previous| snapshot.cpu_frame_index <= previous)
            {
                s.profiler_epoch += 1;
            }
            s.previous_cpu_frame = Some(snapshot.cpu_frame_index);
            s.db.conn
                .execute(
                    "INSERT INTO frame_context VALUES (?, ?, ?, ?, ?)",
                    params![
                        frame_id,
                        record_frame,
                        s.profiler_epoch as i64,
                        size.width,
                        size.height
                    ],
                )
                .unwrap();
            s.db.profiler_frame(frame_id, &snapshot);
            s.db.stage(
                frame_id,
                "cpu_tick",
                present_end.duration_since(frame_start).as_secs_f64() * 1000.0,
            );
            s.db.stage(frame_id, "scene_flush", flush_ms);
            if let Some(stats) = upload_stats {
                s.db.scene_upload(frame_id, stats.ranges as u64, stats.bytes);
            }
            s.db.stage(frame_id, "surface_acquire", acquire_ms);
            s.db.stage(frame_id, "renderer_render", render_ms);
            s.db.stage(frame_id, "present", present_ms);
            s.db.conn.execute_batch("COMMIT;").unwrap();
        }
        if s.finished {
            s.drain_left = s.drain_left.saturating_sub(1);
        }
    }
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().unwrap();
    let mut app = App {
        state: None,
        opts: options(),
    };
    event_loop.run_app(&mut app).unwrap();
}
