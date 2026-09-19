//! Real-time profiler using instrumentation for cross-platform profiling

use crate::trace_data::{ThreadInfo, TraceData, TraceFrame, TraceSpan};
use std::collections::HashMap;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::thread;
use std::time::Duration;

/// Background collector that periodically grabs instrumentation events
pub struct InstrumentationCollector {
    trace_data: Arc<TraceData>,
    running: Arc<AtomicBool>,
    update_interval_ms: u64,
}

impl InstrumentationCollector {
    /// Create a new instrumentation collector
    ///
    /// # Arguments
    /// * `trace_data` - Shared TraceData to update with profiling results
    /// * `update_interval_ms` - How often to collect and update the UI
    pub fn new(trace_data: Arc<TraceData>, update_interval_ms: u64) -> Self {
        Self {
            trace_data,
            running: Arc::new(AtomicBool::new(false)),
            update_interval_ms,
        }
    }

    /// Start collecting in a background thread
    pub fn start(&self) {
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            tracing::trace!("[PROFILER] Already running, ignoring start request");
            return;
        }

        // Profiling is only live while the Flamegraph panel is actively
        // recording — enabling it here (rather than for the whole process
        // lifetime) is what keeps profile_scope! from leaking events into
        // an unbounded channel when nobody is collecting them.
        profiling::clear_events();
        profiling::enable_profiling();

        tracing::trace!(
            "[PROFILER] Profiling enabled: {}",
            profiling::is_profiling_enabled()
        );

        // Drain the producer queue once so the collector can begin from a
        // known boundary. Do not synthesize a test span or sleep here: both
        // change the timing of the application being profiled.
        let initial_events = profiling::collect_events();

        tracing::trace!("[PROFILER] Current event count: {}", initial_events.len());

        let trace_data = Arc::clone(&self.trace_data);
        let running_flag = Arc::clone(&self.running);
        let update_interval = self.update_interval_ms;

        thread::spawn(move || {
            collector_loop(trace_data, running_flag, update_interval);
        });
    }

    /// Stop collecting
    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);

        // Do not join from the UI thread. The collector only owns lock-free
        // queues and will observe this flag on its next tick; joining here
        // created a UI/render shutdown dependency and was a deadlock vector.
        profiling::disable_profiling();
    }

    /// Check if collector is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

/// The collector loop that periodically fetches events
fn collector_loop(
    trace_data: Arc<TraceData>,
    running: Arc<AtomicBool>,
    update_interval_ms: u64,
) {
    tracing::trace!("[PROFILER] Starting instrumentation collector");

    let mut accumulator = TraceAccumulator::from_frame(&trace_data.get_frame());
    let mut last_report = std::time::Instant::now();
    let mut batches = 0u64;
    while running.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(update_interval_ms));

        // `collect_events` returns exactly the events drained on this tick.
        // Consuming that delta avoids cloning the complete session history
        // every 100ms, which used to make the profiler increasingly expensive
        // during long captures.
        let new_events = profiling::collect_events();
        if new_events.is_empty() {
            continue;
        }
        let batch_started = std::time::Instant::now();
        batches += 1;

        tracing::trace!(
            "[PROFILER] Collected {} new instrumentation events",
            new_events.len()
        );

        // Convert and append ONLY new events. Do not rebuild the accumulated
        // trace on every tick; that turns profiling into an eventual stall.
        let mut delta_spans = Vec::new();
        let mut delta_thread_names = Vec::new();
        let mut delta_times = Vec::new();
        let mut delta_boundaries = Vec::new();
        for event in &new_events {
            accumulator.apply_event(event);
            if event.name == "__FRAME_MARKER__" {
                delta_times.push(event.duration_ns as f32 / 1_000_000.0);
                delta_boundaries.push(event.start_ns);
            } else {
                let thread_id = normalized_thread_id(event);
                if thread_id == 0 {
                    delta_thread_names.push((0, "GPU".to_string()));
                } else if let Some(track_name) = event.track_name.as_deref() {
                    delta_thread_names.push((thread_id, track_name.to_string()));
                } else if let Some(name) = accumulator.thread_names.get(&event.thread_id) {
                    // Only publish a fallback name when the profiler actually
                    // knows one. A later unnamed event must never erase a
                    // meaningful name already associated with this lane.
                    delta_thread_names.push((thread_id, name.name.clone()));
                }
                delta_spans.push(TraceSpan {
                    name: event.name.clone(),
                    start_ns: event.start_ns,
                    duration_ns: event.duration_ns,
                    depth: event.depth,
                    thread_id,
                    color_index: (delta_spans.len() % 16) as u8,
                });
            }
        }
        trace_data.append_batch(
            delta_thread_names,
            delta_spans,
            delta_times,
            delta_boundaries,
        );
        // Trace publication clones the immutable snapshot. Keep that work on
        // the collector thread; the UI must only load the completed ArcSwap
        // snapshot during render and never flush an ever-growing history.
        trace_data.publish_pending();

        if last_report.elapsed() >= Duration::from_secs(1) {
            let (spans, threads, pending, boundaries) = trace_data.debug_stats();
            tracing::warn!(
                target: "flamegraph.workload",
                batches,
                events = new_events.len(),
                producer_queue = profiling::init_profiler().pending_event_count(),
                retained_events = profiling::init_profiler().retained_event_count(),
                dropped_events = profiling::init_profiler().dropped_event_count(),
                trace_spans = spans,
                trace_threads = threads,
                pending_deltas = pending,
                frame_boundaries = boundaries,
                batch_ms = batch_started.elapsed().as_secs_f64() * 1000.0,
                "flamegraph collector workload"
            );
            last_report = std::time::Instant::now();
        } else if batch_started.elapsed() >= Duration::from_millis(10) {
            tracing::warn!(
                target: "flamegraph.workload",
                batch_ms = batch_started.elapsed().as_secs_f64() * 1000.0,
                events = new_events.len(),
                "slow flamegraph collector batch"
            );
        }
    }

    // Profiling itself is disabled by InstrumentationCollector::stop(), which
    // runs concurrently with this loop exiting.
    tracing::trace!("[PROFILER] Instrumentation collector stopped");
}

impl Drop for InstrumentationCollector {
    fn drop(&mut self) {
        self.stop();
    }
}

fn normalized_thread_id(event: &profiling::ProfileEvent) -> u64 {
    if let Some(track_name) = event.track_name.as_deref() {
        return logical_track_id(track_name);
    }

    let mut text = event.name.to_ascii_lowercase();
    if let Some(thread_name) = event.thread_name.as_deref() {
        text.push(' ');
        text.push_str(&thread_name.to_ascii_lowercase());
    }

    if event
        .metadata
        .as_deref()
        .is_some_and(|metadata| metadata.contains("track=gpu"))
    {
        return 0;
    }

    // Track ids are semantic lanes, not OS/thread ids. Helio's RenderGraph
    // executes passes on worker threads, so using event.thread_id here turns
    // LightCull/WaterSim into a forest of anonymous rows. Keep CPU-side Helio
    // work on one stable named lane; reserve lane 0 for actual GPU events.
    if text.contains("gpu") {
        0
    } else {
        event.thread_id
    }
}

fn logical_track_id(name: &str) -> u64 {
    if name.eq_ignore_ascii_case("gpu") {
        return 0;
    }
    let mut hash = 0xcbf29ce484222325u64;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if hash == 0 { 1 } else { hash }
}


#[derive(Default)]
struct TraceAccumulator {
    spans: Vec<TraceSpan>,
    thread_names: HashMap<u64, ThreadInfo>,
    frame_times: Vec<f32>,
    frame_boundaries: Vec<u64>,
}

impl TraceAccumulator {
    fn from_frame(frame: &TraceFrame) -> Self {
        Self {
            spans: frame.spans.clone(),
            thread_names: frame.threads.clone(),
            frame_times: frame.frame_times_ms.clone(),
            frame_boundaries: frame.frame_boundaries_ns.clone(),
        }
    }

    fn apply_event(&mut self, event: &profiling::ProfileEvent) {
        if event.name == "__FRAME_MARKER__" {
            self.frame_times
                .push(event.duration_ns as f32 / 1_000_000.0);
            // The marker's start timestamp is where the new frame begins.
            self.frame_boundaries.push(event.start_ns);
            return;
        }

        let thread_name = event
            .thread_name
            .clone()
            .unwrap_or_else(|| format!("Thread {}", event.thread_id));

        self.thread_names.insert(
            event.thread_id,
            ThreadInfo {
                id: event.thread_id,
                name: thread_name,
            },
        );

        self.spans.push(TraceSpan {
            name: event.name.clone(),
            start_ns: event.start_ns,
            duration_ns: event.duration_ns,
            depth: event.depth,
            thread_id: event.thread_id,
            color_index: (self.spans.len() % 16) as u8,
        });
    }

    fn publish(&self, trace_data: &TraceData) -> Result<(), Box<dyn std::error::Error>> {
        // Build through with_data so min/max time and depth are recomputed from spans.
        let thread_names: HashMap<u64, String> = self
            .thread_names
            .iter()
            .map(|(id, info)| (*id, info.name.clone()))
            .collect();
        let mut frame = TraceFrame::with_data(self.spans.clone(), thread_names);
        frame.frame_times_ms = self.frame_times.clone();
        frame.frame_boundaries_ns = self.frame_boundaries.clone();
        trace_data.set_frame(frame);
        Ok(())
    }
}

/// Convert profiling events to TraceData format and ADD them (don't replace!)
pub fn convert_profile_events_to_trace(
    events: &[profiling::ProfileEvent],
    trace_data: &TraceData,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::HashMap;

    // Get current frame to preserve existing data
    let current_frame = trace_data.get_frame();
    let existing_span_count = current_frame.spans.len();
    let mut spans = current_frame.spans.clone();
    let mut thread_names: HashMap<u64, String> = current_frame
        .threads
        .iter()
        .map(|(id, info)| (*id, info.name.clone()))
        .collect();
    let mut frame_times = current_frame.frame_times_ms.clone();
    let mut frame_boundaries = current_frame.frame_boundaries_ns.clone();

    tracing::trace!("[PROFILER] BEFORE: {} existing spans", existing_span_count);

    // Add new events to existing spans and extract frame times
    for (idx, event) in events.iter().enumerate() {
        let thread_id = normalized_thread_id(event);

        // Check if this is a frame marker event
        if event.name == "__FRAME_MARKER__" {
            // Extract frame time from duration field (stored in nanoseconds)
            let frame_time_ms = event.duration_ns as f32 / 1_000_000.0;
            frame_times.push(frame_time_ms);
            frame_boundaries.push(event.start_ns);
            tracing::trace!(
                "[PROFILER] Frame marker: {:.2}ms ({:.1} FPS)",
                frame_time_ms,
                1000.0 / frame_time_ms
            );
            continue; // Don't add frame markers as regular spans
        }

        // Use the thread name from the event if available
        let thread_name = if thread_id == 0 {
            "GPU".to_string()
        } else if let Some(track_name) = event.track_name.as_deref() {
            track_name.to_string()
        } else {
            event
                .thread_name
                .clone()
                .unwrap_or_else(|| format!("Thread {}", event.thread_id))
        };

        thread_names.insert(thread_id, thread_name);

        // Create span from event
        spans.push(TraceSpan {
            name: event.name.clone(),
            start_ns: event.start_ns,
            duration_ns: event.duration_ns,
            depth: event.depth,
            thread_id,
            color_index: (idx % 16) as u8,
        });

        // Debug: Print first few spans to see durations
        if idx < 3 {
            tracing::trace!(
                "[PROFILER] Span {}: {} @ {}ns for {}ns ({:.2}ms)",
                idx,
                event.name,
                event.start_ns,
                event.duration_ns,
                event.duration_ns as f64 / 1_000_000.0
            );
        }
    }

    tracing::trace!(
        "[PROFILER] AFTER: {} spans (added {}), {} frame times",
        spans.len(),
        spans.len() - existing_span_count,
        frame_times.len()
    );

    // Update the trace data with accumulated spans and frame times
    let mut frame = TraceFrame::with_data(spans.clone(), thread_names.clone());
    frame.frame_times_ms = frame_times;
    frame.frame_boundaries_ns = frame_boundaries;
    trace_data.set_frame(frame);

    // Verify it was set correctly
    let verification_frame = trace_data.get_frame();
    tracing::trace!(
        "[PROFILER] VERIFIED: TraceData now has {} spans across {} threads, {} frame times",
        verification_frame.spans.len(),
        verification_frame.threads.len(),
        verification_frame.frame_times_ms.len()
    );

    Ok(())
}
