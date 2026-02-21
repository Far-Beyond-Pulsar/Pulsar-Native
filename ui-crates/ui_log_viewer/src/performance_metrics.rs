//! Performance metrics tracking for Mission Control

use std::collections::VecDeque;
use std::sync::Arc;
use sysinfo::{System, Networks, Components};
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

/// GPU VRAM usage data point
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

/// Network throughput data point (KB/s)
#[derive(Clone)]
pub struct NetDataPoint {
    pub index: usize,
    pub kbps: f64,
}

/// Disk throughput data point (KB/s)
#[derive(Clone)]
pub struct DiskDataPoint {
    pub index: usize,
    pub kbps: f64,
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

    // Network
    pub net_rx_history: VecDeque<NetDataPoint>,
    pub net_tx_history: VecDeque<NetDataPoint>,
    pub net_counter: usize,

    // Disk (process-level)
    pub disk_read_history: VecDeque<DiskDataPoint>,
    pub disk_write_history: VecDeque<DiskDataPoint>,
    pub disk_counter: usize,

    // Current values
    pub current_cpu: f64,
    pub current_memory_mb: f64,
    /// Live VRAM currently used in MiB (0 if unavailable).
    pub current_vram_used_mb: f64,
    pub current_fps: f64,
    pub current_frame_time_ms: f64,
    pub current_net_rx_kbps: f64,
    pub current_net_tx_kbps: f64,
    pub current_disk_read_kbps: f64,
    pub current_disk_write_kbps: f64,

    /// Per-core history: (core_index, history_deque). Populated on first update.
    pub cpu_core_histories: Vec<VecDeque<f64>>,
    /// Per-sensor temperature history: (label, history_deque). Populated on first update.
    pub temp_histories: Vec<(String, VecDeque<f64>)>,

    /// Per-GPU-engine utilization history: (engine_name, history). Populated on first update.
    pub gpu_engine_histories: std::collections::HashMap<String, VecDeque<f64>>,
    /// Shared (non-local/system) GPU memory currently used in MiB.
    pub current_vram_shared_mb: f64,

    // System info
    system: System,
    networks: Networks,
    components: Components,
    current_pid: sysinfo::Pid,
}

impl PerformanceMetrics {
    pub fn new() -> Self {
        let mut system = System::new_all();
        system.refresh_all();

        let networks = Networks::new_with_refreshed_list();
        let components = Components::new_with_refreshed_list();
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

            net_rx_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            net_tx_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            net_counter: 0,

            disk_read_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            disk_write_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            disk_counter: 0,

            current_cpu: 0.0,
            current_memory_mb: 0.0,
            current_vram_used_mb: 0.0,
            current_fps: 0.0,
            current_frame_time_ms: 0.0,
            current_net_rx_kbps: 0.0,
            current_net_tx_kbps: 0.0,
            current_disk_read_kbps: 0.0,
            current_disk_write_kbps: 0.0,
            cpu_core_histories: Vec::new(),
            temp_histories: Vec::new(),
            gpu_engine_histories: std::collections::HashMap::new(),
            current_vram_shared_mb: 0.0,

            system,
            networks,
            components,
            current_pid,
        }
    }

    /// Update system metrics — called every second from the background task.
    pub fn update_system_metrics(&mut self) {
        self.system.refresh_cpu();
        self.system.refresh_memory();
        self.system.refresh_processes();
        self.networks.refresh();
        #[cfg(not(windows))]
        self.components.refresh();

        // ── Per-process CPU + memory ──────────────────────────────────────────
        let cpu_usage = if let Some(process) = self.system.process(self.current_pid) {
            process.cpu_usage() as f64
        } else {
            let cpus = self.system.cpus();
            if !cpus.is_empty() {
                cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() as f64 / cpus.len() as f64
            } else {
                0.0
            }
        };

        let memory_mb = self.system.process(self.current_pid)
            .map(|p| p.memory() as f64 / 1024.0 / 1024.0)
            .unwrap_or(0.0);

        self.current_cpu = cpu_usage;
        self.current_memory_mb = memory_mb;

        // ── Per-core CPU histories ────────────────────────────────────────────
        let core_count = self.system.cpus().len();
        if self.cpu_core_histories.len() != core_count {
            self.cpu_core_histories = vec![VecDeque::with_capacity(MAX_HISTORY_SIZE); core_count];
        }
        for (i, cpu) in self.system.cpus().iter().enumerate() {
            let hist = &mut self.cpu_core_histories[i];
            if hist.len() >= MAX_HISTORY_SIZE { hist.pop_front(); }
            hist.push_back(cpu.cpu_usage() as f64);
        }

        // ── Per-sensor temperature histories ─────────────────────────────────
        // Windows: temperature access is not reliably available without a
        // kernel driver. We surface a UI note instead of showing garbage data.
        #[cfg(not(windows))]
        {
            self.components.refresh(false);
            for comp in self.components.iter() {
                let label = comp.label().to_string();
                let temp = match comp.temperature() {
                    Some(t) => t as f64,
                    None => continue,
                };
                if let Some(entry) = self.temp_histories.iter_mut().find(|(l, _)| *l == label) {
                    if entry.1.len() >= MAX_HISTORY_SIZE { entry.1.pop_front(); }
                    entry.1.push_back(temp);
                } else {
                    let mut dq = VecDeque::with_capacity(MAX_HISTORY_SIZE);
                    dq.push_back(temp);
                    self.temp_histories.push((label, dq));
                }
            }
        }

        // ── GPU VRAM (dedicated + shared) ────────────────────────────────────
        if let Some(used_mb) = gpu_info::query_vram_used_mb() {
            self.current_vram_used_mb = used_mb as f64;
        }
        if let Some(shared_mb) = gpu_info::query_vram_shared_mb() {
            self.current_vram_shared_mb = shared_mb as f64;
        }

        // ── GPU engine utilization (PDH, Windows only) ────────────────────────
        let engine_map = crate::gpu_engines::collect();
        for (engine, pct) in &engine_map {
            let hist = self.gpu_engine_histories
                .entry(engine.clone())
                .or_insert_with(|| VecDeque::with_capacity(MAX_HISTORY_SIZE));
            if hist.len() >= MAX_HISTORY_SIZE { hist.pop_front(); }
            hist.push_back(*pct);
        }
        // Also push 0 for any known engine not seen this tick (keeps histories in sync)
        if !engine_map.is_empty() {
            for &eng in crate::gpu_engines::KNOWN_ENGINES {
                if !engine_map.contains_key(eng) {
                    if let Some(hist) = self.gpu_engine_histories.get_mut(eng) {
                        if hist.len() >= MAX_HISTORY_SIZE { hist.pop_front(); }
                        hist.push_back(0.0);
                    }
                }
            }
        }

        // ── Network (bytes since last refresh ≈ bytes/s) ─────────────────────
        let (total_rx, total_tx) = self.networks
            .iter()
            .fold((0u64, 0u64), |(rx, tx), (_, data)| {
                (rx + data.received(), tx + data.transmitted())
            });
        self.current_net_rx_kbps = total_rx as f64 / 1024.0;
        self.current_net_tx_kbps = total_tx as f64 / 1024.0;

        // ── Disk I/O (process-level, bytes since last refresh ≈ bytes/s) ─────
        let (disk_r, disk_w) = self.system.process(self.current_pid)
            .map(|p| {
                let u = p.disk_usage();
                (u.read_bytes as f64 / 1024.0, u.written_bytes as f64 / 1024.0)
            })
            .unwrap_or((0.0, 0.0));
        self.current_disk_read_kbps = disk_r;
        self.current_disk_write_kbps = disk_w;

        // ── Push to histories ─────────────────────────────────────────────────
        self.add_cpu(cpu_usage);
        self.add_memory(memory_mb);
        self.add_gpu(self.current_vram_used_mb);
        self.add_net(self.current_net_rx_kbps, self.current_net_tx_kbps);
        self.add_disk(self.current_disk_read_kbps, self.current_disk_write_kbps);
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
        if self.gpu_history.len() >= MAX_HISTORY_SIZE { self.gpu_history.pop_front(); }
        self.gpu_history.push_back(GpuDataPoint { index: self.gpu_counter, vram_used_mb });
        self.gpu_counter += 1;
    }

    fn add_net(&mut self, rx_kbps: f64, tx_kbps: f64) {
        if self.net_rx_history.len() >= MAX_HISTORY_SIZE { self.net_rx_history.pop_front(); }
        if self.net_tx_history.len() >= MAX_HISTORY_SIZE { self.net_tx_history.pop_front(); }
        self.net_rx_history.push_back(NetDataPoint { index: self.net_counter, kbps: rx_kbps });
        self.net_tx_history.push_back(NetDataPoint { index: self.net_counter, kbps: tx_kbps });
        self.net_counter += 1;
    }

    fn add_disk(&mut self, read_kbps: f64, write_kbps: f64) {
        if self.disk_read_history.len() >= MAX_HISTORY_SIZE { self.disk_read_history.pop_front(); }
        if self.disk_write_history.len() >= MAX_HISTORY_SIZE { self.disk_write_history.pop_front(); }
        self.disk_read_history.push_back(DiskDataPoint { index: self.disk_counter, kbps: read_kbps });
        self.disk_write_history.push_back(DiskDataPoint { index: self.disk_counter, kbps: write_kbps });
        self.disk_counter += 1;
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
