//! View state management for the flamegraph viewer

use crate::constants::*;
use crate::lod_tree::LODTree;
use crate::trace_data::TraceFrame;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

/// View state for pan, zoom, and interaction
#[derive(Clone)]
pub struct ViewState {
    pub zoom: f32, // Pixels per nanosecond (absolute zoom)
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
    /// Span selected by double-click; drives the cross-thread dependency arrows.
    pub selected_span: Option<usize>,
    pub crop_dragging: bool,
    pub crop_start_time_ns: Option<u64>,
    pub crop_end_time_ns: Option<u64>,
    pub graph_dragging: bool,
    pub graph_drag_start_x: f32,

    // Track viewport width for absolute zoom initialization
    pub viewport_width: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            zoom: 0.0, // Will be initialized based on first frame
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
            selected_span: None,
            crop_dragging: false,
            crop_start_time_ns: None,
            crop_end_time_ns: None,
            graph_dragging: false,
            graph_drag_start_x: 0.0,
            viewport_width: 1000.0, // Default
        }
    }
}

/// Cache with pre-built GPU span data + hierarchical LOD tree.
/// Uses Arc - NO CLONING! All span data built once, zero per-frame iteration.
pub struct SpanCache {
    pub thread_offsets: Arc<BTreeMap<u64, f32>>,
    /// Display-ordered vertical layout: one row per thread, including its
    /// start-time-sorted span indices for O(log n) hover picking. Built in a
    /// single O(spans) pass together with `thread_offsets`.
    pub thread_rows: Arc<Vec<ThreadRowLayout>>,
    pub lod_tree: Arc<LODTree>,
    /// Span indices sorted by `end_ns`, for O(log n) window lookups when
    /// finding cross-thread wait/block dependencies on double-click.
    pub spans_sorted_by_end: Arc<Vec<u32>>,
    /// Self time of every span (duration minus its direct children).
    pub self_ns: Arc<Vec<u64>>,
}

impl SpanCache {
    pub fn build(frame: &TraceFrame) -> Self {
        let build_start = std::time::Instant::now();
        let (thread_rows, thread_offsets) = build_thread_rows(frame);
        let lod_tree = LODTree::build(frame, &thread_offsets);
        let mut sorted_by_end: Vec<u32> = (0..frame.spans.len() as u32).collect();
        sorted_by_end.sort_unstable_by_key(|i| frame.spans[*i as usize].end_ns());
        tracing::trace!(
            "[CACHE] {} spans, {} threads, {} rows in {:?}",
            frame.spans.len(),
            thread_offsets.len(),
            thread_rows.len(),
            build_start.elapsed(),
        );
        Self {
            thread_offsets: Arc::new(thread_offsets),
            thread_rows: Arc::new(thread_rows),
            lod_tree: Arc::new(lod_tree),
            spans_sorted_by_end: Arc::new(sorted_by_end),
            self_ns: Arc::new(crate::analysis::compute_self_times(frame)),
        }
    }
}

/// One vertical row in the flamegraph: a per-thread lane laid out in display
/// order (custom-named threads first, then by id).
#[derive(Debug, Clone)]
pub struct ThreadRow {
    /// World-space Y of the thread's depth-0 row (matches `thread_offsets`).
    pub y: f32,
    /// Total vertical extent of the row, including its trailing padding.
    pub height: f32,
}

/// A `ThreadRow` plus the span lookup state needed for O(log n) hover picking:
/// per-thread span indices sorted by start time.
pub struct ThreadRowLayout {
    pub row: ThreadRow,
    /// Indices into `TraceFrame::spans`, sorted by `start_ns`.
    pub span_indices: Vec<u32>,
}

/// Build the vertical layout once, in a single O(spans) pass.
///
/// The previous implementation recomputed each thread's max depth with a full
/// `frame.spans.iter().filter(...)` scan, i.e. O(threads * spans) — a
/// 30k-thread trace froze the viewer for minutes. All per-thread work here is
/// collected during one walk of the spans.
pub fn build_thread_rows(frame: &TraceFrame) -> (Vec<ThreadRowLayout>, BTreeMap<u64, f32>) {
    let mut max_depth: HashMap<u64, u32> = HashMap::with_capacity(frame.threads.len().min(4096));
    let mut span_lists: HashMap<u64, Vec<u32>> =
        HashMap::with_capacity(frame.threads.len().min(4096));

    for (index, span) in frame.spans.iter().enumerate() {
        let depth = max_depth.entry(span.thread_id).or_insert(0);
        if span.depth > *depth {
            *depth = span.depth;
        }
        span_lists
            .entry(span.thread_id)
            .or_default()
            .push(index as u32);
    }

    // Display order: custom-named threads first, then unnamed threads by id.
    let mut ids: Vec<u64> = span_lists.keys().copied().collect();
    ids.sort_by_key(|id| {
        let custom = frame
            .threads
            .get(id)
            .map(|t| !t.name.starts_with("Thread "))
            .unwrap_or(false);
        (!custom, *id)
    });

    let mut current_y = GRAPH_HEIGHT + TIMELINE_HEIGHT + THREAD_ROW_PADDING;
    let mut rows = Vec::with_capacity(ids.len());
    let mut offsets = BTreeMap::new();
    for id in ids {
        let depth = max_depth.get(&id).copied().unwrap_or(0);
        let height = (depth as f32 + 1.0) * ROW_HEIGHT + THREAD_ROW_PADDING;
        let mut indices = span_lists.remove(&id).unwrap_or_default();
        // Sort by start time so hover picking can binary-search within a row.
        indices.sort_unstable_by_key(|i| frame.spans[*i as usize].start_ns);
        rows.push(ThreadRowLayout {
            row: ThreadRow {
                y: current_y,
                height,
            },
            span_indices: indices,
        });
        offsets.insert(id, current_y);
        current_y += height;
    }

    (rows, offsets)
}

pub const TILE_ROW_HEIGHT: f32 = ROW_HEIGHT * 8.0;
