//! Performance monitoring data structures and utilities.
//!
//! This module provides data types for tracking various performance metrics
//! including FPS, TPS, frame time, memory usage, draw calls, vertices,
//! input latency, and UI consistency.

use std::collections::VecDeque;

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
        self.ui_consistency_history.push_back(UiConsistencyDataPoint {
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
