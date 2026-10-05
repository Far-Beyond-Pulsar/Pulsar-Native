//! Performance monitoring data for the viewport's performance overlay.
//!
//! The viewport writes one sample per frame into [`LiveStats`]. The overlay's
//! sections then read it at their own pace (see `components/performance_overlay`):
//! the text sections at 10 Hz and the charts at 4 Hz. Nothing here is read, copied
//! or rebuilt per frame by the overlay.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    DrawCalls,
    Vertices,
    MemoryMb,
    InputLatencyMs,
}

impl Metric {
    const COUNT: usize = 7;

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
            .map(|n| SharedString::from(if n == HISTORY_CAPACITY { "1k".to_string() } else { n.to_string() }))
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

/// Per-frame copy of everything the UI thread reads out of `GpuRenderer`,
/// gathered with one `try_lock` acquisition so the viewport render path
/// contends with the Helio render thread's blocking lock once instead of
/// several times per frame.
#[derive(Clone, Default)]
pub struct EngineFrameSnapshot {
    pub ui_fps: f64,
    pub helio_fps: f64,
    pub render_fps: f64,
    pub memory_mb: f64,
    pub draw_calls: f64,
    pub vertices: f64,
    pub frame_time_ms: f64,
    pub camera_input: Option<Arc<Mutex<engine_backend::subsystems::render::CameraInput>>>,
    pub pointer_events:
        Option<Arc<Mutex<Vec<engine_backend::subsystems::render::PendingPointerEvent>>>>,
}

/// How an overlay obtains samples for [`LiveStats`] without help from the
/// viewport's render: it is called on the overlay's own timer.
pub type Sampler = std::rc::Rc<dyn Fn(&mut LiveStats)>;

/// How often the overlay samples. Independent of how often the viewport
/// renders, which is a few times a second at most.
pub const SAMPLE_REFRESH: Duration = Duration::from_millis(16);

impl EngineFrameSnapshot {
    /// The renderer's statistics alone, with none of the camera side effects of
    /// [`Self::gather`] (which consumes the scroll-zoom delta, so only the
    /// viewport's render may call it). `None` if the renderer is busy.
    pub fn read_stats(
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    ) -> Option<Self> {
        let engine = gpu_engine.try_lock().ok()?;
        let (memory_mb, draw_calls, vertices, frame_time_ms) = match engine.get_render_metrics() {
            Some(m) => (
                m.memory_usage_mb as f64,
                m.draw_calls as f64,
                m.vertices_drawn as f64,
                m.frame_time_ms as f64,
            ),
            None => (0.0, 0.0, 0.0, 0.0),
        };
        Some(Self {
            ui_fps: engine.get_fps() as f64,
            helio_fps: engine.get_helio_fps() as f64,
            render_fps: engine.get_render_fps() as f64,
            memory_mb,
            draw_calls,
            vertices,
            frame_time_ms,
            camera_input: None,
            pointer_events: None,
        })
    }

    /// Record this snapshot into `stats`.
    pub fn record_into(&self, stats: &mut LiveStats) {
        // The renderer's metric stands in when the UI-side frame count is not
        // available yet.
        let ui_fps = if self.ui_fps > 0.0 { self.ui_fps } else { self.helio_fps };
        stats.record(Metric::UiFps, ui_fps);
        stats.record(Metric::RenderFps, self.render_fps);
        stats.record(Metric::FrameTimeMs, self.frame_time_ms);
        stats.record(Metric::DrawCalls, self.draw_calls);
        stats.record(Metric::Vertices, self.vertices);
        stats.record(Metric::MemoryMb, self.memory_mb);
    }

    /// Gather stats from `GpuRenderer`, pushing the frame-rate-independent
    /// camera-input settings (move speed, scroll zoom) in the same locked
    /// pass. Returns `None` if the renderer mutex was busy; callers skip
    /// their stat updates that frame.
    pub fn gather(
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        move_speed: f32,
        zoom_delta: f32,
    ) -> Option<Self> {
        let engine = gpu_engine.try_lock().ok()?;
        let metrics_opt = engine.get_render_metrics();
        let (memory_mb, draw_calls, vertices, frame_time_ms) = match metrics_opt {
            Some(ref m) => (
                m.memory_usage_mb as f64,
                m.draw_calls as f64,
                m.vertices_drawn as f64,
                m.frame_time_ms as f64,
            ),
            None => (0.0, 0.0, 0.0, 0.0),
        };

        let snapshot = Self {
            ui_fps: engine.get_fps() as f64,
            helio_fps: engine.get_helio_fps() as f64,
            render_fps: engine.get_render_fps() as f64,
            memory_mb,
            draw_calls,
            vertices,
            frame_time_ms,
            camera_input: engine.camera_input(),
            pointer_events: engine.pointer_event_queue(),
        };

        if let Some(cam) = engine.camera_input() {
            if let Ok(mut input) = cam.try_lock() {
                input.move_speed = move_speed;
                input.zoom_delta = zoom_delta;
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
        stats.record(Metric::DrawCalls, 100.0);
        stats.record(Metric::Vertices, 5000.0);
        assert_eq!(stats.mean_since(&mut cursor, Metric::DrawCalls), 100.0);
        assert_eq!(stats.mean_since(&mut cursor, Metric::Vertices), 5000.0);
        assert_eq!(stats.mean_since(&mut cursor, Metric::MemoryMb), 0.0, "never recorded");
    }

    #[::core::prelude::v1::test]
    fn the_label_step_keeps_at_most_ten_labels_and_ends_at_hundreds() {
        let labels = |len: usize| len / label_step(len);
        for len in 1..=HISTORY_CAPACITY {
            assert!(labels(len) <= MAX_LABELS, "{len} samples would show {} labels", labels(len));
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
