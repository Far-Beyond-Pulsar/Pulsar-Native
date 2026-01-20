//! Coordinate transformation utilities for the flamegraph

use std::ops::Range;
use crate::trace_data::TraceFrame;
use crate::state::ViewState;
use crate::constants::THREAD_LABEL_WIDTH;

/// Convert a nanosecond timestamp to X coordinate using absolute zoom (pixels per nanosecond)
pub fn time_to_x(time_ns: u64, frame: &TraceFrame, _viewport_width: f32, view_state: &ViewState) -> f32 {
    if frame.duration_ns() == 0 {
        return THREAD_LABEL_WIDTH;
    }
    
    // Initialize zoom if not set (first frame)
    let zoom = if view_state.zoom == 0.0 {
        // Fit entire duration into viewport width
        let effective_width = view_state.viewport_width - THREAD_LABEL_WIDTH;
        effective_width / frame.duration_ns() as f32
    } else {
        view_state.zoom
    };
    
    // Absolute coordinate: time offset * pixels per nanosecond
    let time_offset = (time_ns - frame.min_time_ns) as f32;
    (time_offset * zoom) + view_state.pan_x + THREAD_LABEL_WIDTH
}

/// Calculate the visible time range based on current pan and zoom
pub fn visible_range(frame: &TraceFrame, viewport_width: f32, view_state: &ViewState) -> Range<u64> {
    if frame.duration_ns() == 0 {
        return 0..0;
    }

    let effective_width = viewport_width - THREAD_LABEL_WIDTH;

    // Initialize zoom if not set
    let zoom = if view_state.zoom == 0.0 {
        effective_width / frame.duration_ns() as f32
    } else {
        view_state.zoom
    };

    // Calculate visible time range in nanoseconds
    // pan_x is in pixels offset from left edge
    // Visible range is from -pan_x to (viewport_width - pan_x) in pixel space
    // Convert to nanoseconds by dividing by zoom (pixels per nanosecond)
    
    let left_edge_ns = (-view_state.pan_x) / zoom;
    let right_edge_ns = (effective_width - view_state.pan_x) / zoom;
    
    let start_ns = (frame.min_time_ns as f64 + left_edge_ns as f64) as u64;
    let end_ns = (frame.min_time_ns as f64 + right_edge_ns as f64) as u64;

    // CRITICAL: Add HUGE padding to prevent culling during pan/zoom
    // Use 100% of visible range as padding on EACH side
    // This ensures spans are loaded well before they come into view
    let visible_duration = end_ns.saturating_sub(start_ns);
    let padding = visible_duration.max(frame.duration_ns() / 5);
    
    let padded_start = start_ns.saturating_sub(padding).max(frame.min_time_ns);
    let padded_end = end_ns.saturating_add(padding).min(frame.min_time_ns + frame.duration_ns());
    
    padded_start..padded_end
}
