//! Performance monitoring data structures and utilities.
//!
//! This module provides data types for tracking various performance metrics
//! including FPS, TPS, frame time, memory usage, draw calls, vertices,
//! input latency, and UI consistency.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Maximum number of data points to keep in history for rolling graphs.
pub const MAX_HISTORY_SIZE: usize = 120;

/// FPS (Frames Per Second) data point.
#[derive(Clone)]
pub struct FpsDataPoint {
    pub index: usize,
    pub fps: f64,
}

/// TPS (Ticks Per Second) data point.
#[derive(Clone)]
pub struct TpsDataPoint {
    pub index: usize,
    pub tps: f64,
}

/// Frame time data point for jitter analysis.
#[derive(Clone)]
pub struct FrameTimeDataPoint {
    pub index: usize,
    pub frame_time_ms: f64,
}

/// Memory usage data point.
#[derive(Clone)]
pub struct MemoryDataPoint {
    pub index: usize,
    pub memory_mb: f64,
}

/// Draw calls per frame data point.
#[derive(Clone)]
pub struct DrawCallsDataPoint {
    pub index: usize,
    pub draw_calls: f64,
}

/// Vertices rendered data point.
#[derive(Clone)]
pub struct VerticesDataPoint {
    pub index: usize,
    pub vertices: f64,
}

/// Input latency data point (measured on input thread).
#[derive(Clone)]
pub struct InputLatencyDataPoint {
    pub index: usize,
    pub latency_ms: f64,
}

/// UI refresh consistency data point (tracks FPS variance).
#[derive(Clone)]
pub struct UiConsistencyDataPoint {
    pub index: usize,
    pub consistency_score: f64,
}

/// Container for all performance metric histories.
pub struct PerformanceMetrics {
    pub fps_history: VecDeque<FpsDataPoint>,
    pub fps_sample_counter: usize,

    pub tps_history: VecDeque<TpsDataPoint>,
    pub tps_sample_counter: usize,

    pub frame_time_history: VecDeque<FrameTimeDataPoint>,
    pub frame_time_counter: usize,

    pub memory_history: VecDeque<MemoryDataPoint>,
    pub memory_counter: usize,

    pub draw_calls_history: VecDeque<DrawCallsDataPoint>,
    pub draw_calls_counter: usize,

    pub vertices_history: VecDeque<VerticesDataPoint>,
    pub vertices_counter: usize,

    pub input_latency_history: VecDeque<InputLatencyDataPoint>,
    pub input_latency_counter: usize,

    pub ui_consistency_history: VecDeque<UiConsistencyDataPoint>,
    pub ui_consistency_counter: usize,
}

impl PerformanceMetrics {
    /// Create a new performance metrics container with pre-allocated histories.
    pub fn new() -> Self {
        Self {
            fps_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            fps_sample_counter: 0,

            tps_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            tps_sample_counter: 0,

            frame_time_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            frame_time_counter: 0,

            memory_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            memory_counter: 0,

            draw_calls_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            draw_calls_counter: 0,

            vertices_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            vertices_counter: 0,

            input_latency_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            input_latency_counter: 0,

            ui_consistency_history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            ui_consistency_counter: 0,
        }
    }

    /// Add a new FPS data point to the history.
    pub fn add_fps(&mut self, fps: f64) {
        if self.fps_history.len() >= MAX_HISTORY_SIZE {
            self.fps_history.pop_front();
        }
        self.fps_history.push_back(FpsDataPoint {
            index: self.fps_sample_counter,
            fps,
        });
        self.fps_sample_counter += 1;
    }

    /// Add a new TPS data point to the history.
    pub fn add_tps(&mut self, tps: f64) {
        if self.tps_history.len() >= MAX_HISTORY_SIZE {
            self.tps_history.pop_front();
        }
        self.tps_history.push_back(TpsDataPoint {
            index: self.tps_sample_counter,
            tps,
        });
        self.tps_sample_counter += 1;
    }

    /// Add a new frame time data point to the history.
    pub fn add_frame_time(&mut self, frame_time_ms: f64) {
        if self.frame_time_history.len() >= MAX_HISTORY_SIZE {
            self.frame_time_history.pop_front();
        }
        self.frame_time_history.push_back(FrameTimeDataPoint {
            index: self.frame_time_counter,
            frame_time_ms,
        });
        self.frame_time_counter += 1;
    }

    /// Add a new memory usage data point to the history.
    pub fn add_memory(&mut self, memory_mb: f64) {
        if self.memory_history.len() >= MAX_HISTORY_SIZE {
            self.memory_history.pop_front();
        }
        self.memory_history.push_back(MemoryDataPoint {
            index: self.memory_counter,
            memory_mb,
        });
        self.memory_counter += 1;
    }

    /// Add a new draw calls data point to the history.
    pub fn add_draw_calls(&mut self, draw_calls: f64) {
        if self.draw_calls_history.len() >= MAX_HISTORY_SIZE {
            self.draw_calls_history.pop_front();
        }
        self.draw_calls_history.push_back(DrawCallsDataPoint {
            index: self.draw_calls_counter,
            draw_calls,
        });
        self.draw_calls_counter += 1;
    }

    /// Add a new vertices data point to the history.
    pub fn add_vertices(&mut self, vertices: f64) {
        if self.vertices_history.len() >= MAX_HISTORY_SIZE {
            self.vertices_history.pop_front();
        }
        self.vertices_history.push_back(VerticesDataPoint {
            index: self.vertices_counter,
            vertices,
        });
        self.vertices_counter += 1;
    }

    /// Add a new input latency data point to the history.
    pub fn add_input_latency(&mut self, latency_ms: f64) {
        if self.input_latency_history.len() >= MAX_HISTORY_SIZE {
            self.input_latency_history.pop_front();
        }
        self.input_latency_history.push_back(InputLatencyDataPoint {
            index: self.input_latency_counter,
            latency_ms,
        });
        self.input_latency_counter += 1;
    }

    /// Add a new UI consistency data point to the history.
    pub fn add_ui_consistency(&mut self, consistency_score: f64) {
        if self.ui_consistency_history.len() >= MAX_HISTORY_SIZE {
            self.ui_consistency_history.pop_front();
        }
        self.ui_consistency_history
            .push_back(UiConsistencyDataPoint {
                index: self.ui_consistency_counter,
                consistency_score,
            });
        self.ui_consistency_counter += 1;
    }
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-frame copy of everything the UI thread reads out of `GpuRenderer`,
/// gathered with one `try_lock` acquisition so the viewport render path
/// contends with the Helio render thread's blocking lock once instead of
/// several times per frame.
#[derive(Clone, Default)]
pub struct EngineFrameSnapshot {
    pub ui_fps: f64,
    pub helio_fps: f64,
    pub render_fps: f64,
    pub memory_mb: f64,
    pub draw_calls: f64,
    pub vertices: f64,
    pub frame_time_ms: f64,
    pub camera_input: Option<Arc<Mutex<engine_backend::subsystems::render::CameraInput>>>,
    pub pointer_events:
        Option<Arc<Mutex<Vec<engine_backend::subsystems::render::PendingPointerEvent>>>>,
}

impl EngineFrameSnapshot {
    /// Gather stats from `GpuRenderer`, pushing the frame-rate-independent
    /// camera-input settings (move speed, scroll zoom) in the same locked
    /// pass. Returns `None` if the renderer mutex was busy; callers skip
    /// their stat updates that frame.
    pub fn gather(
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        move_speed: f32,
        zoom_delta: f32,
    ) -> Option<Self> {
        let engine = gpu_engine.try_lock().ok()?;
        let metrics_opt = engine.get_render_metrics();
        let (memory_mb, draw_calls, vertices, frame_time_ms) = match metrics_opt {
            Some(ref m) => (
                m.memory_usage_mb as f64,
                m.draw_calls as f64,
                m.vertices_drawn as f64,
                m.frame_time_ms as f64,
            ),
            None => (0.0, 0.0, 0.0, 0.0),
        };

        let snapshot = Self {
            ui_fps: engine.get_fps() as f64,
            helio_fps: engine.get_helio_fps() as f64,
            render_fps: engine.get_render_fps() as f64,
            memory_mb,
            draw_calls,
            vertices,
            frame_time_ms,
            camera_input: engine.camera_input(),
            pointer_events: engine.pointer_event_queue(),
        };

        if let Some(cam) = engine.camera_input() {
            if let Ok(mut input) = cam.try_lock() {
                input.move_speed = move_speed;
                input.zoom_delta = zoom_delta;
            }
        }

        Some(snapshot)
    }
}

/// Immutable copy of the metric histories plus the headline FPS values the
/// performance overlay renders, built once per frame while the overlay is
/// open instead of threading eight separately cloned Vecs through the
/// element tree.
pub struct PerformanceSnapshot {
    pub ui_fps: f64,
    pub render_fps: f64,
    pub fps_history: Vec<FpsDataPoint>,
    pub frame_time_history: Vec<FrameTimeDataPoint>,
    pub memory_history: Vec<MemoryDataPoint>,
    pub draw_calls_history: Vec<DrawCallsDataPoint>,
    pub vertices_history: Vec<VerticesDataPoint>,
    pub input_latency_history: Vec<InputLatencyDataPoint>,
}

impl PerformanceSnapshot {
    pub fn empty() -> Self {
        Self {
            ui_fps: 0.0,
            render_fps: 0.0,
            fps_history: Vec::new(),
            frame_time_history: Vec::new(),
            memory_history: Vec::new(),
            draw_calls_history: Vec::new(),
            vertices_history: Vec::new(),
            input_latency_history: Vec::new(),
        }
    }

    pub fn capture(metrics: &PerformanceMetrics, ui_fps: f64, render_fps: f64) -> Self {
        Self {
            ui_fps,
            render_fps,
            fps_history: metrics.fps_history.iter().cloned().collect(),
            frame_time_history: metrics.frame_time_history.iter().cloned().collect(),
            memory_history: metrics.memory_history.iter().cloned().collect(),
            draw_calls_history: metrics.draw_calls_history.iter().cloned().collect(),
            vertices_history: metrics.vertices_history.iter().cloned().collect(),
            input_latency_history: metrics.input_latency_history.iter().cloned().collect(),
        }
    }
}

impl Default for PerformanceSnapshot {
    fn default() -> Self {
        Self::empty()
    }
}
