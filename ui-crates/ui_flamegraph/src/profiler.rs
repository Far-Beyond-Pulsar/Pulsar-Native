//! Real-time profiler using DTrace for cross-platform profiling

use std::sync::Arc;
use std::thread;
use std::time::Duration;
use parking_lot::RwLock;
use dtrace_profiler::DTraceProfiler as DTrace;
use crate::trace_data::{TraceData, TraceSpan, TraceFrame};

/// Background profiler that samples the running process using DTrace
pub struct BackgroundProfiler {
    dtrace: Arc<DTrace>,
    trace_data: Arc<TraceData>,
    running: Arc<RwLock<bool>>,
    update_interval_ms: u64,
}

impl BackgroundProfiler {
    /// Create a new background profiler
    /// 
    /// # Arguments
    /// * `trace_data` - Shared TraceData to update with profiling results
    /// * `sample_rate` - Samples per second (e.g., 100 for 100 Hz)
    /// * `update_interval_ms` - How often to update the UI with new data
    pub fn new(trace_data: Arc<TraceData>, sample_rate: i32, update_interval_ms: u64) -> Self {
        let dtrace = Arc::new(DTrace::new());
        
        Self {
            dtrace,
            trace_data,
            running: Arc::new(RwLock::new(false)),
            update_interval_ms,
        }
    }

    /// Start profiling in a background thread
    pub fn start(&self) {
        let mut running = self.running.write();
        if *running {
            return; // Already running
        }
        *running = true;

        let trace_data = Arc::clone(&self.trace_data);
        let running_flag = Arc::clone(&self.running);
        let dtrace = Arc::clone(&self.dtrace);
        let update_interval = self.update_interval_ms;

        // Start DTrace sampling
        if let Err(e) = dtrace.start(99) {
            eprintln!("[PROFILER] Failed to start DTrace: {}", e);
            *self.running.write() = false;
            return;
        }

        thread::spawn(move || {
            profiler_update_loop(dtrace, trace_data, running_flag, update_interval);
        });
    }

    /// Stop profiling
    pub fn stop(&self) {
        let mut running = self.running.write();
        *running = false;
        self.dtrace.stop();
    }

    /// Check if profiler is running
    pub fn is_running(&self) -> bool {
        *self.running.read()
    }
}

/// The profiler update loop that periodically converts DTrace samples to TraceData
fn profiler_update_loop(
    dtrace: Arc<DTrace>,
    trace_data: Arc<TraceData>,
    running: Arc<RwLock<bool>>,
    update_interval_ms: u64,
) {
    println!("[PROFILER] Starting DTrace update loop");

    while *running.read() {
        thread::sleep(Duration::from_millis(update_interval_ms));

        // Get samples from DTrace
        let samples = dtrace.take_samples();
        
        if samples.is_empty() {
            continue;
        }

        println!("[PROFILER] Processing {} samples", samples.len());

        // Convert to TraceData format
        if let Err(e) = convert_dtrace_to_trace(&samples, &trace_data) {
            eprintln!("[PROFILER] Failed to convert samples: {}", e);
        }
    }

    println!("[PROFILER] DTrace update loop stopped");
}

/// Convert DTrace samples to TraceData format
fn convert_dtrace_to_trace(
    samples: &[dtrace_profiler::Sample],
    trace_data: &TraceData,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::HashMap;

    let mut spans = Vec::new();
    let mut thread_names = HashMap::new();
    let base_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos() as u64;

    // Process each sample
    for (sample_idx, sample) in samples.iter().enumerate() {
        let thread_id = sample.thread_id;
        thread_names.entry(thread_id)
            .or_insert_with(|| format!("Thread {}", thread_id));

        // Create spans for each frame in the stack
        for (depth, frame) in sample.stack_frames.iter().enumerate() {
            let duration_ns = 10_000_000; // Estimate ~10ms per sample
            
            spans.push(TraceSpan {
                name: clean_function_name(&frame.function_name),
                start_ns: sample.timestamp_ns,
                duration_ns,
                depth: depth as u32,
                thread_id,
                color_index: (sample_idx % 16) as u8,
            });
        }
    }

    // Update the trace data
    trace_data.set_frame(TraceFrame::with_data(spans, thread_names));

    Ok(())
}

/// Clean up function names for display
fn clean_function_name(name: &str) -> String {
    // Remove module prefixes and clean up symbols
    name.split('`')
        .last()
        .unwrap_or(name)
        .split("::")
        .last()
        .unwrap_or(name)
        .split('<')
        .next()
        .unwrap_or(name)
        .split('+')
        .next()
        .unwrap_or(name)
        .to_string()
}

