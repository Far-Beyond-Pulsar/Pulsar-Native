//! Performance metrics tracking for Mission Control

use std::collections::VecDeque;
use std::sync::Arc;
use sysinfo::System;
use parking_lot::RwLock;
use crate::gpu_info;

/// Maximum number of data points to keep in history
pub const MAX_HISTORY_SIZE: usize = 60;

/// CPU usage data point
#[derive(Clone)]
pub struct CpuDataPoint {
    pub index: usize,
    pub usage: f64,
}

/// Memory usage data point
#[derive(Clone)]
pub struct MemoryDataPoint {
    pub index: usize,
    pub memory_mb: f64,
}

/// GPU usage data point (from render metrics)
#[derive(Clone)]
pub struct GpuDataPoint {
    pub index: usize,
    /// VRAM currently used in MiB.
    pub vram_used_mb: f64,
}

/// FPS data point
#[derive(Clone)]
pub struct FpsDataPoint {
    pub index: usize,
    pub fps: f64,
}

/// Frame time data point
#[derive(Clone)]
pub struct FrameTimeDataPoint {
    pub index: usize,
    pub frame_time_ms: f64,
}

/// Container for all performance metrics
pub struct PerformanceMetrics {
    pub cpu_history: VecDeque<CpuDataPoint>,
    pub cpu_counter: usize,

    pub memory_history: VecDeque<MemoryDataPoint>,
    pub memory_counter: usize,

    pub gpu_history: VecDeque<GpuDataPoint>,
    pub gpu_counter: usize,

    pub fps_history: VecDeque<FpsDataPoint>,
    pub fps_counter: usize,

    pub frame_time_history: VecDeque<FrameTimeDataPoint>,
    pub frame_time_counter: usize,

    // Current values
    pub current_cpu: f64,
    pub current_memory_mb: f64,
    /// Live VRAM currently used in MiB (0 if unavailable).
    pub current_vram_used_mb: f64,
    pub current_fps: f64,
    pub current_frame_time_ms: f64,

    // System info
    system: System,
    current_pid: sysinfo::Pid,
}

impl PerformanceMetrics {
    pub fn new() -> Self {
        let mut system = System::new_all();
        system.refresh_all();

        let current_pid = sysinfo::get_current_pid().unwrap_or(sysinfo::Pid::from_u32(0));

        Self {
            cpu_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            cpu_counter: 0,

            memory_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            memory_counter: 0,

            gpu_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            gpu_counter: 0,

            fps_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            fps_counter: 0,

            frame_time_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            frame_time_counter: 0,

            current_cpu: 0.0,
            current_memory_mb: 0.0,
            current_vram_used_mb: 0.0,
            current_fps: 0.0,
            current_frame_time_ms: 0.0,

            system,
            current_pid,
        }
    }

    /// Update system metrics (CPU, Memory)
    pub fn update_system_metrics(&mut self) {
        // Refresh system info
        self.system.refresh_cpu();
        self.system.refresh_memory();
        self.system.refresh_processes();

        // Get CPU usage for current process
        let cpu_usage = if let Some(process) = self.system.process(self.current_pid) {
            process.cpu_usage() as f64
        } else {
            // Fallback to average CPU usage if process not found
            let cpus = self.system.cpus();
            if !cpus.is_empty() {
                cpus.iter().map(|cpu| cpu.cpu_usage()).sum::<f32>() as f64 / cpus.len() as f64
            } else {
                0.0
            }
        };

        // Get memory usage for current process in MB
        let memory_mb = if let Some(process) = self.system.process(self.current_pid) {
            process.memory() as f64 / 1024.0 / 1024.0
        } else {
            0.0
        };

        self.current_cpu = cpu_usage;
        self.current_memory_mb = memory_mb;

        // Query live VRAM usage from platform APIs.
        if let Some(used_mb) = gpu_info::query_vram_used_mb() {
            self.current_vram_used_mb = used_mb as f64;
        }

        // Add to history
        self.add_cpu(cpu_usage);
        self.add_memory(memory_mb);
        self.add_gpu(self.current_vram_used_mb);
    }

    /// Update from render metrics (FPS, Frame Time)
    pub fn update_from_render_metrics(&mut self, fps: f32, frame_time_ms: f32, _memory_mb: f32) {
        self.current_fps = fps as f64;
        self.current_frame_time_ms = frame_time_ms as f64;

        self.add_fps(fps as f64);
        self.add_frame_time(frame_time_ms as f64);
    }

    fn add_cpu(&mut self, usage: f64) {
        if self.cpu_history.len() >= MAX_HISTORY_SIZE {
            self.cpu_history.pop_front();
        }
        self.cpu_history.push_back(CpuDataPoint {
            index: self.cpu_counter,
            usage,
        });
        self.cpu_counter += 1;
    }

    fn add_memory(&mut self, memory_mb: f64) {
        if self.memory_history.len() >= MAX_HISTORY_SIZE {
            self.memory_history.pop_front();
        }
        self.memory_history.push_back(MemoryDataPoint {
            index: self.memory_counter,
            memory_mb,
        });
        self.memory_counter += 1;
    }

    fn add_gpu(&mut self, vram_used_mb: f64) {
        if self.gpu_history.len() >= MAX_HISTORY_SIZE {
            self.gpu_history.pop_front();
        }
        self.gpu_history.push_back(GpuDataPoint {
            index: self.gpu_counter,
            vram_used_mb,
        });
        self.gpu_counter += 1;
    }

    fn add_fps(&mut self, fps: f64) {
        if self.fps_history.len() >= MAX_HISTORY_SIZE {
            self.fps_history.pop_front();
        }
        self.fps_history.push_back(FpsDataPoint {
            index: self.fps_counter,
            fps,
        });
        self.fps_counter += 1;
    }

    fn add_frame_time(&mut self, frame_time_ms: f64) {
        if self.frame_time_history.len() >= MAX_HISTORY_SIZE {
            self.frame_time_history.pop_front();
        }
        self.frame_time_history.push_back(FrameTimeDataPoint {
            index: self.frame_time_counter,
            frame_time_ms,
        });
        self.frame_time_counter += 1;
    }
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared performance metrics accessible across the application
pub type SharedPerformanceMetrics = Arc<RwLock<PerformanceMetrics>>;

/// Create a new shared performance metrics instance
pub fn create_shared_metrics() -> SharedPerformanceMetrics {
    Arc::new(RwLock::new(PerformanceMetrics::new()))
}
