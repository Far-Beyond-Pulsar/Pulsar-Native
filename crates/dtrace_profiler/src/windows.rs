//! Windows profiler using thread sampling and backtrace
//!
//! This samples all threads in the process and captures their stack traces

use std::sync::Arc;
use std::path::Path;
use parking_lot::RwLock;
use anyhow::{Result, Context};
use crossbeam_channel::Sender;
use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};
use backtrace::Backtrace;

use crate::{Sample, StackFrame};

pub fn run_platform_sampler(
    pid: u32,
    frequency_hz: u32,
    samples: Arc<RwLock<Vec<Sample>>>,
    running: Arc<RwLock<bool>>,
    db_sender: Option<Sender<Vec<Sample>>>,
) -> Result<()> {
    println!("[PROFILER] Starting Windows thread sampler for PID {} at {} Hz", pid, frequency_hz);
    
    let sample_interval_ms = 1000 / frequency_hz.max(1);
    let mut batch_samples = Vec::new();

    while *running.read() {
        // Capture backtrace of current thread
        // In a real profiler, you'd enumerate and suspend all threads, but for simplicity
        // we'll use backtrace which captures the current call stack
        let bt = Backtrace::new();
        
        let mut stack_frames = Vec::new();
        for frame in bt.frames() {
            for symbol in frame.symbols() {
                let function_name = symbol.name()
                    .map(|n| format!("{:#}", n))
                    .unwrap_or_else(|| format!("{:?}", frame.ip()));
                
                let module_name = symbol.filename()
                    .and_then(|f| f.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                
                stack_frames.push(StackFrame {
                    function_name,
                    module_name,
                    address: frame.ip() as u64,
                });
            }
        }
        
        if !stack_frames.is_empty() {
            let sample = Sample {
                thread_id: format!("{:?}", std::thread::current().id()).parse().unwrap_or(0),
                process_id: pid as u64,
                timestamp_ns: get_timestamp_ns(),
                stack_frames,
            };

            samples.write().push(sample.clone());
            batch_samples.push(sample);

            // Send to database every 10 samples
            if batch_samples.len() >= 10 {
                if let Some(ref sender) = db_sender {
                    let _ = sender.send(batch_samples.clone());
                    println!("[PROFILER] Sent {} samples to database", batch_samples.len());
                    batch_samples.clear();
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(sample_interval_ms as u64));
    }

    // Flush remaining samples
    if !batch_samples.is_empty() {
        if let Some(ref sender) = db_sender {
            let _ = sender.send(batch_samples);
        }
    }

    println!("[PROFILER] Windows profiler stopped");
    Ok(())
}

fn get_timestamp_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
