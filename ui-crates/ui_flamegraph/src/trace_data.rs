use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::collections::HashMap;
use parking_lot::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSpan {
    pub name: String,
    pub start_ns: u64,
    pub duration_ns: u64,
    pub depth: u32,
    pub thread_id: u64,
    pub color_index: u8,
}

impl TraceSpan {
    pub fn end_ns(&self) -> u64 {
        self.start_ns + self.duration_ns
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThreadInfo {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct TraceFrame {
    pub spans: Vec<TraceSpan>,
    pub min_time_ns: u64,
    pub max_time_ns: u64,
    pub max_depth: u32,
    pub threads: HashMap<u64, ThreadInfo>,
    pub frame_times_ms: Vec<f32>, // History of frame times
}

impl TraceFrame {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_span(&mut self, span: TraceSpan) {
        if self.spans.is_empty() {
            self.min_time_ns = span.start_ns;
            self.max_time_ns = span.end_ns();
        } else {
            self.min_time_ns = self.min_time_ns.min(span.start_ns);
            self.max_time_ns = self.max_time_ns.max(span.end_ns());
        }
        self.max_depth = self.max_depth.max(span.depth);
        
        // Ensure thread exists
        if !self.threads.contains_key(&span.thread_id) {
            self.threads.insert(span.thread_id, ThreadInfo {
                id: span.thread_id,
                name: match span.thread_id {
                    0 => "GPU".to_string(),
                    1 => "Main Thread".to_string(),
                    id => format!("Worker {}", id - 1),
                },
            });
        }
        
        self.spans.push(span);
    }

    pub fn duration_ns(&self) -> u64 {
        if self.spans.is_empty() {
            0
        } else {
            self.max_time_ns - self.min_time_ns
        }
    }
    
    pub fn add_frame_time(&mut self, ms: f32) {
        self.frame_times_ms.push(ms);
        // Keep last 200 frames
        if self.frame_times_ms.len() > 200 {
            self.frame_times_ms.remove(0);
        }
    }
}

#[derive(Clone)]
pub struct TraceData {
    inner: Arc<RwLock<TraceFrame>>,
}

impl TraceData {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(TraceFrame::new())),
        }
    }

    pub fn add_span(&self, span: TraceSpan) {
        self.inner.write().add_span(span);
    }

    pub fn add_frame_time(&self, ms: f32) {
        self.inner.write().add_frame_time(ms);
    }

    pub fn get_frame(&self) -> TraceFrame {
        self.inner.read().clone()
    }

    pub fn clear(&self) {
        *self.inner.write() = TraceFrame::new();
    }
}

impl Default for TraceData {
    fn default() -> Self {
        Self::new()
    }
}