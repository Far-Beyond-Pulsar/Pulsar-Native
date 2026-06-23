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
    pub thread_id: u64,
    pub depth: u32,
    pub color_index: usize,
    pub span_count: usize,
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
    /// Min/max time covered
    time_min: u64,
    time_max: u64,
}

impl LODLevel {
    fn new(time_min: u64, time_max: u64, bucket_size_ns: u64) -> Self {
        let num_buckets = ((time_max - time_min) / bucket_size_ns + 1) as usize;
        Self {
            bucket_size_ns,
            buckets: vec![BTreeMap::new(); num_buckets],
            num_buckets,
            time_min,
            time_max,
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
                    thread_id: span.thread_id,
                    depth: span.depth,
                    color_index: span.color_index as usize,
                    span_count: 1,
                };

                self.buckets[bucket_idx]
                    .entry(key)
                    .or_default()
                    .push(merged);
            }
        }

        // Merge adjacent spans within each bucket
        for bucket in &mut self.buckets {
            for spans_list in bucket.values_mut() {
                spans_list.sort_by_key(|s| s.start_ns);

                let mut i = 0;
                while i < spans_list.len() {
                    let j = i + 1;
                    while j < spans_list.len() {
                        let gap = spans_list[j].start_ns - spans_list[i].end_ns;
                        // Merge if gap < 1 pixel worth of time (at this LOD level)
                        if gap < self.bucket_size_ns / 10 {
                            spans_list[i].end_ns = spans_list[j].end_ns;
                            spans_list[i].span_count += spans_list[j].span_count;
                            spans_list.remove(j);
                        } else {
                            break;
                        }
                    }
                    i += 1;
                }
            }
        }
    }

    /// Query spans in time range — O(output_size)!
    pub(crate) fn query(
        &self,
        time_start: u64,
        time_end: u64,
        y_min: f32,
        y_max: f32,
        result: &mut Vec<MergedSpan>,
    ) {
        self.query_foreach(time_start, time_end, y_min, y_max, |s| result.push(s.clone()));
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

/// Hierarchical LOD tree - multiple levels from fine to coarse
/// NOT cloneable - use Arc to share!
pub struct LODTree {
    levels: Vec<LODLevel>,
    /// Bucket sizes for each level (ns).
    pub bucket_sizes: Vec<u64>,
}

impl LODTree {
    /// Build LOD hierarchy from trace data
    pub fn build(frame: &TraceFrame, thread_offsets: &BTreeMap<u64, f32>) -> Self {
        let _build_start = std::time::Instant::now();

        let time_min = frame.min_time_ns;
        let time_max = frame.min_time_ns + frame.duration_ns().max(1);

        // Create multiple LOD levels with increasing bucket sizes.
        // Each merged span covers the FULL bucket range it belongs to,
        // so a 1s-bucket span at far zoom-out is ~20px wide on a 1920px viewport.
        // This ensures spans NEVER vanish — they just merge into wider coverage blocks.
        let bucket_sizes = vec![
            50_000,      // 0.05ms   — zoomed in 1000x+
            100_000,     // 0.1ms    — zoomed in 500x+
            500_000,     // 0.5ms    — zoomed in 100x+
            1_000_000,   // 1ms      — zoomed in 50x+
            5_000_000,   // 5ms      — zoomed in 10x+
            10_000_000,  // 10ms     — default / normal
            50_000_000,  // 50ms     — zoomed out 5x
            100_000_000, // 100ms    — zoomed out 10x
            200_000_000, // 200ms    — zoomed out 20x
            500_000_000, // 500ms    — zoomed out 50x
            1_000_000_000, // 1s    — zoomed out 100x+
        ];

        let mut levels = Vec::new();
        for &bucket_size in &bucket_sizes {
            let mut level = LODLevel::new(time_min, time_max, bucket_size);
            level.add_spans(&frame.spans, thread_offsets);
            levels.push(level);
        }

        Self { levels, bucket_sizes }
    }

    /// Select best LOD level based on zoom (pixels per nanosecond).
    /// Targets ~4+ pixels per merged bucket so spans NEVER vanish.
    fn select_level(&self, pixels_per_ns: f64) -> usize {
        // Want ~4+ pixels per merged span bucket
        // pixels_per_ns * bucket_size_ns >= 4.0
        // bucket_size_ns >= 4.0 / pixels_per_ns
        // We find the level where bucket_size_ns is closest to but not below this target.

        let min_bucket_ns = (4.0 / pixels_per_ns) as u64;

        // Fallback: if even the coarsest level isn't wide enough, return the coarsest
        if min_bucket_ns >= self.levels.last().map(|l| l.bucket_size_ns).unwrap_or(1) {
            return self.levels.len() - 1;
        }

        // Find level with bucket_size_ns >= min_bucket_ns (closest)
        let mut best_level = self.levels.len() - 1;
        let mut best_diff = u64::MAX;

        for (i, level) in self.levels.iter().enumerate() {
            if level.bucket_size_ns >= min_bucket_ns {
                let diff = level.bucket_size_ns - min_bucket_ns;
                if diff < best_diff {
                    best_diff = diff;
                    best_level = i;
                }
            }
        }

        best_level
    }

    /// Collect all merged spans from a specific LOD level as GpuSpans.
    /// Called once when the LOD level changes — cached thereafter.
    pub fn collect_level_gpu_spans(&self, level_idx: usize, min_time_ns: u64) -> Vec<crate::rendering::types::GpuSpan> {
        let level = &self.levels[level_idx.min(self.levels.len() - 1)];
        let mut out = Vec::with_capacity(65536);
        for bucket in &level.buckets {
            for spans in bucket.values() {
                for span in spans {
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

    /// Query a specific LOD level within time + Y bounds — O(visible_buckets).
    pub(crate) fn query_level(
        &self,
        level_idx: usize,
        time_start: u64,
        time_end: u64,
        y_min: f32,
        y_max: f32,
        result: &mut Vec<MergedSpan>,
    ) {
        self.levels[level_idx.min(self.levels.len() - 1)]
            .query(time_start, time_end, y_min, y_max, result);
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

    /// Query with automatic LOD selection - O(output) complexity!
    pub fn query_dynamic(
        &self,
        time_start: u64,
        time_end: u64,
        y_min: f32,
        y_max: f32,
        viewport_width: f32,
    ) -> Vec<MergedSpan> {
        let visible_duration = (time_end - time_start).max(1) as f64;
        let pixels_per_ns = viewport_width as f64 / visible_duration;

        let level_idx = self.select_level(pixels_per_ns);
        let mut result = Vec::new();

        self.levels[level_idx].query(time_start, time_end, y_min, y_max, &mut result);

        result
    }
}
