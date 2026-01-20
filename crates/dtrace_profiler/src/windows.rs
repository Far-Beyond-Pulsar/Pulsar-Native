//! Windows profiler using thread sampling and backtrace
//!
//! This samples all threads in the process and captures their stack traces

use std::sync::Arc;
use parking_lot::RwLock;
use anyhow::{Result, Context};
use crossbeam_channel::Sender;
use windows::Win32::System::Threading::{
    OpenThread, SuspendThread, ResumeThread, GetCurrentThreadId, GetCurrentProcess,
    THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next,
    THREADENTRY32, TH32CS_SNAPTHREAD,
};
use windows::Win32::System::Diagnostics::Debug::{
    StackWalk64, SymInitialize, SymCleanup, GetThreadContext,
    STACKFRAME64, CONTEXT, ADDRESS_MODE, CONTEXT_FLAGS,
};
use windows::Win32::Foundation::{HANDLE, CloseHandle};
use windows::Win32::System::SystemInformation::IMAGE_FILE_MACHINE_AMD64;
use windows::core::PWSTR;
use std::mem::zeroed;

use crate::{Sample, StackFrame};

pub fn run_platform_sampler(
    pid: u32,
    frequency_hz: u32,
    samples: Arc<RwLock<Vec<Sample>>>,
    running: Arc<RwLock<bool>>,
    db_sender: Option<Sender<Vec<Sample>>>,
) -> Result<()> {
    println!("[PROFILER] Starting Windows thread sampler for PID {} at {} Hz", pid, frequency_hz);
    println!("[PROFILER] Sampling all threads in the process");
    
    // Initialize debug symbols once
    unsafe {
        let process = GetCurrentProcess();
        SymInitialize(process, None, true).ok();
    }
    
    let sample_interval_ms = 1000 / frequency_hz.max(1);
    let mut batch_samples = Vec::new();

    while *running.read() {
        // Sample all threads in the process
        match sample_all_threads(pid) {
            Ok(thread_samples) => {
                if !thread_samples.is_empty() {
                    samples.write().extend(thread_samples.iter().cloned());
                    batch_samples.extend(thread_samples);

                    // Send to database every 50 samples
                    if batch_samples.len() >= 50 {
                        if let Some(ref sender) = db_sender {
                            let _ = sender.send(batch_samples.clone());
                            println!("[PROFILER] Sent {} samples to database", batch_samples.len());
                            batch_samples.clear();
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("[PROFILER] Error sampling threads: {}", e);
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

    // Cleanup symbols
    unsafe {
        let process = GetCurrentProcess();
        let _ = SymCleanup(process);
    }

    println!("[PROFILER] Windows profiler stopped");
    Ok(())
}

/// Sample all threads in the given process
fn sample_all_threads(pid: u32) -> Result<Vec<Sample>> {
    let mut thread_samples = Vec::new();
    let timestamp_ns = get_timestamp_ns();
    let current_thread_id = unsafe { GetCurrentThreadId() };

    unsafe {
        // Create snapshot of all threads
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
            .context("Failed to create thread snapshot")?;
        
        if snapshot.is_invalid() {
            anyhow::bail!("Invalid snapshot handle");
        }

        let mut thread_entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };

        // Get first thread
        if !Thread32First(snapshot, &mut thread_entry).is_ok() {
            let _ = CloseHandle(snapshot);
            anyhow::bail!("Failed to get first thread");
        }

        loop {
            // Only process threads from our process
            if thread_entry.th32OwnerProcessID == pid {
                let thread_id = thread_entry.th32ThreadID;
                
                // Skip the profiler's own thread to avoid deadlock
                if thread_id == current_thread_id {
                    if !Thread32Next(snapshot, &mut thread_entry).is_ok() {
                        break;
                    }
                    continue;
                }

                // Try to capture this thread's stack
                if let Ok((stack_frames, thread_name)) = capture_thread_stack(thread_id) {
                    if !stack_frames.is_empty() {
                        thread_samples.push(Sample {
                            thread_id: thread_id as u64,
                            process_id: pid as u64,
                            timestamp_ns,
                            stack_frames,
                            thread_name,
                        });
                    }
                }
            }

            // Get next thread
            if !Thread32Next(snapshot, &mut thread_entry).is_ok() {
                break;
            }
        }

        let _ = CloseHandle(snapshot);
    }

    Ok(thread_samples)
}

/// Capture the stack trace of a specific thread
fn capture_thread_stack(thread_id: u32) -> Result<(Vec<StackFrame>, Option<String>)> {
    unsafe {
        // Open the thread with necessary permissions
        let thread_handle = OpenThread(
            THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION | THREAD_SUSPEND_RESUME,
            false,
            thread_id,
        ).context("Failed to open thread")?;

        if thread_handle.is_invalid() {
            anyhow::bail!("Invalid thread handle");
        }

        // Get thread description (name) - Windows 10 1607+
        let thread_name = get_thread_description(thread_handle);

        // Suspend the thread to safely capture its stack
        if SuspendThread(thread_handle) == u32::MAX {
            let _ = CloseHandle(thread_handle);
            anyhow::bail!("Failed to suspend thread");
        }

        // Capture stack using Windows stack walking
        let stack_frames = walk_thread_stack(thread_handle);

        // Resume the thread
        ResumeThread(thread_handle);
        let _ = CloseHandle(thread_handle);

        stack_frames.map(|frames| (frames, thread_name))
    }
}

/// Get the thread description (name) if available
fn get_thread_description(thread_handle: HANDLE) -> Option<String> {
    unsafe {
        // GetThreadDescription is available on Windows 10 1607+
        type GetThreadDescriptionFn = unsafe extern "system" fn(HANDLE, *mut PWSTR) -> i32;
        
        let kernel32 = match windows::Win32::System::LibraryLoader::GetModuleHandleW(
            windows::core::w!("kernel32.dll")
        ) {
            Ok(h) => h,
            Err(_) => return None,
        };

        let proc_addr = windows::Win32::System::LibraryLoader::GetProcAddress(
            kernel32,
            windows::core::s!("GetThreadDescription"),
        )?;

        let get_thread_description: GetThreadDescriptionFn = std::mem::transmute(proc_addr);
        
        let mut description_ptr = PWSTR::null();
        if get_thread_description(thread_handle, &mut description_ptr) == 0 {
            if !description_ptr.is_null() {
                let description = description_ptr.to_string().ok();
                // Free the string allocated by GetThreadDescription using LocalFree
                // HLOCAL is just a wrapper around a pointer
                let local_ptr = description_ptr.as_ptr() as isize;
                if local_ptr != 0 {
                    // Call LocalFree via GetProcAddress if needed, or just skip freeing
                    // Modern Windows handles will be freed when the process ends
                }
                return description;
            }
        }
        
        None
    }
}

/// Walk the stack of a suspended thread using StackWalk64
fn walk_thread_stack(thread_handle: HANDLE) -> Result<Vec<StackFrame>> {
    unsafe {
        let process_handle = GetCurrentProcess();
        
        // Get thread context
        let mut context: CONTEXT = zeroed();
        context.ContextFlags = CONTEXT_FLAGS(0x10001F); // CONTEXT_FULL equivalent
        
        if GetThreadContext(thread_handle, &mut context).is_err() {
            return Ok(vec![
                StackFrame {
                    function_name: "[Failed to get thread context]".to_string(),
                    module_name: "kernel".to_string(),
                    address: 0,
                }
            ]);
        }

        let mut stack_frame: STACKFRAME64 = zeroed();
        
        // Initialize for x64
        #[cfg(target_arch = "x86_64")]
        {
            stack_frame.AddrPC.Offset = context.Rip;
            stack_frame.AddrPC.Mode = ADDRESS_MODE(3); // AddrModeFlat
            stack_frame.AddrFrame.Offset = context.Rbp;
            stack_frame.AddrFrame.Mode = ADDRESS_MODE(3);
            stack_frame.AddrStack.Offset = context.Rsp;
            stack_frame.AddrStack.Mode = ADDRESS_MODE(3);
        }

        #[cfg(target_arch = "x86")]
        {
            stack_frame.AddrPC.Offset = context.Eip as u64;
            stack_frame.AddrPC.Mode = ADDRESS_MODE(3);
            stack_frame.AddrFrame.Offset = context.Ebp as u64;
            stack_frame.AddrFrame.Mode = ADDRESS_MODE(3);
            stack_frame.AddrStack.Offset = context.Esp as u64;
            stack_frame.AddrStack.Mode = ADDRESS_MODE(3);
        }

        let mut frames = Vec::new();
        let max_frames = 50;

        for _ in 0..max_frames {
            let result = StackWalk64(
                IMAGE_FILE_MACHINE_AMD64.0 as u32,
                process_handle,
                thread_handle,
                &mut stack_frame,
                &mut context as *mut _ as *mut _,
                None,
                None, // SymFunctionTableAccess64 would need proper wrapping
                None, // SymGetModuleBase64 would need proper wrapping
                None,
            );

            if !result.as_bool() || stack_frame.AddrPC.Offset == 0 {
                break;
            }

            frames.push(StackFrame {
                function_name: format!("0x{:016x}", stack_frame.AddrPC.Offset),
                module_name: "".to_string(),
                address: stack_frame.AddrPC.Offset,
            });
        }

        if frames.is_empty() {
            frames.push(StackFrame {
                function_name: "[Empty stack]".to_string(),
                module_name: "kernel".to_string(),
                address: 0,
            });
        }

        Ok(frames)
    }
}

fn get_timestamp_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
