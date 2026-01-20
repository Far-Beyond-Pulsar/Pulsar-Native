//! Hierarchical LOD tree with pre-merged spans at each level
//! Query complexity: O(visible_output_size), NOT O(total_data_size)

use std::collections::BTreeMap;
use crate::trace_data::{TraceFrame, TraceSpan};
use crate::constants::ROW_HEIGHT;

/// Pre-merged span ready for rendering
#[derive(Clone, Debug)]
pub struct MergedSpan {
    pub start_ns: u64,
    pub end_ns: u64,
    pub y: f32,
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
        ((time_ns - self.time_min) / self.bucket_size_ns).min((self.num_buckets - 1) as u64) as usize
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
                    thread_id: span.thread_id,
                    depth: span.depth,
                    color_index: span.color_index as usize,
                    span_count: 1,
                };

                self.buckets[bucket_idx]
                    .entry(key)
                    .or_insert_with(Vec::new)
                    .push(merged);
            }
        }

        // Merge adjacent spans within each bucket
        for bucket in &mut self.buckets {
            for spans_list in bucket.values_mut() {
                spans_list.sort_by_key(|s| s.start_ns);

                let mut i = 0;
                while i < spans_list.len() {
                    let mut j = i + 1;
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

    /// Query spans in time range - O(output_size)!
    fn query(&self, time_start: u64, time_end: u64, y_min: f32, y_max: f32, result: &mut Vec<MergedSpan>) {
        let start_bucket = self.bucket_index(time_start);
        let end_bucket = self.bucket_index(time_end);

        for bucket_idx in start_bucket..=end_bucket.min(self.num_buckets - 1) {
            for spans in self.buckets[bucket_idx].values() {
                for span in spans {
                    // RELAXED time culling - allow spans that are partially visible
                    // Don't cull if there's ANY overlap with visible range
                    // This prevents spans from disappearing during pan/zoom
                    if span.end_ns < time_start || span.start_ns > time_end {
                        // But also check if span is within bucket range
                        // (bucket iteration already gives us locality)
                        continue;
                    }
                    // Y culling - keep this strict
                    if span.y + ROW_HEIGHT < y_min || span.y > y_max {
                        continue;
                    }
                    result.push(span.clone());
                }
            }
        }
    }
}

/// Hierarchical LOD tree - multiple levels from fine to coarse
/// NOT cloneable - use Arc to share!
pub struct LODTree {
    levels: Vec<LODLevel>,
}

impl LODTree {
    /// Build LOD hierarchy from trace data
    pub fn build(frame: &TraceFrame, thread_offsets: &BTreeMap<u64, f32>) -> Self {
        let build_start = std::time::Instant::now();

        let time_min = frame.min_time_ns;
        let time_max = frame.min_time_ns + frame.duration_ns().max(1);

        // Create multiple LOD levels with increasing bucket sizes
        let bucket_sizes = vec![
            50_000,      // 0.05ms - ultra fine (zoomed in 100x+)
            100_000,     // 0.1ms  - very fine (zoomed in 20-100x)
            500_000,     // 0.5ms  - fine (zoomed in 5-20x)
            1_000_000,   // 1ms    - medium (zoomed in 2-5x)
            5_000_000,   // 5ms    - coarse (normal view)
            10_000_000,  // 10ms   - very coarse (zoomed out 2-5x)
            50_000_000,  // 50ms   - ultra coarse (zoomed out 5x+)
        ];

        let mut levels = Vec::new();
        for &bucket_size in &bucket_sizes {
            let mut level = LODLevel::new(time_min, time_max, bucket_size);
            level.add_spans(&frame.spans, thread_offsets);
            levels.push(level);
        }

        Self { levels }
    }

    /// Select best LOD level based on zoom (pixels per nanosecond)
    fn select_level(&self, pixels_per_ns: f64) -> usize {
        // Want ~2+ pixels per merged span for visibility
        // pixels_per_ns * bucket_size_ns >= 2.0
        // bucket_size_ns <= 2.0 / pixels_per_ns

        let ideal_bucket_size = (2.0 / pixels_per_ns) as u64;

        // Find level with bucket size closest to (but not larger than) ideal
        let mut best_level = 0;
        let mut best_diff = u64::MAX;

        for (i, level) in self.levels.iter().enumerate() {
            if level.bucket_size_ns <= ideal_bucket_size {
                let diff = ideal_bucket_size - level.bucket_size_ns;
                if diff < best_diff {
                    best_diff = diff;
                    best_level = i;
                }
            }
        }

        best_level
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
        let visible_duration = (time_end - time_start) as f64;
        let pixels_per_ns = viewport_width as f64 / visible_duration;

        let level_idx = self.select_level(pixels_per_ns);
        let mut result = Vec::new();

        self.levels[level_idx].query(time_start, time_end, y_min, y_max, &mut result);

        result
    }
}
