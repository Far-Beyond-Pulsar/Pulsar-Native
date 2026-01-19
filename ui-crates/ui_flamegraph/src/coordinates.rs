//! Coordinate transformation utilities for the flamegraph

use std::ops::Range;
use crate::trace_data::TraceFrame;
use crate::state::ViewState;
use crate::constants::THREAD_LABEL_WIDTH;

/// Convert a nanosecond timestamp to X coordinate
pub fn time_to_x(time_ns: u64, frame: &TraceFrame, viewport_width: f32, view_state: &ViewState) -> f32 {
    if frame.duration_ns() == 0 {
        return 0.0;
    }
    let normalized = (time_ns - frame.min_time_ns) as f32 / frame.duration_ns() as f32;
    (normalized * (viewport_width - THREAD_LABEL_WIDTH) * view_state.zoom) + view_state.pan_x + THREAD_LABEL_WIDTH
}

/// Calculate the visible time range based on current pan and zoom
pub fn visible_range(frame: &TraceFrame, viewport_width: f32, view_state: &ViewState) -> Range<u64> {
    if frame.duration_ns() == 0 {
        return 0..0;
    }

    let effective_width = viewport_width - THREAD_LABEL_WIDTH;

    // Calculate visible time range based on pan and zoom
    // Don't clamp to 0..1 here - allow wider range to prevent aggressive culling
    let normalized_start = (-(view_state.pan_x + effective_width * 0.5)) / (effective_width * view_state.zoom);
    let normalized_end = ((effective_width * 1.5 - view_state.pan_x) / (effective_width * view_state.zoom));

    let start_ns = (frame.min_time_ns as f64 + (normalized_start as f64 * frame.duration_ns() as f64)) as u64;
    let end_ns = (frame.min_time_ns as f64 + (normalized_end as f64 * frame.duration_ns() as f64)) as u64;

    // Allow rendering beyond visible bounds to prevent culling during pan/zoom
    let padding = frame.duration_ns() / 10; // 10% padding on each side
    start_ns.saturating_sub(padding)..end_ns.saturating_add(padding)
}
