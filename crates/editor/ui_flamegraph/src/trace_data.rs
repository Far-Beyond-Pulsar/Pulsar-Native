use arc_swap::ArcSwap;
use crossbeam_queue::SegQueue;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;

use crate::sample_data;

static FLUSH_COUNT: AtomicU64 = AtomicU64::new(0);
static FLUSHED_SPANS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSpan {
    pub name: String,
    pub start_ns: u64,
    pub duration_ns: u64,
    pub depth: u32,
    pub thread_id: u64,
    pub color_index: u8,
}

impl TraceSpan {
    pub fn end_ns(&self) -> u64 {
        self.start_ns + self.duration_ns
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThreadInfo {
    pub id: u64,
    pub name: String,
}

impl ThreadInfo {
    /// Check if this thread has a human-friendly name (not just "Thread N")
    pub fn has_custom_name(&self) -> bool {
        !self.name.starts_with("Thread ")
    }

    /// Get a sort priority (lower = earlier)
    /// Named threads come first, then unnamed threads sorted by ID
    pub fn sort_priority(&self) -> (bool, u64) {
        (!self.has_custom_name(), self.id)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TraceFrame {
    pub spans: Vec<TraceSpan>,
    pub min_time_ns: u64,
    pub max_time_ns: u64,
    pub max_depth: u32,
    pub threads: HashMap<u64, ThreadInfo>,
    pub frame_times_ms: Vec<f32>, // History of frame times
    /// Start timestamps of each recorded frame (one per `__FRAME_MARKER__`
    /// event, or the frame starts emitted by the trace generators). Sorted
    /// chronologically. Each marks where a frame ends and the next begins.
    pub frame_boundaries_ns: Vec<u64>,
}

impl TraceFrame {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_data(spans: Vec<TraceSpan>, threads: HashMap<u64, String>) -> Self {
        let mut frame = Self::default();

        // Convert thread names HashMap to ThreadInfo HashMap
        for (id, name) in threads {
            frame.threads.insert(id, ThreadInfo { id, name });
        }

        // Add all spans
        for span in spans {
            frame.add_span(span);
        }

        frame
    }

    pub fn add_span(&mut self, span: TraceSpan) {
        if self.spans.is_empty() {
            self.min_time_ns = span.start_ns;
            self.max_time_ns = span.end_ns();
        } else {
            self.min_time_ns = self.min_time_ns.min(span.start_ns);
            self.max_time_ns = self.max_time_ns.max(span.end_ns());
        }
        self.max_depth = self.max_depth.max(span.depth);

        // Ensure thread exists
        self.threads
            .entry(span.thread_id)
            .or_insert_with(|| ThreadInfo {
                id: span.thread_id,
                name: match span.thread_id {
                    0 => "GPU".to_string(),
                    1 => "Main Thread".to_string(),
                    2 => "Render Thread".to_string(),
                    3 => "Physics Thread".to_string(),
                    4 => "Audio Thread".to_string(),
                    5 => "Network Thread".to_string(),
                    6 => "I/O Thread".to_string(),
                    7..=10 => format!("Worker {}", span.thread_id - 7),
                    id => format!("Thread {}", id),
                },
            });

        self.spans.push(span);
    }

    pub fn duration_ns(&self) -> u64 {
        if self.spans.is_empty() {
            0
        } else {
            self.max_time_ns - self.min_time_ns
        }
    }

    /// Get threads sorted with named threads first, then by ID
    pub fn get_sorted_threads(&self) -> Vec<ThreadInfo> {
        let mut threads: Vec<ThreadInfo> = self.threads.values().cloned().collect();
        threads.sort_by_key(|t| t.sort_priority());
        threads
    }

    pub fn add_frame_time(&mut self, ms: f32) {
        self.frame_times_ms.push(ms);
        // Keep last 200 frames
        if self.frame_times_ms.len() > 200 {
            self.frame_times_ms.remove(0);
        }
    }

    /// Record the start timestamp of a frame boundary. Callers are expected to
    /// push boundaries in chronological order.
    pub fn add_frame_boundary(&mut self, start_ns: u64) {
        self.frame_boundaries_ns.push(start_ns);
    }
}

#[derive(Clone)]
pub struct TraceData {
    inner: Arc<ArcSwap<TraceFrame>>,
    pending: Arc<SegQueue<TraceDelta>>,
}

struct TraceDelta {
    thread_names: Vec<(u64, String)>,
    spans: Vec<TraceSpan>,
    frame_times: Vec<f32>,
    frame_boundaries: Vec<u64>,
}

impl TraceData {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ArcSwap::from_pointee(TraceFrame::new())),
            pending: Arc::new(SegQueue::new()),
        }
    }

    /// Create TraceData with comprehensive sample data
    /// Generates 2000+ frames with dedicated threads for engine subsystems
    pub fn with_sample_data() -> Self {
        let trace = Self::new();
        sample_data::populate_sample_trace(&trace, sample_data::SAMPLE_FRAME_COUNT);

        let total_spans = trace.get_frame().spans.len();
        tracing::trace!(
            "Generated sample trace: {} spans across {} frames",
            total_spans,
            sample_data::SAMPLE_FRAME_COUNT
        );

        trace
    }

    /// Generate a massive multi-threaded trace for scalability testing.
    /// Creates `num_threads` threads each with `depth` nesting levels and ~`spans_per_frame` total
    /// spans distributed across `num_frames` frames. Total spans ≈ num_frames * spans_per_frame.
    pub fn generate_massive_trace(
        &self,
        num_threads: u64,
        depth: u32,
        num_frames: usize,
        spans_per_frame: usize,
    ) {
        use rand::RngExt;
        let mut rng = rand::rng();
        let mut current_time = 0u64;
        let mut all_spans = Vec::new();
        let mut frame_times = Vec::new();
        let mut frame_boundaries = Vec::new();

        let thread_names: Vec<String> = (0..num_threads)
            .map(|t| match t {
                0 => "GPU".to_string(),
                1 => "Main".to_string(),
                2 => "Render".to_string(),
                3 => "Physics".to_string(),
                4 => "Audio".to_string(),
                id => format!("Worker_{}", id - 5),
            })
            .collect();

        let base_frame_time = 16_600_000u64;

        for _ in 0..num_frames {
            let frame_start = current_time;
            let frame_dur = (base_frame_time as i64 + rng.random_range(-3_000_000i64..5_000_000i64))
                .max(8_000_000) as u64;

            frame_times.push(frame_dur as f32 / 1_000_000.0);
            frame_boundaries.push(frame_start);

            // Distribute ~spans_per_frame across threads
            let spans_per_thread = (spans_per_frame / num_threads as usize).max(1);
            for tid in 0..num_threads {
                let thread_stagger = rng.random_range(0u64..1_000_000);
                let thread_start = frame_start + thread_stagger;
                let thread_dur = frame_dur.saturating_sub(thread_stagger);

                for s in 0..spans_per_thread {
                    // Build a nesting tree: each span is at a random depth up to `depth`
                    let span_depth = (s as u32) % (depth + 1);
                    let span_frac = s as f64 / spans_per_thread as f64;
                    let span_start = thread_start + (span_frac * thread_dur as f64) as u64;
                    let span_end = if s + 1 < spans_per_thread {
                        thread_start
                            + (((s + 1) as f64 / spans_per_thread as f64) * thread_dur as f64)
                                as u64
                    } else {
                        thread_start + thread_dur
                    };
                    let span_dur = span_end.saturating_sub(span_start).max(1_000);

                    // Build a compound name showing thread + depth
                    let name = if span_depth == 0 {
                        format!("{}::Frame", thread_names[tid as usize])
                    } else {
                        let ops = ["Process", "Update", "Prepare", "Execute", "Finalize"];
                        let op = ops[(span_depth as usize - 1) % ops.len()];
                        format!("{}::{}_{}", thread_names[tid as usize], op, s)
                    };

                    all_spans.push(TraceSpan {
                        name,
                        start_ns: span_start,
                        duration_ns: span_dur,
                        depth: span_depth,
                        thread_id: tid,
                        color_index: ((tid + s as u64) % 16) as u8,
                    });
                }
            }

            current_time += frame_dur;
        }

        let mut frame = TraceFrame::new();
        frame.frame_times_ms = frame_times;
        frame.frame_boundaries_ns = frame_boundaries;

        let threads: HashMap<u64, String> = thread_names
            .into_iter()
            .enumerate()
            .map(|(id, name)| (id as u64, name))
            .collect();

        // Build frame with spans
        for span in all_spans {
            frame.add_span(span);
        }

        tracing::trace!(
            "Generated massive trace: {} spans, {} threads, {} frames",
            frame.spans.len(),
            frame.threads.len(),
            num_frames,
        );

        self.set_frame(frame);
    }

    pub fn add_span(&self, span: TraceSpan) {
        let mut frame = (*self.inner.load_full()).clone();
        frame.add_span(span);
        self.inner.store(Arc::new(frame));
    }

    pub fn add_frame_time(&self, ms: f32) {
        let mut frame = (*self.inner.load_full()).clone();
        frame.add_frame_time(ms);
        self.inner.store(Arc::new(frame));
    }

    pub fn add_frame_boundary(&self, start_ns: u64) {
        let mut frame = (*self.inner.load_full()).clone();
        frame.add_frame_boundary(start_ns);
        self.inner.store(Arc::new(frame));
    }

    pub fn get_frame(&self) -> Arc<TraceFrame> {
        self.inner.load_full()
    }

    /// Publish queued collector deltas without making a UI render pay the
    /// copy-on-write cost. The collector owns this work: `get_frame()` is now
    /// a pure ArcSwap load and therefore cannot stall the editor while a trace
    /// grows.
    pub fn publish_pending(&self) {
        self.flush_pending();
    }

    /// Cheap diagnostic snapshot. This intentionally does not flush pending
    /// deltas, so instrumentation can report backlog pressure without doing
    /// the expensive publication work it is measuring.
    pub fn debug_stats(&self) -> (usize, usize, usize, usize) {
        let frame = self.inner.load();
        (
            frame.spans.len(),
            frame.threads.len(),
            self.pending.len(),
            frame.frame_boundaries_ns.len(),
        )
    }

    fn flush_pending(&self) {
        if self.pending.is_empty() {
            return;
        }
        let started = Instant::now();
        let mut frame = (*self.inner.load_full()).clone();
        let old_span_count = frame.spans.len();
        let mut changed = false;
        let mut delta_count = 0usize;
        while let Some(delta) = self.pending.pop() {
            changed = true;
            delta_count += 1;
            for (id, name) in delta.thread_names {
                frame.threads.entry(id).or_insert(ThreadInfo { id, name });
            }
            for span in delta.spans {
                frame.add_span(span);
            }
            for time in delta.frame_times {
                frame.add_frame_time(time);
            }
            for boundary in delta.frame_boundaries {
                frame.add_frame_boundary(boundary);
            }
        }
        if changed {
            let new_span_count = frame.spans.len();
            self.inner.store(Arc::new(frame));
            let flushes = FLUSH_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            FLUSHED_SPANS.fetch_add(new_span_count as u64, Ordering::Relaxed);
            let elapsed = started.elapsed();
            if elapsed >= std::time::Duration::from_millis(5) || flushes % 60 == 0 {
                tracing::warn!(
                    target: "flamegraph.workload",
                    flushes,
                    delta_count,
                    old_span_count,
                    new_span_count,
                    pending_after = self.pending.len(),
                    elapsed_ms = elapsed.as_secs_f64() * 1000.0,
                    cumulative_copied_spans = FLUSHED_SPANS.load(Ordering::Relaxed),
                    "trace publication cloned the accumulated history"
                );
            }
        }
    }

    pub fn set_frame(&self, frame: TraceFrame) {
        self.inner.store(Arc::new(frame));
    }

    pub fn clear(&self) {
        self.inner.store(Arc::new(TraceFrame::new()));
    }

    pub fn append_batch(
        &self,
        thread_names: impl IntoIterator<Item = (u64, String)>,
        spans: impl IntoIterator<Item = TraceSpan>,
        frame_times: impl IntoIterator<Item = f32>,
        frame_boundaries: impl IntoIterator<Item = u64>,
    ) {
        self.pending.push(TraceDelta {
            thread_names: thread_names.into_iter().collect(),
            spans: spans.into_iter().collect(),
            frame_times: frame_times.into_iter().collect(),
            frame_boundaries: frame_boundaries.into_iter().collect(),
        });
    }
}

impl Default for TraceData {
    fn default() -> Self {
        Self::new()
    }
}
