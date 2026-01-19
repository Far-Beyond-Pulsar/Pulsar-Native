//! Cross-platform profiler
//!
//! - Windows: Simple PowerShell-based sampling (ETW/WPR requires complex setup)
//! - macOS: DTrace
//! - Linux: perf

mod database;

#[cfg(windows)]
mod windows;

#[cfg(unix)]
mod unix;

use std::sync::Arc;
use std::path::{Path, PathBuf};
use parking_lot::RwLock;
use anyhow::{Result, Context};
use crossbeam_channel::{Sender, Receiver, unbounded};

pub use database::{TraceDatabase, TraceInfo};

/// A CPU sample containing stack trace information
#[derive(Debug, Clone)]
pub struct Sample {
    /// Thread ID that was sampled
    pub thread_id: u64,
    /// Process ID
    pub process_id: u64,
    /// Timestamp in nanoseconds
    pub timestamp_ns: u64,
    /// Stack frames from bottom (deepest) to top
    pub stack_frames: Vec<StackFrame>,
}

/// A single stack frame
#[derive(Debug, Clone)]
pub struct StackFrame {
    /// Function name or symbol
    pub function_name: String,
    /// Module/library name
    pub module_name: String,
    /// Address in memory
    pub address: u64,
}

/// Profiler that samples the current process using pprof
pub struct DTraceProfiler {
    process_id: u32,
    samples: Arc<RwLock<Vec<Sample>>>,
    running: Arc<RwLock<bool>>,
    db_path: Option<PathBuf>,
    trace_id: Option<i64>,
    db_sender: Option<Sender<Vec<Sample>>>,
}

impl DTraceProfiler {
    /// Create a new profiler for the current process
    pub fn new() -> Self {
        Self {
            process_id: std::process::id(),
            samples: Arc::new(RwLock::new(Vec::new())),
            running: Arc::new(RwLock::new(false)),
            db_path: None,
            trace_id: None,
            db_sender: None,
        }
    }

    /// Create a new profiler with database persistence
    pub fn with_database(db_path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            process_id: std::process::id(),
            samples: Arc::new(RwLock::new(Vec::new())),
            running: Arc::new(RwLock::new(false)),
            db_path: Some(db_path.as_ref().to_path_buf()),
            trace_id: None,
            db_sender: None,
        })
    }

    /// Start profiling with the given sample frequency (Hz)
    pub fn start(&mut self, frequency_hz: u32) -> Result<()> {
        let mut running = self.running.write();
        if *running {
            anyhow::bail!("Profiler is already running");
        }

        // Start database writer thread if we have a database
        let sender = if let Some(ref db_path) = self.db_path {
            let mut database = TraceDatabase::create(db_path)?;
            let trace_id = database.start_trace(frequency_hz, self.process_id)?;
            self.trace_id = Some(trace_id);
            println!("[PROFILER] Started trace session ID: {}", trace_id);

            let (tx, rx) = unbounded();
            
            // Spawn database writer thread
            std::thread::spawn(move || {
                database_writer_thread(database, trace_id, rx);
            });

            Some(tx)
        } else {
            None
        };

        self.db_sender = sender.clone();
        *running = true;

        let pid = self.process_id;
        let samples = Arc::clone(&self.samples);
        let running_flag = Arc::clone(&self.running);

        std::thread::spawn(move || {
            #[cfg(windows)]
            let result = crate::windows::run_platform_sampler(pid, frequency_hz, samples, running_flag, sender);
            
            #[cfg(unix)]
            let result = crate::unix::run_platform_sampler(pid, frequency_hz, samples, running_flag, sender);

            if let Err(e) = result {
                eprintln!("[PROFILER] Profiler error: {}", e);
            }
        });

        Ok(())
    }

    /// Stop profiling
    pub fn stop(&mut self) {
        *self.running.write() = false;

        // Close database sender to signal writer thread
        self.db_sender = None;
    }

    /// Check if profiler is running
    pub fn is_running(&self) -> bool {
        *self.running.read()
    }

    /// Get all collected samples and clear the buffer
    pub fn take_samples(&self) -> Vec<Sample> {
        let mut samples = self.samples.write();
        std::mem::take(&mut *samples)
    }

    /// Get current sample count without consuming
    pub fn sample_count(&self) -> usize {
        self.samples.read().len()
    }

    /// Get the trace ID for this profiling session
    pub fn trace_id(&self) -> Option<i64> {
        self.trace_id
    }

    /// Get the database path if one is configured
    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    /// Get samples from database since a timestamp
    pub fn get_samples_from_db(&self, since_ns: u64) -> Result<Vec<Sample>> {
        if let (Some(ref db_path), Some(trace_id)) = (&self.db_path, self.trace_id) {
            let db = TraceDatabase::create(db_path)?;
            db.get_samples_since(trace_id, since_ns)
        } else {
            Ok(Vec::new())
        }
    }
}

/// Database writer thread that receives batches of samples
fn database_writer_thread(mut database: TraceDatabase, trace_id: i64, receiver: Receiver<Vec<Sample>>) {
    println!("[PROFILER] Database writer thread started");
    
    while let Ok(samples) = receiver.recv() {
        if let Err(e) = database.insert_samples(trace_id, &samples) {
            eprintln!("[PROFILER] Failed to insert samples: {}", e);
        }
    }

    // End trace when channel closes
    if let Err(e) = database.end_trace(trace_id) {
        eprintln!("[PROFILER] Failed to end trace: {}", e);
    }

    println!("[PROFILER] Database writer thread stopped");
}
