//! Real-time profiler using instrumentation for cross-platform profiling

use crate::trace_data::{ThreadInfo, TraceData, TraceFrame, TraceSpan};
use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Background collector that periodically grabs instrumentation events
pub struct InstrumentationCollector {
    trace_data: Arc<TraceData>,
    running: Arc<parking_lot::RwLock<bool>>,
    frame_time_running: Arc<parking_lot::RwLock<bool>>,
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
            running: Arc::new(parking_lot::RwLock::new(false)),
            frame_time_running: Arc::new(parking_lot::RwLock::new(false)),
            update_interval_ms,
        }
    }

    /// Start collecting in a background thread
    pub fn start(&self) {
        let mut running = self.running.write();
        if *running {
            tracing::trace!("[PROFILER] Already running, ignoring start request");
            return; // Already running
        }
        *running = true;

        tracing::trace!(
            "[PROFILER] Profiling enabled: {}",
            profiling::is_profiling_enabled()
        );

        *self.frame_time_running.write() = true;

        let frame_time_running = Arc::clone(&self.frame_time_running);
        thread::spawn(move || {
            while *frame_time_running.read() {
                thread::sleep(Duration::from_millis(16));

                if let Some(frame_time_ms) = sample_current_frame_time_ms() {
                    profiling::record_frame_time(frame_time_ms);
                }
            }
        });

        // Create a test span to verify the system works
        {
            profiling::profile_scope!("TEST_SPAN_FROM_COLLECTOR");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        // Small delay to let the test span be recorded
        std::thread::sleep(std::time::Duration::from_millis(10));

        // CRITICAL: Collect events from channel into storage FIRST
        profiling::collect_events();

        tracing::trace!(
            "[PROFILER] Current event count: {}",
            profiling::get_all_events().len()
        );

        let trace_data = Arc::clone(&self.trace_data);
        let running_flag = Arc::clone(&self.running);
        let update_interval = self.update_interval_ms;

        // NOTE: Don't enable/disable profiling here!
        // Profiling is enabled globally at engine startup
        // We just collect the events that are already being recorded

        thread::spawn(move || {
            collector_loop(trace_data, running_flag, update_interval);
        });
    }

    /// Stop collecting
    pub fn stop(&self) {
        *self.running.write() = false;
        *self.frame_time_running.write() = false;
    }

    /// Check if collector is running
    pub fn is_running(&self) -> bool {
        *self.running.read()
    }
}

/// The collector loop that periodically fetches events
fn collector_loop(
    trace_data: Arc<TraceData>,
    running: Arc<parking_lot::RwLock<bool>>,
    update_interval_ms: u64,
) {
    tracing::trace!("[PROFILER] Starting instrumentation collector");

    let mut accumulator = TraceAccumulator::from_frame(&trace_data.get_frame());
    let mut last_event_count = 0;

    while *running.read() {
        thread::sleep(Duration::from_millis(update_interval_ms));

        // CRITICAL: Collect from channel into storage first!
        profiling::collect_events();

        // NOW get all events from storage
        let all_events = profiling::get_all_events();

        // Only process new events since last update
        if all_events.len() <= last_event_count {
            continue;
        }

        let new_events = &all_events[last_event_count..];
        last_event_count = all_events.len();

        tracing::trace!(
            "[PROFILER] Collected {} new instrumentation events (total: {})",
            new_events.len(),
            all_events.len()
        );

        // Convert ONLY new events to TraceData format
        for event in new_events {
            accumulator.apply_event(event);
        }

        if let Err(e) = accumulator.publish(&trace_data) {
            tracing::error!("[PROFILER] Failed to convert events: {}", e);
        }
    }

    // NOTE: Don't disable profiling here!
    // Profiling is managed by the engine, not the collector
    tracing::trace!("[PROFILER] Instrumentation collector stopped");
}

fn sample_current_frame_time_ms() -> Option<f32> {
    let engine_context = engine_state::EngineContext::global()?;

    for window_id in engine_context.renderers.window_ids() {
        let handle = engine_context.renderers.get(window_id)?;
        let Some(renderer) = handle
            .as_helio::<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>()
        else {
            continue;
        };

        if let Some(frame_time_ms) = sample_renderer_frame_time(renderer).filter(|ms| *ms > 0.0) {
            return Some(frame_time_ms);
        }
    }

    None
}

fn sample_renderer_frame_time(
    renderer: Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
) -> Option<f32> {
    let engine = renderer.try_lock().ok()?;
    engine.get_render_metrics().map(|metrics| metrics.frame_time_ms)
}

#[derive(Default)]
struct TraceAccumulator {
    spans: Vec<TraceSpan>,
    thread_names: HashMap<u64, ThreadInfo>,
    frame_times: Vec<f32>,
}

impl TraceAccumulator {
    fn from_frame(frame: &TraceFrame) -> Self {
        Self {
            spans: frame.spans.clone(),
            thread_names: frame.threads.clone(),
            frame_times: frame.frame_times_ms.clone(),
        }
    }

    fn apply_event(&mut self, event: &profiling::ProfileEvent) {
        if event.name == "__FRAME_MARKER__" {
            self.frame_times.push(event.duration_ns as f32 / 1_000_000.0);
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

    tracing::trace!("[PROFILER] BEFORE: {} existing spans", existing_span_count);

    // Add new events to existing spans and extract frame times
    for (idx, event) in events.iter().enumerate() {
        let thread_id = event.thread_id;

        // Check if this is a frame marker event
        if event.name == "__FRAME_MARKER__" {
            // Extract frame time from duration field (stored in nanoseconds)
            let frame_time_ms = event.duration_ns as f32 / 1_000_000.0;
            frame_times.push(frame_time_ms);
            tracing::trace!(
                "[PROFILER] Frame marker: {:.2}ms ({:.1} FPS)",
                frame_time_ms,
                1000.0 / frame_time_ms
            );
            continue; // Don't add frame markers as regular spans
        }

        // Use the thread name from the event if available
        let thread_name = event
            .thread_name
            .clone()
            .unwrap_or_else(|| format!("Thread {}", thread_id));

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
