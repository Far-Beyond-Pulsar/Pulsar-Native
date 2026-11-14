//! Performance monitoring data for the viewport's performance overlay.
//!
//! [`FrameSampler`] fills [`LiveStats`] on the overlay's timer from the
//! renderer's stats feed and the window's presented-frame count: frame rates
//! measured between frame timestamps ([`RateMeter`]), so they are exact and
//! move every sample. The overlay's sections then read it at their own pace
//! (see `components/performance_overlay`): the text sections at 10 Hz and the
//! charts at 4 Hz.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use engine_backend::services::gpu_renderer::{RenderTally, RendererStats, RendererStatsFeed};
use gpui::SharedString;

/// How often the text sections (headline, rendering, input) refresh.
pub const TEXT_REFRESH: Duration = Duration::from_millis(100);

/// How often the charts take a sample and redraw.
pub const CHART_REFRESH: Duration = Duration::from_millis(250);

/// Samples a chart keeps. Up to here its history grows; after that it rolls.
pub const HISTORY_CAPACITY: usize = 1000;

/// The most x-axis labels a chart shows.
pub const MAX_LABELS: usize = 10;

// ── Live statistics ─────────────────────────────────────────────────────────

/// What the overlay measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    UiFps,
    RenderFps,
    FrameTimeMs,
    RenderCpuMs,
    GpuMs,
    GpuMemoryMb,
    DrawCalls,
    Vertices,
    InputLatencyMs,
}

impl Metric {
    const COUNT: usize = 9;

    fn index(self) -> usize {
        self as usize
    }
}

/// Running totals for one metric. They only ever grow, so any number of readers
/// can each remember where they last looked and take the mean since then,
/// without resetting anything for the others.
#[derive(Clone, Copy, Default)]
struct Totals {
    sum: f64,
    count: u64,
    last: f64,
}

/// The latest per-frame samples, written by the viewport.
#[derive(Default)]
pub struct LiveStats {
    totals: [Totals; Metric::COUNT],
}

/// One reader's position in [`LiveStats`].
#[derive(Clone, Default)]
pub struct Cursor {
    seen: [Totals; Metric::COUNT],
}

impl LiveStats {
    /// Record one frame's value.
    pub fn record(&mut self, metric: Metric, value: f64) {
        let totals = &mut self.totals[metric.index()];
        totals.sum += value;
        totals.count += 1;
        totals.last = value;
    }

    /// The mean of what was recorded since `cursor` last asked, or the most
    /// recent value if nothing arrived in between (a paused viewport keeps
    /// showing what it last saw). Moves `cursor` forward for this metric only.
    pub fn mean_since(&self, cursor: &mut Cursor, metric: Metric) -> f64 {
        let now = self.totals[metric.index()];
        let before = &mut cursor.seen[metric.index()];
        let arrived = now.count.saturating_sub(before.count);
        let mean = if arrived == 0 {
            now.last
        } else {
            (now.sum - before.sum) / arrived as f64
        };
        *before = now;
        mean
    }

    /// Whether `metric` was ever recorded: a source that reports nothing (no
    /// GPU timestamps, no allocator report) shows as unknown, not as 0.
    pub fn recorded(&self, metric: Metric) -> bool {
        self.totals[metric.index()].count > 0
    }
}

/// A count as the overlays show it: `950`, `12.3k`, `4.56M`, `1.20B`.
pub fn compact_count(n: f64) -> String {
    let n = n.max(0.0);
    match n {
        n if n >= 1e9 => format!("{:.2}B", n / 1e9),
        n if n >= 1e6 => format!("{:.2}M", n / 1e6),
        n if n >= 1e4 => format!("{:.1}k", n / 1e3),
        n => format!("{n:.0}"),
    }
}

/// Shared between the viewport (writer) and the overlay's sections (readers).
pub type SharedStats = Arc<parking_lot::Mutex<LiveStats>>;

// ── Chart history ───────────────────────────────────────────────────────────

/// A fixed-capacity rolling history of one metric.
#[derive(Default)]
pub struct History {
    values: VecDeque<f64>,
}

impl History {
    pub fn push(&mut self, value: f64) {
        if self.values.len() >= HISTORY_CAPACITY {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn last(&self) -> Option<f64> {
        self.values.back().copied()
    }

    /// The history as chart points: each carries its 1-based *position in the
    /// window* as its x value. Positions, not sample numbers, so once the
    /// history is full and rolling the labels stay put while the data scrolls
    /// past them.
    pub fn points(&self, clamp_max: Option<f64>) -> Vec<ChartPoint> {
        self.values
            .iter()
            .enumerate()
            .map(|(i, &v)| ChartPoint {
                position: position_label(i + 1),
                value: clamp_max.map_or(v, |max| v.min(max)),
            })
            .collect()
    }
}

#[derive(Clone)]
pub struct ChartPoint {
    pub position: SharedString,
    pub value: f64,
}

/// `"1"`..`"999"`, then `"1k"`, built once. Chart x values are cloned for every point on
/// every paint; sharing the strings makes that a pointer copy.
fn position_label(position: usize) -> SharedString {
    static LABELS: std::sync::OnceLock<Vec<SharedString>> = std::sync::OnceLock::new();
    let labels = LABELS.get_or_init(|| {
        (1..=HISTORY_CAPACITY)
            .map(|n| {
                SharedString::from(if n == HISTORY_CAPACITY {
                    "1k".to_string()
                } else {
                    n.to_string()
                })
            })
            .collect()
    });
    labels
        .get(position.wrapping_sub(1))
        .cloned()
        .unwrap_or_else(|| SharedString::from(position.to_string()))
}

/// Every how many samples the x axis gets a label, so that at most
/// [`MAX_LABELS`] show.
///
/// The step grows as the history does: every sample while there are 10 or
/// fewer, then 2, 5, 10, 20, 50, and finally 100. A full history of 1000 samples
/// is therefore labelled 100, 200, ... 1000, and stays that way as it rolls.
pub fn label_step(len: usize) -> usize {
    const STEPS: [usize; 7] = [1, 2, 5, 10, 20, 50, 100];
    let needed = len.div_ceil(MAX_LABELS).max(1);
    STEPS
        .into_iter()
        .find(|&step| step >= needed)
        .unwrap_or(100)
}

/// What the viewport's render takes from `GpuRenderer` once per render: the
/// camera input and pointer queue its handlers write to. The overlays'
/// statistics are not here; they come from [`FrameSampler`].
#[derive(Clone, Default)]
pub struct EngineFrameSnapshot {
    pub camera_input: Option<Arc<Mutex<engine_backend::subsystems::render::CameraInput>>>,
    pub pointer_events:
        Option<Arc<Mutex<Vec<engine_backend::subsystems::render::PendingPointerEvent>>>>,
}

/// How an overlay obtains samples for [`LiveStats`] without help from the
/// viewport's render: it is called on the overlay's own timer.
pub type Sampler = std::rc::Rc<dyn Fn(&mut LiveStats)>;

/// How often the overlay samples. A rate is recorded once
/// [`MIN_RATE_WINDOW`] has passed, so this only bounds how late it is.
pub const SAMPLE_REFRESH: Duration = Duration::from_millis(16);

/// The shortest span a frame rate is measured over: a handful of frames, so a
/// rate is exact to the frame and still moves ten times a second.
pub const MIN_RATE_WINDOW: Duration = Duration::from_millis(100);

/// With no new frame for this long, the rate reads 0 (the renderer idles, or
/// the window presents nothing).
pub const STALL_AFTER: Duration = Duration::from_millis(500);

/// The renderer's published statistics, for an overlay's timer.
///
/// The render scheduler holds the renderer's mutex for every frame it draws,
/// so polling the renderer with `try_lock` misses most of the time and the
/// overlay freezes. This takes the renderer's [`RendererStatsFeed`] once (the
/// only `try_lock`, retried on each read until it succeeds) and from then on
/// reads the feed alone.
pub struct RendererStatsSource {
    engine: Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    feed: std::cell::OnceCell<RendererStatsFeed>,
}

impl RendererStatsSource {
    /// Touches nothing: the feed is taken on the first read.
    pub fn new(engine: Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>) -> Self {
        Self {
            engine,
            feed: std::cell::OnceCell::new(),
        }
    }

    fn feed(&self) -> Option<&RendererStatsFeed> {
        if let Some(feed) = self.feed.get() {
            return Some(feed);
        }
        let feed = self.engine.try_lock().ok()?.stats_feed();
        Some(self.feed.get_or_init(|| feed))
    }

    /// The renderer's statistics as of now; `None` until the feed is taken.
    pub fn latest(&self) -> Option<RendererStats> {
        Some(self.feed()?.latest())
    }
}

/// A frame count, when its latest frame happened, and the time spent on all
/// of them: what both the renderer and a window count.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tally {
    pub frames: u64,
    pub last: Option<Instant>,
    pub busy: Duration,
}

impl From<RenderTally> for Tally {
    fn from(tally: RenderTally) -> Self {
        Self {
            frames: tally.frames,
            last: tally.last,
            busy: tally.busy,
        }
    }
}

impl From<gpui::PresentTally> for Tally {
    fn from(tally: gpui::PresentTally) -> Self {
        Self {
            frames: tally.frames,
            last: tally.last,
            busy: Duration::ZERO,
        }
    }
}

/// A rate measured between two tallies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rate {
    /// Frames arrived: per second, the mean time between them, and the mean
    /// time spent on each.
    Running {
        fps: f64,
        interval_ms: f64,
        busy_ms: f64,
    },
    /// None for [`STALL_AFTER`].
    Stalled,
}

/// Measures a [`Rate`] from successive reads of a [`Tally`].
///
/// The rate is the frames between two reads over the time between *their
/// latest frames*, not between the reads: exact to the frame, with no
/// window-boundary jitter. A read less than [`MIN_RATE_WINDOW`] after the
/// last measured one is held back until enough frames span it.
#[derive(Default)]
pub struct RateMeter {
    since: Option<Tally>,
}

impl RateMeter {
    pub fn measure(&mut self, now: Tally, at: Instant) -> Option<Rate> {
        let since = match self.since {
            Some(since) if since.last.is_some() => since,
            // No frame seen yet: start from here.
            _ => {
                self.since = Some(now);
                return None;
            }
        };
        let (Some(last), Some(before)) = (now.last, since.last) else {
            return None;
        };
        let frames = now.frames.saturating_sub(since.frames);
        if frames == 0 {
            return (at.saturating_duration_since(last) >= STALL_AFTER).then_some(Rate::Stalled);
        }
        let span = last.saturating_duration_since(before);
        if span < MIN_RATE_WINDOW {
            return None;
        }
        self.since = Some(now);
        let seconds = span.as_secs_f64();
        Some(Rate::Running {
            fps: frames as f64 / seconds,
            interval_ms: seconds * 1000.0 / frames as f64,
            busy_ms: now.busy.saturating_sub(since.busy).as_secs_f64() * 1000.0
                / frames as f64,
        })
    }
}

/// Fills [`LiveStats`] from the renderer's stats feed and the window's
/// presented frames, on an overlay's timer. Never locks the renderer.
pub struct FrameSampler {
    renderer: RendererStatsSource,
    /// The viewport's window, once the viewport has seen it (empty until its
    /// first frame pump).
    window: std::rc::Rc<std::cell::OnceCell<gpui::PresentedFrames>>,
    render_rate: std::cell::RefCell<RateMeter>,
    ui_rate: std::cell::RefCell<RateMeter>,
}

impl FrameSampler {
    pub fn new(
        engine: Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        window: std::rc::Rc<std::cell::OnceCell<gpui::PresentedFrames>>,
    ) -> Self {
        Self {
            renderer: RendererStatsSource::new(engine),
            window,
            render_rate: Default::default(),
            ui_rate: Default::default(),
        }
    }

    /// Record what changed since the last call.
    pub fn sample(&self, stats: &mut LiveStats) {
        let now = Instant::now();
        if let Some(window) = self.window.get() {
            match self.ui_rate.borrow_mut().measure(window.read().into(), now) {
                Some(Rate::Running { fps, .. }) => stats.record(Metric::UiFps, fps),
                Some(Rate::Stalled) => stats.record(Metric::UiFps, 0.0),
                None => {}
            }
        }
        let Some(renderer) = self.renderer.latest() else {
            return;
        };
        match self
            .render_rate
            .borrow_mut()
            .measure(renderer.tally.into(), now)
        {
            Some(Rate::Running {
                fps,
                interval_ms,
                busy_ms,
            }) => {
                stats.record(Metric::RenderFps, fps);
                stats.record(Metric::FrameTimeMs, interval_ms);
                stats.record(Metric::RenderCpuMs, busy_ms);
            }
            Some(Rate::Stalled) => stats.record(Metric::RenderFps, 0.0),
            None => {}
        }
        if let Some(details) = renderer.details {
            if let Some(gpu_ms) = details.profiler.as_ref().and_then(|p| p.total_gpu_ms) {
                stats.record(Metric::GpuMs, f64::from(gpu_ms));
            }
            if let Some(bytes) = details.gpu_memory_bytes {
                stats.record(Metric::GpuMemoryMb, bytes as f64 / (1024.0 * 1024.0));
            }
            // Helio's named frame stats, totalled over every pass that
            // contributes to them (`helio::stats`).
            if let Some(frame) = &details.frame_stats {
                use helio::stats::{DRAW_CALLS, VERTICES};
                if let Some(n) = frame.get(DRAW_CALLS) {
                    stats.record(Metric::DrawCalls, n as f64);
                }
                if let Some(n) = frame.get(VERTICES) {
                    stats.record(Metric::Vertices, n as f64);
                }
            }
        }
    }
}

impl EngineFrameSnapshot {
    /// Take the camera input and pointer queue from `GpuRenderer`, pushing the
    /// frame-rate-independent camera-input settings (move speed, scroll zoom)
    /// in the same locked pass. Returns `None` if the renderer mutex was busy.
    pub fn gather(
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        move_speed: f32,
        zoom_delta: f32,
    ) -> Option<Self> {
        let engine = gpu_engine.try_lock().ok()?;
        let snapshot = Self {
            camera_input: engine.camera_input(),
            pointer_events: engine.pointer_event_queue(),
        };

        if let Some(cam) = engine.camera_input() {
            if let Ok(mut input) = cam.try_lock() {
                input.move_speed = move_speed;
                input.zoom_delta += zoom_delta;
            }
        }

        Some(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn readers_each_get_the_mean_since_they_last_looked() {
        let mut stats = LiveStats::default();
        let (mut text, mut chart) = (Cursor::default(), Cursor::default());

        for v in [10.0, 20.0, 30.0] {
            stats.record(Metric::FrameTimeMs, v);
        }
        assert_eq!(stats.mean_since(&mut text, Metric::FrameTimeMs), 20.0);

        stats.record(Metric::FrameTimeMs, 50.0);
        // The text reader sees only what arrived after its last look...
        assert_eq!(stats.mean_since(&mut text, Metric::FrameTimeMs), 50.0);
        // ...and the chart, which never looked, still sees everything: one
        // reader does not reset another.
        assert_eq!(stats.mean_since(&mut chart, Metric::FrameTimeMs), 27.5);
    }

    #[::core::prelude::v1::test]
    fn with_nothing_new_the_last_value_stays_on_screen() {
        let mut stats = LiveStats::default();
        let mut cursor = Cursor::default();
        stats.record(Metric::UiFps, 144.0);
        assert_eq!(stats.mean_since(&mut cursor, Metric::UiFps), 144.0);
        assert_eq!(stats.mean_since(&mut cursor, Metric::UiFps), 144.0);
    }

    #[::core::prelude::v1::test]
    fn metrics_are_independent() {
        let mut stats = LiveStats::default();
        let mut cursor = Cursor::default();
        stats.record(Metric::RenderCpuMs, 9.5);
        stats.record(Metric::GpuMs, 5.25);
        assert_eq!(stats.mean_since(&mut cursor, Metric::RenderCpuMs), 9.5);
        assert_eq!(stats.mean_since(&mut cursor, Metric::GpuMs), 5.25);
        assert!(!stats.recorded(Metric::GpuMemoryMb), "never recorded");
    }

    fn tally(frames: u64, last: Instant, busy_ms: u64) -> Tally {
        Tally {
            frames,
            last: Some(last),
            busy: Duration::from_millis(busy_ms),
        }
    }

    #[::core::prelude::v1::test]
    fn a_rate_is_frames_over_the_time_between_their_last_frames() {
        let t0 = Instant::now();
        let mut meter = RateMeter::default();
        assert_eq!(meter.measure(tally(10, t0, 100), t0), None, "first read");
        // 8 frames, the last 125 ms after the first read's last: 64 fps,
        // 15.625 ms apart, 10 ms of CPU each, whenever the read happens.
        let last = t0 + Duration::from_millis(125);
        let read_at = last + Duration::from_millis(7);
        assert_eq!(
            meter.measure(tally(18, last, 180), read_at),
            Some(Rate::Running {
                fps: 64.0,
                interval_ms: 15.625,
                busy_ms: 10.0,
            })
        );
    }

    #[::core::prelude::v1::test]
    fn a_short_window_waits_for_more_frames() {
        let t0 = Instant::now();
        let mut meter = RateMeter::default();
        meter.measure(tally(0, t0, 0), t0);
        let soon = t0 + Duration::from_millis(40);
        assert_eq!(meter.measure(tally(3, soon, 0), soon), None);
        let later = t0 + Duration::from_millis(100);
        assert!(matches!(
            meter.measure(tally(6, later, 0), later),
            Some(Rate::Running { fps, .. }) if (fps - 60.0).abs() < 1e-9
        ));
    }

    #[::core::prelude::v1::test]
    fn no_frames_for_a_while_reads_as_stalled() {
        let t0 = Instant::now();
        let mut meter = RateMeter::default();
        meter.measure(tally(5, t0, 0), t0);
        assert_eq!(
            meter.measure(tally(5, t0, 0), t0 + Duration::from_millis(100)),
            None
        );
        assert_eq!(
            meter.measure(tally(5, t0, 0), t0 + STALL_AFTER),
            Some(Rate::Stalled)
        );
    }

    #[::core::prelude::v1::test]
    fn the_label_step_keeps_at_most_ten_labels_and_ends_at_hundreds() {
        let labels = |len: usize| len / label_step(len);
        for len in 1..=HISTORY_CAPACITY {
            assert!(
                labels(len) <= MAX_LABELS,
                "{len} samples would show {} labels",
                labels(len)
            );
        }
        assert_eq!(label_step(5), 1, "every sample while there are few");
        assert_eq!(label_step(10), 1);
        assert_eq!(label_step(11), 2);
        assert_eq!(label_step(100), 10, "100 samples: 10, 20 ... 100");
        assert_eq!(label_step(101), 20);
        assert_eq!(label_step(500), 50);
        assert_eq!(label_step(501), 100);
        assert_eq!(label_step(1000), 100, "a full history: 100, 200 ... 1000");
    }

    #[::core::prelude::v1::test]
    fn the_step_never_shrinks_as_the_history_grows() {
        let mut previous = 1;
        for len in 1..=HISTORY_CAPACITY {
            let step = label_step(len);
            assert!(step >= previous, "labels would jump back at {len} samples");
            previous = step;
        }
    }

    #[::core::prelude::v1::test]
    fn the_history_grows_then_rolls_with_static_positions() {
        let mut history = History::default();
        for i in 0..HISTORY_CAPACITY {
            history.push(i as f64);
        }
        assert_eq!(history.len(), HISTORY_CAPACITY);
        let before = history.points(None);
        assert_eq!(before.first().unwrap().position.as_ref(), "1");
        assert_eq!(before.last().unwrap().position.as_ref(), "1k");

        // One more sample: the oldest falls off, the labels do not move.
        history.push(5000.0);
        let after = history.points(None);
        assert_eq!(after.len(), HISTORY_CAPACITY);
        assert_eq!(after.first().unwrap().position.as_ref(), "1");
        assert_eq!(after.last().unwrap().position.as_ref(), "1k");
        assert_eq!(after.last().unwrap().value, 5000.0);
        assert_eq!(after.first().unwrap().value, 1.0, "sample 0 rolled off");
    }

    #[::core::prelude::v1::test]
    fn spikes_can_be_capped_for_display() {
        let mut history = History::default();
        history.push(12.0);
        history.push(400.0);
        let points = history.points(Some(50.0));
        assert_eq!(points[0].value, 12.0);
        assert_eq!(points[1].value, 50.0);
        assert_eq!(history.last(), Some(400.0), "the stored value is untouched");
    }
}
