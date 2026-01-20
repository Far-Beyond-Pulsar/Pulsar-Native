//! Real-time profiler using instrumentation for cross-platform profiling

use std::sync::Arc;
use std::thread;
use std::time::Duration;
use crate::trace_data::{TraceData, TraceSpan, TraceFrame};

/// Background collector that periodically grabs instrumentation events
pub struct InstrumentationCollector {
    trace_data: Arc<TraceData>,
    running: Arc<parking_lot::RwLock<bool>>,
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
            update_interval_ms,
        }
    }

    /// Start collecting in a background thread
    pub fn start(&self) {
        let mut running = self.running.write();
        if *running {
            return; // Already running
        }
        *running = true;

        let trace_data = Arc::clone(&self.trace_data);
        let running_flag = Arc::clone(&self.running);
        let update_interval = self.update_interval_ms;

        // Enable profiling globally
        profiling::enable_profiling();

        thread::spawn(move || {
            collector_loop(trace_data, running_flag, update_interval);
        });
    }

    /// Stop collecting
    pub fn stop(&self) {
        *self.running.write() = false;
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
    println!("[PROFILER] Starting instrumentation collector");

    while *running.read() {
        thread::sleep(Duration::from_millis(update_interval_ms));

        // Collect events from the profiler
        let events = profiling::collect_events();
        
        if events.is_empty() {
            continue;
        }

        println!("[PROFILER] Collected {} instrumentation events", events.len());

        // Convert to TraceData format
        if let Err(e) = convert_profile_events_to_trace(&events, &trace_data) {
            eprintln!("[PROFILER] Failed to convert events: {}", e);
        }
    }

    profiling::disable_profiling();
    println!("[PROFILER] Instrumentation collector stopped");
}

/// Convert profiling events to TraceData format
pub fn convert_profile_events_to_trace(
    events: &[profiling::ProfileEvent],
    trace_data: &TraceData,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::HashMap;

    let mut spans = Vec::new();
    let mut thread_names = HashMap::new();

    // Process each event
    for (idx, event) in events.iter().enumerate() {
        let thread_id = event.thread_id;
        
        // Use the thread name from the event if available
        let thread_name = event.thread_name.clone()
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
    }

    // Update the trace data
    trace_data.set_frame(TraceFrame::with_data(spans, thread_names));

    Ok(())
}

