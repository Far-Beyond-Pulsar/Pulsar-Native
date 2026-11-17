//! Hierarchical LOD tree with pre-merged spans at each level
//! Query complexity: O(visible_output_size), NOT O(total_data_size)

use crate::constants::ROW_HEIGHT;
use crate::trace_data::{TraceFrame, TraceSpan};
use std::collections::BTreeMap;

/// Pre-merged span ready for rendering
#[derive(Clone, Debug)]
pub struct MergedSpan {
    pub start_ns: u64,
    pub end_ns: u64,
    pub y: f32,
    pub label: String,
    pub depth: u32,
    pub color_index: usize,
    pub span_count: usize,
    /// Duration of the member whose label/color this block currently shows.
    pub label_dur_ns: u64,
}

/// One level in the LOD hierarchy
#[derive(Clone)]
struct LODLevel {
    /// Time bucket size in nanoseconds (e.g., 100_000 = 0.1ms)
    bucket_size_ns: u64,
    /// Buckets indexed by: bucket_index -> (thread_id, depth) -> merged spans
    buckets: Vec<BTreeMap<(u64, u32), Vec<MergedSpan>>>,
    /// Number of buckets
    num_buckets: usize,
    /// Start of the time covered
    time_min: u64,
}

impl LODLevel {
    fn new(time_min: u64, time_max: u64, bucket_size_ns: u64) -> Self {
        let num_buckets = ((time_max - time_min) / bucket_size_ns + 1) as usize;
        Self {
            bucket_size_ns,
            buckets: vec![BTreeMap::new(); num_buckets],
            num_buckets,
            time_min,
        }
    }

    fn bucket_index(&self, time_ns: u64) -> usize {
        ((time_ns - self.time_min) / self.bucket_size_ns).min((self.num_buckets - 1) as u64)
            as usize
    }

    /// Add spans from original data, merging adjacent ones in same bucket
    fn add_spans(&mut self, spans: &[TraceSpan], thread_offsets: &BTreeMap<u64, f32>) {
        for span in spans {
            if let Some(&y_offset) = thread_offsets.get(&span.thread_id) {
                let bucket_idx = self.bucket_index(span.start_ns);
                let y = y_offset + (span.depth as f32 * ROW_HEIGHT);
                let key = (span.thread_id, span.depth);

                let merged = MergedSpan {
                    start_ns: span.start_ns,
                    end_ns: span.end_ns(),
                    y,
                    label: span.name.clone(),
                    depth: span.depth,
                    color_index: span.color_index as usize,
                    span_count: 1,
                    label_dur_ns: span.end_ns().saturating_sub(span.start_ns),
                };

                self.buckets[bucket_idx]
                    .entry(key)
                    .or_default()
                    .push(merged);
            }
        }

        // Merge adjacent spans within each bucket (single pass, no removals —
        // the previous `spans_list.remove(j)` rebuild loop was O(n^2) inside a
        // hot bucket).
        for bucket in &mut self.buckets {
            for spans_list in bucket.values_mut() {
                spans_list.sort_by_key(|s| s.start_ns);

                let mut merged: Vec<MergedSpan> = Vec::with_capacity(spans_list.len());
                for span in spans_list.drain(..) {
                    if let Some(last) = merged.last_mut() {
                        let gap = span.start_ns.saturating_sub(last.end_ns);
                        // Merge if gap < 1 pixel worth of time (at this LOD level)
                        if gap < self.bucket_size_ns / 10 {
                            // Name the block after its longest member, not the
                            // first one: a ~0ms lock wait that happens to start
                            // the row must not label the frame it sits beside.
                            if span.label_dur_ns > last.label_dur_ns {
                                last.label = span.label;
                                last.color_index = span.color_index;
                                last.label_dur_ns = span.label_dur_ns;
                            }
                            last.end_ns = last.end_ns.max(span.end_ns);
                            last.span_count += span.span_count;
                            continue;
                        }
                    }
                    merged.push(span);
                }
                *spans_list = merged;
            }
        }
    }

    /// Walk visible buckets with a callback — avoids cloning spans into a Vec.
    pub(crate) fn query_foreach(
        &self,
        time_start: u64,
        time_end: u64,
        y_min: f32,
        y_max: f32,
        mut f: impl FnMut(&MergedSpan),
    ) {
        let start_bucket = self.bucket_index(time_start);
        let end_bucket = self.bucket_index(time_end);

        for bucket_idx in start_bucket..=end_bucket.min(self.num_buckets - 1) {
            for spans in self.buckets[bucket_idx].values() {
                for span in spans {
                    if span.end_ns < time_start || span.start_ns > time_end {
                        continue;
                    }
                    if span.y + ROW_HEIGHT < y_min || span.y > y_max {
                        continue;
                    }
                    f(span);
                }
            }
        }
    }
}

/// Bucket size of each LOD level (ns), from fine to coarse.
///
/// Each merged span covers the FULL bucket range it belongs to, so a 1s-bucket
/// span at far zoom-out is ~20px wide on a 1920px viewport. This ensures spans
/// NEVER vanish — they just merge into wider coverage blocks.
pub const BUCKET_SIZES_NS: [u64; 11] = [
    50_000,        // 0.05ms   — zoomed in 1000x+
    100_000,       // 0.1ms    — zoomed in 500x+
    500_000,       // 0.5ms    — zoomed in 100x+
    1_000_000,     // 1ms      — zoomed in 50x+
    5_000_000,     // 5ms      — zoomed in 10x+
    10_000_000,    // 10ms     — default / normal
    50_000_000,    // 50ms     — zoomed out 5x
    100_000_000,   // 100ms    — zoomed out 10x
    200_000_000,   // 200ms    — zoomed out 20x
    500_000_000,   // 500ms    — zoomed out 50x
    1_000_000_000, // 1s    — zoomed out 100x+
];

/// Hierarchical LOD tree - multiple levels from fine to coarse
/// NOT cloneable - use Arc to share!
pub struct LODTree {
    /// One level per entry of [`BUCKET_SIZES_NS`].
    levels: Vec<LODLevel>,
}

impl LODTree {
    /// Build LOD hierarchy from trace data
    pub fn build(frame: &TraceFrame, thread_offsets: &BTreeMap<u64, f32>) -> Self {
        let _build_start = std::time::Instant::now();

        let time_min = frame.min_time_ns;
        let time_max = frame.min_time_ns + frame.duration_ns().max(1);

        let mut levels = Vec::new();
        for &bucket_size in &BUCKET_SIZES_NS {
            let mut level = LODLevel::new(time_min, time_max, bucket_size);
            level.add_spans(&frame.spans, thread_offsets);
            levels.push(level);
        }

        Self { levels }
    }

    /// Select the LOD level for a zoom (pixels per nanosecond): the finest
    /// level whose buckets are ~4+ pixels wide, so spans NEVER vanish.
    pub fn level_for(pixels_per_ns: f64) -> usize {
        let min_bucket_ns = (4.0 / pixels_per_ns) as u64;
        let coarsest = BUCKET_SIZES_NS.len() - 1;
        // Even the coarsest level isn't wide enough.
        if min_bucket_ns >= BUCKET_SIZES_NS[coarsest] {
            return coarsest;
        }
        let mut best_level = coarsest;
        let mut best_diff = u64::MAX;
        for (i, &bucket_ns) in BUCKET_SIZES_NS.iter().enumerate() {
            if bucket_ns >= min_bucket_ns {
                let diff = bucket_ns - min_bucket_ns;
                if diff < best_diff {
                    best_diff = diff;
                    best_level = i;
                }
            }
        }
        best_level
    }

    /// Collect merged spans from a specific LOD level as GpuSpans, restricted
    /// to a vertical world-Y window. This is the virtualized path: only the
    /// thread rows currently in view are uploaded, so a trace with thousands
    /// of threads never builds a gigabyte vertex buffer.
    /// Called when the LOD level or the vertical window changes.
    pub fn collect_level_gpu_spans_ybounded(
        &self,
        level_idx: usize,
        min_time_ns: u64,
        y_min: f32,
        y_max: f32,
    ) -> Vec<crate::rendering::types::GpuSpan> {
        let level = &self.levels[level_idx.min(self.levels.len() - 1)];
        let mut out = Vec::with_capacity(65536);
        for bucket in &level.buckets {
            for spans in bucket.values() {
                for span in spans {
                    if span.y + ROW_HEIGHT < y_min || span.y > y_max {
                        continue;
                    }
                    out.push(crate::rendering::types::GpuSpan {
                        start_rel_ns: (span.start_ns - min_time_ns) as f32,
                        end_rel_ns: (span.end_ns - min_time_ns) as f32,
                        y: span.y,
                        color_index: span.color_index as u32,
                        span_count: span.span_count as u32,
                        depth: span.depth,
                        _pad: [0; 2],
                    });
                }
            }
        }
        out
    }

    /// Walk visible buckets at a specific LOD level with a callback — no allocation.
    pub(crate) fn query_level_foreach(
        &self,
        level_idx: usize,
        time_start: u64,
        time_end: u64,
        y_min: f32,
        y_max: f32,
        f: impl FnMut(&MergedSpan),
    ) {
        self.levels[level_idx.min(self.levels.len() - 1)]
            .query_foreach(time_start, time_end, y_min, y_max, f);
    }
}
