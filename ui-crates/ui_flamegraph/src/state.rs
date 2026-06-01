//! View state management for the flamegraph viewer

use crate::constants::*;
use crate::lod_tree::LODTree;
use crate::lod_tree::MergedSpan;
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
            crop_dragging: false,
            crop_start_time_ns: None,
            crop_end_time_ns: None,
            graph_dragging: false,
            graph_drag_start_x: 0.0,
            viewport_width: 1000.0, // Default
        }
    }
}



impl Rect {
    fn intersects(&self, other: &Rect) -> bool {
        self.x_min <= other.x_max
            && self.x_max >= other.x_min
            && self.y_min <= other.y_max
            && self.y_max >= other.y_min
    }

    fn contains(&self, other: &Rect) -> bool {
        other.x_min >= self.x_min
            && other.x_max <= self.x_max
            && other.y_min >= self.y_min
            && other.y_max <= self.y_max
    }
}

/// Cache with hierarchical LOD tree - O(output) queries!
/// Uses Arc - NO CLONING!
pub struct SpanCache {
    pub thread_offsets: Arc<BTreeMap<u64, f32>>,
    pub lod_tree: Arc<LODTree>,
    pub tile_cache: Arc<parking_lot::Mutex<SpanTileCache>>,
}

impl SpanCache {
    pub fn build(frame: &TraceFrame) -> Self {
        let build_start = std::time::Instant::now();
        let thread_offsets = calculate_thread_y_offsets(frame);
        let lod_tree = LODTree::build(frame, &thread_offsets);
        tracing::trace!("[CACHE] total cache build: {:?}", build_start.elapsed());
        Self {
            thread_offsets: Arc::new(thread_offsets),
            lod_tree: Arc::new(lod_tree),
            tile_cache: Arc::new(parking_lot::Mutex::new(SpanTileCache::new())),
        }
    }
}

pub const TILE_TIME_NS: u64 = 8_000_000;
pub const TILE_ROW_HEIGHT: f32 = ROW_HEIGHT * 8.0;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TileKey {
    time_index: i64,
    row_index: i32,
    zoom_bucket: u64,
}

/// Cache of rendered span tiles keyed by world-space buckets.
pub struct SpanTileCache {
    tiles: HashMap<TileKey, Arc<Vec<MergedSpan>>>,
}

impl SpanTileCache {
    pub fn new() -> Self {
        Self {
            tiles: HashMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.tiles.clear();
    }

    pub fn get_or_build_tile(
        &mut self,
        frame: &TraceFrame,
        lod_tree: &LODTree,
        time_index: i64,
        row_index: i32,
        zoom: f32,
    ) -> Arc<Vec<MergedSpan>> {
        let zoom_bucket = (zoom.max(0.0001) * 1024.0).round() as u64;
        let key = TileKey {
            time_index,
            row_index,
            zoom_bucket,
        };

        if let Some(cached) = self.tiles.get(&key) {
            return Arc::clone(cached);
        }

        let tile_time_start = frame
            .min_time_ns
            .saturating_add((time_index.max(0) as u64).saturating_mul(TILE_TIME_NS));
        let tile_time_end = frame
            .min_time_ns
            .saturating_add(((time_index.max(0) as u64) + 1).saturating_mul(TILE_TIME_NS))
            .min(frame.min_time_ns + frame.duration_ns());

        let tile_y_min = (row_index.max(0) as f32) * TILE_ROW_HEIGHT;
        let tile_y_max = tile_y_min + TILE_ROW_HEIGHT;
        let tile_width_px = TILE_TIME_NS as f32 * zoom.max(0.0001);

        let spans = lod_tree.query_dynamic(
            tile_time_start,
            tile_time_end,
            tile_y_min,
            tile_y_max,
            tile_width_px.max(1.0),
        );

        let spans = Arc::new(spans);
        self.tiles.insert(key, Arc::clone(&spans));
        spans
    }
}

/// Calculate Y offsets for each thread in the flamegraph
pub fn calculate_thread_y_offsets(frame: &TraceFrame) -> BTreeMap<u64, f32> {
    let mut offsets = BTreeMap::new();
    let mut current_y = GRAPH_HEIGHT + TIMELINE_HEIGHT + THREAD_ROW_PADDING;

    // Get threads sorted with named threads first, then by ID
    let sorted_threads = frame.get_sorted_threads();

    for thread_info in sorted_threads {
        let thread_id = thread_info.id;

        // Calculate max depth for this thread
        let max_depth_for_thread = frame
            .spans
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
