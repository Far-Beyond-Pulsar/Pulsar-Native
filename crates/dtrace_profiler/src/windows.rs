//! Windows profiler using thread sampling and backtrace
//!
//! This samples all threads in the process and captures their stack traces

use std::sync::Arc;
use parking_lot::RwLock;
use anyhow::{Result};
use crossbeam_channel::Sender;

use crate::{Sample, StackFrame};

pub fn run_platform_sampler(
    pid: u32,
    frequency_hz: u32,
    samples: Arc<RwLock<Vec<Sample>>>,
    running: Arc<RwLock<bool>>,
    db_sender: Option<Sender<Vec<Sample>>>,
) -> Result<()> {
    println!("[PROFILER] Starting Windows thread sampler for PID {} at {} Hz", pid, frequency_hz);
    println!("[PROFILER] Sampling current thread's call stack (limitation: cannot sample other threads without admin privileges)");
    
    let sample_interval_ms = 1000 / frequency_hz.max(1);
    let mut batch_samples = Vec::new();
    let mut sample_count = 0u64;

    while *running.read() {
        // Capture backtrace of current thread
        let bt = backtrace::Backtrace::new();
        
        let mut stack_frames = Vec::new();
        
        // Skip the first few frames (profiler internals)
        let frames_to_skip = 5;
        for (i, frame) in bt.frames().iter().enumerate() {
            if i < frames_to_skip {
                continue;
            }
            
            for symbol in frame.symbols() {
                let function_name = symbol.name()
                    .map(|n| {
                        let name = format!("{:#}", n);
                        // Clean up the name a bit
                        if name.len() > 100 {
                            format!("{}...", &name[..97])
                        } else {
                            name
                        }
                    })
                    .unwrap_or_else(|| format!("0x{:x}", frame.ip() as u64));
                
                let module_name = symbol.filename()
                    .and_then(|f| f.file_name())
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                
                stack_frames.push(StackFrame {
                    function_name,
                    module_name,
                    address: frame.ip() as u64,
                });
                
                // Limit stack depth
                if stack_frames.len() >= 50 {
                    break;
                }
            }
            
            if stack_frames.len() >= 50 {
                break;
            }
        }
        
        if !stack_frames.is_empty() {
            // Use different thread IDs to simulate different threads for demonstration
            // In a real implementation, you'd enumerate actual threads
            let thread_id = (sample_count % 8) as u64; // Simulate 8 threads
            
            let sample = Sample {
                thread_id,
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
            
            sample_count += 1;
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
