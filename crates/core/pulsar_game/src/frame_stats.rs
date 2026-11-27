//! Frame-time breakdown of the game window, for reading from a log.
//!
//! Set `PULSAR_FRAME_STATS` to the seconds between reports (any other value:
//! 2) and the game logs, at info level under target `pulsar_game::frame_stats`,
//! once per report interval (`FRAME_STATS`): the frame rate, the mean and worst
//! time of each CPU stage of [`crate::windowed_app`]'s frame, the render graph's
//! CPU time by pass, and the renderer's GPU time (timestamp queries, a frame or
//! two late) by pass; every fifth report also wgpu's allocator report by
//! allocation name (`GPU_MEMORY`). Off, it costs one branch per stage. A
//! diagnostic for reading a run from its log; the editor's profiler records the
//! same timings while it records.
//!
//! The stages, in frame order: `scene_lock` (waiting for the shared world's write
//! lock, which the tick thread holds while it ticks), `step_scene`, `camera`,
//! `prepare` (scene flush and frame extraction), `record_submit` (render graph
//! recording and the queue submit), `acquire` (the swapchain wait: vsync and the
//! GPU finishing the frame before), `copy_present`, and `between` (from the end
//! of one frame to the start of the next: the event loop and anything it waits
//! for).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The CPU stages of one frame, in order.
pub const STAGES: [&str; 8] = [
    "scene_lock",
    "step_scene",
    "camera",
    "prepare",
    "record_submit",
    "acquire",
    "copy_present",
    "between",
];

pub const SCENE_LOCK: usize = 0;
pub const STEP_SCENE: usize = 1;
pub const CAMERA: usize = 2;
pub const PREPARE: usize = 3;
pub const RECORD_SUBMIT: usize = 4;
pub const ACQUIRE: usize = 5;
pub const COPY_PRESENT: usize = 6;
pub const BETWEEN: usize = 7;

/// The report interval, when `PULSAR_FRAME_STATS` is set.
pub fn interval() -> Option<Duration> {
    static INTERVAL: OnceLock<Option<Duration>> = OnceLock::new();
    *INTERVAL.get_or_init(|| {
        let raw = std::env::var("PULSAR_FRAME_STATS").ok()?;
        let seconds = raw.trim().parse::<f64>().ok().filter(|s| *s > 0.0).unwrap_or(2.0);
        Some(Duration::from_secs_f64(seconds.clamp(0.25, 60.0)))
    })
}

#[derive(Default)]
struct Stage {
    total: Duration,
    worst: Duration,
}

/// One window's accumulating report.
pub struct FrameStats {
    interval: Option<Duration>,
    since: Instant,
    frames: u32,
    stages: [Stage; STAGES.len()],
    stage_start: Option<Instant>,
    frame_end: Option<Instant>,
    gpu_total: f64,
    gpu_samples: u32,
    gpu_generation: Option<u64>,
    gpu_passes: Vec<(&'static str, f64, u32)>,
    cpu_generation: u64,
    cpu_total: f64,
    cpu_samples: u32,
    cpu_passes: Vec<(&'static str, f64, u32)>,
    reports: u32,
}

impl FrameStats {
    pub fn new() -> Self {
        Self {
            interval: interval(),
            since: Instant::now(),
            frames: 0,
            stages: Default::default(),
            stage_start: None,
            frame_end: None,
            gpu_total: 0.0,
            gpu_samples: 0,
            gpu_generation: None,
            gpu_passes: Vec::new(),
            cpu_generation: 0,
            cpu_total: 0.0,
            cpu_samples: 0,
            cpu_passes: Vec::new(),
            reports: 0,
        }
    }

    pub fn enabled(&self) -> bool {
        self.interval.is_some()
    }

    fn add(&mut self, stage: usize, time: Duration) {
        let stage = &mut self.stages[stage];
        stage.total += time;
        stage.worst = stage.worst.max(time);
    }

    /// A frame begins: the time since the last one ended is `between`.
    pub fn begin(&mut self) {
        if !self.enabled() {
            return;
        }
        let now = Instant::now();
        if let Some(end) = self.frame_end.take() {
            self.add(BETWEEN, now - end);
        }
        self.stage_start = Some(now);
    }

    /// The stage that started at the last mark ended now.
    pub fn mark(&mut self, stage: usize) {
        if !self.enabled() {
            return;
        }
        let now = Instant::now();
        if let Some(start) = self.stage_start.replace(now) {
            self.add(stage, now - start);
        }
    }

    /// Move the mark to now without counting the time since the last one (a
    /// stretch measured with [`Self::record`] instead).
    pub fn restart(&mut self) {
        if self.enabled() {
            self.stage_start = Some(Instant::now());
        }
    }

    /// Time a stage measured elsewhere (it does not move the mark).
    pub fn record(&mut self, stage: usize, time: Duration) {
        if self.enabled() {
            self.add(stage, time);
        }
    }

    /// The frame ended; read the renderer's newest GPU timings and log when the
    /// interval is over. True every fifth report: time for [`log_gpu_memory`].
    pub fn end(&mut self, size: [u32; 2], timing: &helio::RenderTimingSnapshot) -> bool {
        let Some(interval) = self.interval else {
            return false;
        };
        let now = Instant::now();
        self.frame_end = Some(now);
        self.frames += 1;
        if self.gpu_generation != timing.gpu_frame_index {
            self.gpu_generation = timing.gpu_frame_index;
            if let Some(total) = timing.total_gpu_ms {
                self.gpu_total += f64::from(total);
                self.gpu_samples += 1;
                for pass in &timing.passes {
                    let Some(ms) = pass.gpu_ms else { continue };
                    accumulate(&mut self.gpu_passes, pass.name, ms);
                }
            }
        }
        if self.cpu_generation != timing.generation {
            self.cpu_generation = timing.generation;
            if let Some(total) = timing.total_cpu_ms {
                self.cpu_total += f64::from(total);
                self.cpu_samples += 1;
                for pass in &timing.passes {
                    if let Some(ms) = pass.cpu_ms {
                        accumulate(&mut self.cpu_passes, pass.name, ms);
                    }
                }
            }
        }
        let elapsed = now - self.since;
        if elapsed < interval {
            return false;
        }
        let frames = self.frames.max(1);
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mut cpu = String::new();
        for (name, stage) in STAGES.iter().zip(&self.stages) {
            cpu.push_str(&format!(
                " {name}={:.2}/{:.2}",
                ms(stage.total) / f64::from(frames),
                ms(stage.worst)
            ));
        }
        let gpu_ms = self.gpu_total / f64::from(self.gpu_samples.max(1));
        let graph_cpu_ms = self.cpu_total / f64::from(self.cpu_samples.max(1));
        let passes = top(&mut self.gpu_passes, 48);
        let cpu_passes = top(&mut self.cpu_passes, 12);
        tracing::info!(
            target: "pulsar_game::frame_stats",
            "FRAME_STATS {}x{} fps={:.1} frame_ms={:.2} gpu_ms={gpu_ms:.2} (samples {}) | cpu mean/worst ms:{cpu} | graph cpu ms {graph_cpu_ms:.2}:{cpu_passes} | gpu passes ms:{passes}",
            size[0],
            size[1],
            f64::from(self.frames) / elapsed.as_secs_f64(),
            ms(elapsed) / f64::from(frames),
            self.gpu_samples,
        );
        let reports = self.reports + 1;
        *self = Self::new();
        self.interval = Some(interval);
        self.frame_end = Some(now);
        self.reports = reports;
        reports % 5 == 1
    }
}

impl Default for FrameStats {
    fn default() -> Self {
        Self::new()
    }
}

fn accumulate(passes: &mut Vec<(&'static str, f64, u32)>, name: &'static str, ms: f32) {
    match passes.iter_mut().find(|(known, ..)| *known == name) {
        Some(entry) => {
            entry.1 += f64::from(ms);
            entry.2 += 1;
        }
        None => passes.push((name, f64::from(ms), 1)),
    }
}

/// The `count` most expensive passes by mean, as ` name=ms` pairs.
fn top(passes: &mut [(&'static str, f64, u32)], count: usize) -> String {
    passes.sort_by(|a, b| (b.1 / f64::from(b.2)).total_cmp(&(a.1 / f64::from(a.2))));
    let mut out = String::new();
    for (name, total, samples) in passes.iter().take(count) {
        out.push_str(&format!(" {name}={:.3}", total / f64::from(*samples)));
    }
    out
}

/// Log the engine devices' video memory by owner and by owner and label, the
/// largest first, with the share their owners report live, and every
/// allocation made outside an owner scope by label
/// (`engine_backend::services::vram`). Walks every allocation: callers run it
/// every few seconds, not per frame.
pub fn log_gpu_memory() {
    use engine_backend::services::vram::{VramSnapshot, UNSCOPED};
    let snapshot = engine_backend::services::vram::snapshot(true);
    let uses = snapshot.name_use();
    let mut by_owner: Vec<(&str, u64)> = Vec::new();
    let mut by_label: Vec<(&str, &str, u64, u32)> = Vec::new();
    let allocations = snapshot
        .devices
        .iter()
        .flat_map(|device| &device.blocks)
        .flat_map(|block| &block.allocations);
    for allocation in allocations {
        let owner = VramSnapshot::owner_of(allocation, &uses);
        match by_owner.iter_mut().find(|(known, _)| *known == owner) {
            Some(entry) => entry.1 += allocation.size,
            None => by_owner.push((owner, allocation.size)),
        }
        let label = allocation.name.as_str();
        match by_label.iter_mut().find(|(o, l, ..)| *o == owner && *l == label) {
            Some(entry) => {
                entry.2 += allocation.size;
                entry.3 += 1;
            }
            None => by_label.push((owner, label, allocation.size, 1)),
        }
    }
    by_owner.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    by_label.sort_by_key(|entry| std::cmp::Reverse(entry.2));
    let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
    let owners: String = by_owner
        .iter()
        .map(|(owner, bytes)| format!(" [{owner} {:.1}]", mb(*bytes)))
        .collect();
    let mut top = String::new();
    for (owner, label, bytes, count) in by_label.iter().take(40) {
        let used = uses
            .get(label)
            .and_then(|known| known.used_fraction)
            .map_or(String::new(), |fraction| format!(" {:.0}% used", fraction * 100.0));
        top.push_str(&format!(" [{owner}: {label} x{count} {:.1}{used}]", mb(*bytes)));
    }
    let unscoped: String = by_label
        .iter()
        .filter(|(owner, ..)| *owner == UNSCOPED)
        .map(|(_, label, bytes, count)| format!(" [{label} x{count} {:.2}]", mb(*bytes)))
        .collect();
    tracing::info!(
        target: "pulsar_game::frame_stats",
        "GPU_MEMORY allocated_mb={:.0} reserved_mb={:.0} devices={} | by owner (MB):{owners} | largest (MB):{top} | unscoped (MB):{unscoped}",
        mb(snapshot.allocated()),
        mb(snapshot.reserved()),
        snapshot.devices.len(),
    );
}
