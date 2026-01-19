//! View state management for the flamegraph viewer

use std::collections::BTreeMap;
use crate::trace_data::TraceFrame;
use crate::constants::*;

/// View state for pan, zoom, and interaction
#[derive(Clone)]
pub struct ViewState {
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    pub dragging: bool,
    pub drag_start_x: f32,
    pub drag_start_y: f32,
    pub drag_pan_start_x: f32,
    pub drag_pan_start_y: f32,
    pub hovered_span: Option<usize>,
    pub mouse_x: f32,
    pub mouse_y: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            dragging: false,
            drag_start_x: 0.0,
            drag_start_y: 0.0,
            drag_pan_start_x: 0.0,
            drag_pan_start_y: 0.0,
            hovered_span: None,
            mouse_x: 0.0,
            mouse_y: 0.0,
        }
    }
}

/// Cache for pre-computed span layout data
pub struct SpanCache {
    pub thread_offsets: BTreeMap<u64, f32>,
}

impl SpanCache {
    /// Build cache from a trace frame
    pub fn build(frame: &TraceFrame) -> Self {
        let thread_offsets = calculate_thread_y_offsets(frame);
        Self { thread_offsets }
    }
}

/// Calculate Y offsets for each thread in the flamegraph
pub fn calculate_thread_y_offsets(frame: &TraceFrame) -> BTreeMap<u64, f32> {
    let mut offsets = BTreeMap::new();
    let mut current_y = GRAPH_HEIGHT + TIMELINE_HEIGHT + THREAD_ROW_PADDING;

    // Sort threads: GPU (0) first, Main Thread (1) second, then workers
    let mut thread_ids: Vec<u64> = frame.threads.keys().copied().collect();
    thread_ids.sort_by_key(|id| match id {
        0 => (0, *id), // GPU first
        1 => (1, *id), // Main Thread second
        _ => (2, *id), // Workers after
    });

    for thread_id in thread_ids {
        // Calculate max depth for this thread
        let max_depth_for_thread = frame.spans
            .iter()
            .filter(|s| s.thread_id == thread_id)
            .map(|s| s.depth)
            .max()
            .unwrap_or(0);

        offsets.insert(thread_id, current_y);
        current_y += (max_depth_for_thread + 1) as f32 * ROW_HEIGHT + THREAD_ROW_PADDING;
    }

    offsets
}
