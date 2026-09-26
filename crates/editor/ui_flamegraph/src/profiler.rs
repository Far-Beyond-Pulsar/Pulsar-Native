//! Real-time profiler using instrumentation for cross-platform profiling

use crate::trace_data::{ThreadInfo, TraceData, TraceFrame, TraceSpan};
use parking_lot::Mutex;
use profiling::remote::TargetConnection;
use profiling::ProfileEvent;
use std::collections::HashMap;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::thread;
use std::time::Duration;

/// Where a recording's events come from.
enum Source {
    /// This process's own instrumentation queue.
    Local,
    /// Another process, through its shared-memory ring
    /// (`profiling::remote`).
    Remote(TargetConnection),
}

impl Source {
    fn collect(&mut self, session: &Mutex<Vec<ProfileEvent>>) -> Vec<ProfileEvent> {
        match self {
            Source::Local => profiling::collect_events(),
            Source::Remote(connection) => {
                let mut events = Vec::new();
                connection.read_events(&mut events);
                // The target keeps nothing, so the session is kept here for
                // saving (bounded like the local profiler's retention).
                let mut session = session.lock();
                let room = profiling::profiler::DEFAULT_RETAINED_EVENT_CAPACITY.saturating_sub(session.len());
                session.extend(events.iter().take(room).cloned());
                events
            }
        }
    }
}

/// Background collector that periodically grabs instrumentation events
pub struct InstrumentationCollector {
    trace_data: Arc<TraceData>,
    running: Arc<AtomicBool>,
    update_interval_ms: u64,
    /// Taken by the collector thread when it starts.
    source: Mutex<Option<Source>>,
    remote: bool,
    uncap_frame_rate: bool,
    /// A remote recording's events, for saving.
    session: Arc<Mutex<Vec<ProfileEvent>>>,
}

impl InstrumentationCollector {
    /// A collector for this process's own instrumentation.
    ///
    /// # Arguments
    /// * `trace_data` - Shared TraceData to update with profiling results
    /// * `update_interval_ms` - How often to collect and update the UI
    pub fn new(trace_data: Arc<TraceData>, update_interval_ms: u64) -> Self {
        Self {
            trace_data,
            running: Arc::new(AtomicBool::new(false)),
            update_interval_ms,
            source: Mutex::new(Some(Source::Local)),
            remote: false,
            uncap_frame_rate: false,
            session: Arc::default(),
        }
    }

    /// A collector for another process (a running game, another editor).
    pub fn remote(
        trace_data: Arc<TraceData>,
        update_interval_ms: u64,
        connection: TargetConnection,
        uncap_frame_rate: bool,
    ) -> Self {
        Self {
            trace_data,
            running: Arc::new(AtomicBool::new(false)),
            update_interval_ms,
            source: Mutex::new(Some(Source::Remote(connection))),
            remote: true,
            uncap_frame_rate,
            session: Arc::default(),
        }
    }

    /// Whether this records another process.
    pub fn is_remote(&self) -> bool {
        self.remote
    }

    /// The events of a remote recording so far (local recordings are kept
    /// by the in-process profiler: `profiling::get_all_events`).
    pub fn session_events(&self) -> Vec<ProfileEvent> {
        self.session.lock().clone()
    }

    /// Start collecting in a background thread. Fails when a remote target
    /// cannot be recorded (e.g. another viewer is recording it).
    pub fn start(&self) -> Result<(), String> {
        let Some(mut source) = self.source.lock().take() else {
            tracing::trace!("[PROFILER] Already running, ignoring start request");
            return Ok(());
        };
        match &mut source {
            Source::Local => {
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
            }
            Source::Remote(connection) => {
                if let Err(error) = connection.start_recording(self.uncap_frame_rate) {
                    *self.source.lock() = Some(source);
                    return Err(error);
                }
                tracing::info!(pid = connection.pid(), "[PROFILER] Recording another process");
            }
        }
        self.running.store(true, Ordering::Release);

        let trace_data = Arc::clone(&self.trace_data);
        let running_flag = Arc::clone(&self.running);
        let update_interval = self.update_interval_ms;
        let session = Arc::clone(&self.session);

        thread::spawn(move || {
            collector_loop(trace_data, running_flag, update_interval, source, session);
        });
        Ok(())
    }

    /// Stop collecting
    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);

        // Do not join from the UI thread. The collector only owns lock-free
        // queues and will observe this flag on its next tick; joining here
        // created a UI/render shutdown dependency and was a deadlock vector.
        // A remote target stops streaming when the thread drops its
        // connection.
        if !self.remote {
            profiling::disable_profiling();
        }
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
    mut source: Source,
    session: Arc<Mutex<Vec<ProfileEvent>>>,
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
        let new_events = source.collect(&session);
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
                } else if event.thread_name.is_some() {
                    // Only publish a real nickname. A later unnamed event must
                    // never erase a meaningful name already on this lane.
                    delta_thread_names.push((thread_id, lane_name(event)));
                } else if let Some(name) = accumulator.thread_names.get(&event.thread_id) {
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

/// The lane an event is drawn on: the thread it actually ran on.
///
/// Lanes are deliberately NOT semantic groups. A span appears on the real
/// thread that executed it, in the order it executed, so a blocked or busy
/// thread is visible as such. Nothing is matched on span or thread *names*
/// either: CPU scopes such as `helio_flush_gpu_mirror` contain "gpu" and must
/// stay on their thread. The one non-thread lane is the GPU timeline, whose
/// synthesized events are emitted with `thread_id == 0` at the source.
fn normalized_thread_id(event: &profiling::ProfileEvent) -> u64 {
    event.thread_id
}

/// Display name for an event's lane: the thread's nickname when one was set
/// (`profiling::set_thread_name`) or the OS thread name, else a generic label.
fn lane_name(event: &profiling::ProfileEvent) -> String {
    if event.thread_id == 0 {
        return "GPU".to_string();
    }
    event
        .thread_name
        .clone()
        .unwrap_or_else(|| format!("Thread {}", event.thread_id))
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
        thread_names.insert(thread_id, lane_name(event));

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

#[cfg(test)]
mod tests {
    use super::*;
    use profiling::remote::{self, TargetDescription};
    use std::time::Instant;

    /// A remote collector records another target through shared memory:
    /// spans reach the trace, frame markers the frame-time graph, and the
    /// session is kept for saving. (The target publishes from this process
    /// here; `profiling`'s own test covers two real processes.)
    #[test]
    fn a_remote_collector_records_a_published_target() {
        let dir = std::env::temp_dir().join(format!("flamegraph-remote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let description = TargetDescription { kind: "game".into(), name: "g".into(), project: String::new() };
        let publisher = remote::serve_in(&dir, description, 1 << 20).unwrap();
        let target = remote::list_targets_in(&dir).pop().expect("listed");
        assert!(target.is_current_process());

        let trace = Arc::new(TraceData::new());
        let connection = TargetConnection::open(&target.path).unwrap();
        let collector = InstrumentationCollector::remote(Arc::clone(&trace), 10, connection, false);
        assert!(collector.is_remote());
        collector.start().unwrap();

        let deadline = Instant::now() + Duration::from_secs(20);
        while collector.session_events().iter().filter(|e| e.name == "remote_frame").count() < 20 {
            assert!(Instant::now() < deadline, "no events arrived");
            {
                profiling::profile_scope!("remote_frame");
                profiling::profile_scope!("remote_work");
                std::hint::black_box((0..100u64).sum::<u64>());
            }
            profiling::record_frame_time(2.0);
            thread::sleep(Duration::from_millis(2));
        }
        // Let the collector publish a batch to the trace.
        thread::sleep(Duration::from_millis(100));
        collector.stop();

        let frame = trace.get_frame();
        assert!(frame.spans.iter().any(|s| s.name == "remote_work" && s.depth == 1), "spans reached the trace");
        assert!(!frame.frame_times_ms.is_empty(), "frame markers reached the frame-time graph");
        assert!(collector.session_events().iter().any(|e| e.name == "remote_work"));
        drop(publisher);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
