use crate::constants::*;
use crate::coordinates::time_to_x;
use crate::lod_tree::{LODTree, MergedSpan};
use crate::rendering::text::{push_text, CHAR_H, CHAR_W};
use crate::rendering::types::{GpuSpan, RectInstance};
use crate::state::{SpanCache, ViewState};
use crate::trace_data::TraceFrame;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

const LABEL_MIN_PX: f32 = 40.0;

/// Return the effective zoom, falling back to frame-fit if unset.
#[inline(always)]
fn effective_zoom(vs: &ViewState, viewport_w: f32, frame: &TraceFrame) -> f32 {
    if vs.zoom == 0.0 && frame.duration_ns() > 0 {
        viewport_w / frame.duration_ns() as f32
    } else {
        vs.zoom
    }
}

/// Minimum pixel gap between grid/ruler ticks.
/// Grows at far zoom-out so density decreases — at default zoom ≈60px,
/// at extreme zoom-out ≈300px (so only 5-8 lines across the viewport).
#[inline(always)]
fn tick_min_px(zoom: f32) -> f32 {
    // t = 1.0 at default zoom (2e-6), → 0.0 at very far zoom
    let t = (zoom * 5.0e5).min(1.0);
    60.0 * (1.0 + (1.0 - t) * 4.0)
}

// ── GPU span passthrough ──────────────────────────────────────────────────

/// No-op: GpuSpans are pre-built once during SpanCache construction.
/// Returns the pre-built data as-is for GPU vertex-pulling.
pub fn build_instances(spans: &[GpuSpan]) -> &[GpuSpan] {
    spans
}

// ── Ruler (RectInstance overlays) ─────────────────────────────────────────

/// Build ruler tick + label instances.  Ticks are ≥ 60 px apart.
pub fn build_ruler_instances(
    frame: &TraceFrame,
    vs: &ViewState,
    surface_w: f32,
) -> Vec<RectInstance> {
    let mut rects = Vec::new();
    if frame.duration_ns() == 0 {
        return rects;
    }

    let vr = crate::coordinates::visible_range(frame, surface_w, vs);
    let zoom = effective_zoom(vs, surface_w, frame);

    let min_px = tick_min_px(zoom);
    let target_step_ns = (min_px / zoom.max(1e-10)) as u64;
    let candidates: [u64; 16] = [
        1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 30000, 60000, 300000,
    ];
    let mut step_ms = 1u64;
    for &c in &candidates {
        if c * 1_000_000 >= target_step_ns {
            step_ms = c;
            break;
        }
    }
    // Fallback: if no candidate matched, use the largest
    if step_ms == 1 && target_step_ns > 1_000_000 {
        step_ms = *candidates.last().unwrap();
    }
    let step_ns = step_ms * 1_000_000;

    // Major ticks
    let first = (vr.start / step_ns) * step_ns;
    let mut t = first;
    while t <= vr.end {
        if t >= frame.min_time_ns {
            let x = time_to_x(t, frame, surface_w, vs);
            if x >= THREAD_LABEL_WIDTH && x <= surface_w {
                rects.push(RectInstance {
                    pos: [x, TIMELINE_HEIGHT - 8.0],
                    size: [1.0, 8.0],
                    color: [0.5, 0.5, 0.5, 0.6],
                    kind: 0,
                    rot: 0,
                    _pad: [0; 2],
                });

                let ms = (t - frame.min_time_ns) as f64 / 1_000_000.0;
                let label = if ms >= 1000.0 && step_ms >= 500 {
                    format!("{:.1}s", ms / 1000.0)
                } else if ms >= 100.0 || step_ms >= 50 {
                    format!("{:.0}ms", ms)
                } else if ms >= 10.0 {
                    format!("{:.1}ms", ms)
                } else {
                    format!("{:.2}ms", ms)
                };
                push_text(
                    &label,
                    x + 3.0,
                    TIMELINE_HEIGHT - CHAR_H - 1.0,
                    [0.6, 0.6, 0.6, 0.8],
                    1.0,
                    &mut rects,
                );
            }
        }
        t += step_ns;
    }

    // Minor ticks (fifth of major)
    let minor_step = (step_ns / 5).max(1_000_000);
    let mfirst = (vr.start / minor_step) * minor_step;
    let mut mt = mfirst;
    while mt <= vr.end {
        if mt >= frame.min_time_ns && mt % step_ns != 0 {
            let x = time_to_x(mt, frame, surface_w, vs);
            if x >= THREAD_LABEL_WIDTH && x <= surface_w {
                rects.push(RectInstance {
                    pos: [x, TIMELINE_HEIGHT - 4.0],
                    size: [1.0, 4.0],
                    color: [0.5, 0.5, 0.5, 0.3],
                    kind: 0,
                    rot: 0,
                    _pad: [0; 2],
                });
            }
        }
        mt += minor_step;
    }

    rects
}

// ── Text labels ───────────────────────────────────────────────────────────

/// Visible time range with small tolerance — avoids edge rounding / underflow
/// without the 100%+ padding that caused 80K-bucket walks.
fn visible_range_tight(frame: &TraceFrame, viewport_w: f32, vs: &ViewState) -> Range<u64> {
    if frame.duration_ns() == 0 {
        return 0..0;
    }
    let effective_w = viewport_w - THREAD_LABEL_WIDTH;
    let zoom = if vs.zoom == 0.0 {
        effective_w / frame.duration_ns() as f32
    } else {
        vs.zoom
    };
    let left_ns = (-vs.pan_x as f64) / zoom as f64;
    let right_ns = (effective_w as f64 - vs.pan_x as f64) / zoom as f64;
    let tol = ((right_ns - left_ns) * 0.05).max(50_000.0);
    let start = ((frame.min_time_ns as f64 + left_ns - tol).max(frame.min_time_ns as f64)) as u64;
    let end = (frame.min_time_ns as f64 + right_ns + tol) as u64;
    start..end
}

/// Build text label rects from the LOD tree — precisely culled to viewport.
/// Labels appear as soon as the block is wide enough for ~5 characters.
pub fn build_text_instances(
    frame: &TraceFrame,
    lod_tree: &LODTree,
    level_idx: usize,
    vs: &ViewState,
    viewport_w: f32,
    viewport_h: f32,
) -> Vec<RectInstance> {
    let mut rects = Vec::new();

    // Use tight visible range (5% tolerance) — the padded version causes 80K-bucket walks
    let vr = visible_range_tight(frame, viewport_w, vs);
    if vr.start >= vr.end {
        return rects;
    }

    let zoom = if vs.zoom == 0.0 {
        let ew = viewport_w - THREAD_LABEL_WIDTH;
        ew / frame.duration_ns().max(1) as f32
    } else {
        vs.zoom
    };

    let y_adj = -GRAPH_HEIGHT;
    let y_min_lod = -y_adj - vs.pan_y - ROW_HEIGHT;
    let y_max_lod = viewport_h - y_adj - vs.pan_y;

    // Relaxed threshold — merged spans can cover 5+ buckets, so use 20% of the strict value
    let min_dur_ns = (LABEL_MIN_PX / zoom.max(1e-10) * 0.20) as u64;

    lod_tree.query_level_foreach(level_idx, vr.start, vr.end, y_min_lod, y_max_lod, |ms| {
        // Pre-check: skip clearly too-short spans
        let dur = ms.end_ns - ms.start_ns;
        if dur < min_dur_ns {
            return;
        }

        let x1 = time_to_x(ms.start_ns, frame, viewport_w, vs);
        let x2 = time_to_x(ms.end_ns, frame, viewport_w, vs);
        let rw = x2 - x1;

        if rw >= LABEL_MIN_PX {
            let sy = ms.y + y_adj + vs.pan_y + PADDING;
            let sh = (ROW_HEIGHT - PADDING) * 0.8;
            let avail = rw - (PADDING * 4.0);
            let max_c = (avail / CHAR_W) as usize;

            if max_c >= 5 {
                let bytes = ms.label.as_bytes();
                let end = bytes.len().min(max_c);
                let label = if end >= bytes.len() {
                    &ms.label
                } else {
                    core::str::from_utf8(&bytes[..end]).unwrap_or(&ms.label)
                };
                push_text(
                    label,
                    x1 + PADDING + 1.0,
                    sy + (sh - CHAR_H) / 2.0,
                    [0.02, 0.02, 0.02, 0.92],
                    1.0,
                    &mut rects,
                );
            }
        }
    });

    rects
}

// ── Debug overlay ─────────────────────────────────────────────────────────

/// Build a debug overlay with rendering stats (top-left corner).
pub fn build_debug_overlay(
    frame: &TraceFrame,
    lod_tree: &LODTree,
    level_idx: usize,
    vs: &ViewState,
    viewport_w: f32,
) -> Vec<RectInstance> {
    let mut rects = Vec::new();
    let zoom = if vs.zoom == 0.0 {
        let ew = viewport_w - THREAD_LABEL_WIDTH;
        ew / frame.duration_ns().max(1) as f32
    } else {
        vs.zoom
    };
    let vis_ms = if viewport_w > 0.0 {
        let ew = viewport_w - THREAD_LABEL_WIDTH;
        ew / zoom.max(1e-10) / 1_000_000.0
    } else {
        0.0
    };
    let bs = lod_tree.bucket_sizes[level_idx.min(lod_tree.bucket_sizes.len() - 1)];
    let info = format!(
        "LOD:{}  z:{:.2e}  {:.1}ms  bs:{}μs  bucket_w:{:.0}px",
        level_idx,
        zoom,
        vis_ms,
        bs / 1000,
        bs as f32 * zoom,
    );
    push_text(&info, 4.0, 4.0, [0.0, 1.0, 0.0, 0.9], 1.0, &mut rects);
    rects
}

// ── Overlays (grid lines + thread separators) ────────────────────────────

/// Build grid lines and thread separators as RectInstance overlay.
pub fn build_overlay_instances(
    frame: &TraceFrame,
    thread_offsets: &BTreeMap<u64, f32>,
    vs: &ViewState,
    viewport_w: f32,
    viewport_h: f32,
) -> Vec<RectInstance> {
    let mut rects = Vec::new();
    if frame.spans.is_empty() {
        return rects;
    }

    let vr = crate::coordinates::visible_range(frame, viewport_w, vs);
    let zoom = effective_zoom(vs, viewport_w, frame);
    let y_adj = -GRAPH_HEIGHT;

    // Vertical grid lines — density decreases as zoom decreases
    let min_px = tick_min_px(zoom);
    let target_step_ns = (min_px / zoom.max(1e-10)) as u64;
    let candidates: [u64; 16] = [
        1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 30000, 60000, 300000,
    ];
    let mut step_ms = 1u64;
    for &c in &candidates {
        if c * 1_000_000 >= target_step_ns {
            step_ms = c;
            break;
        }
    }
    if step_ms == 1 && target_step_ns > 1_000_000 {
        step_ms = *candidates.last().unwrap();
    }
    let step_ns = step_ms * 1_000_000;
    let gfirst = (vr.start / step_ns) * step_ns;
    let mut gt = gfirst;
    while gt <= vr.end {
        if gt >= frame.min_time_ns {
            let x = time_to_x(gt, frame, viewport_w, vs);
            if x >= THREAD_LABEL_WIDTH && x <= viewport_w {
                rects.push(RectInstance {
                    pos: [x, 0.0],
                    size: [1.0, viewport_h],
                    color: [0.25, 0.25, 0.25, 0.15],
                    kind: 0,
                    rot: 0,
                    _pad: [0; 2],
                });
            }
        }
        gt += step_ns;
    }

    // Thread separators
    let mut idx = 0u32;
    for (_tid, y_off) in thread_offsets.iter() {
        if idx > 0 {
            let sy = y_off + y_adj + vs.pan_y;
            if sy >= 0.0 && sy < viewport_h {
                rects.push(RectInstance {
                    pos: [THREAD_LABEL_WIDTH, sy],
                    size: [viewport_w - THREAD_LABEL_WIDTH, 1.0],
                    color: [0.3, 0.3, 0.3, 0.3],
                    kind: 0,
                    rot: 0,
                    _pad: [0; 2],
                });
            }
        }
        idx += 1;
    }

    rects
}

// ── Frame boundary lines ──────────────────────────────────────────────────

/// Thin vertical lines marking where one frame ends and the next begins.
/// Each is drawn at the `__FRAME_MARKER__` start timestamp, culled to the
/// visible time range and de-noised by a minimum pixel spacing so far
/// zoom-out doesn't produce an unbroken wall.
pub fn build_frame_boundary_instances(
    frame: &TraceFrame,
    vs: &ViewState,
    viewport_w: f32,
    viewport_h: f32,
) -> Vec<RectInstance> {
    let mut rects = Vec::new();
    let mut last_x = f32::MIN;
    for &b in &frame.frame_boundaries_ns {
        let x = time_to_x(b, frame, viewport_w, vs);
        if x < THREAD_LABEL_WIDTH || x > viewport_w {
            continue;
        }
        if x - last_x < FRAME_LINE_MIN_SPACING {
            continue;
        }
        last_x = x;
        rects.push(RectInstance {
            pos: [x, 0.0],
            size: [1.0, viewport_h],
            color: FRAME_LINE_COLOR,
            kind: 0,
            rot: 0,
            _pad: [0; 2],
        });
    }
    rects
}

// ── Cross-thread dependency arrows ────────────────────────────────────────

/// Nanosecond tolerance for treating two cross-thread spans as "end-aligned",
/// meaning the later-starting one was gated on the earlier-starting one.
const WAIT_ALIGN_TOLERANCE_NS: u64 = 150_000;
/// Maximum number of dependency arrows rendered per double-click.
const MAX_DEPENDENCY_ARROWS: usize = 16;
/// Number of line segments used to tessellate each curved arrow.
const ARROW_SEGMENTS: usize = 24;
const ARROW_THICKNESS: f32 = 1.8;
const ARROW_HEAD_LEN: f32 = 12.0;
const ARROW_HEAD_HALF_WIDTH: f32 = 3.5;
const CURVE_BOW_MIN: f32 = 48.0;
const CURVE_BOW_MAX: f32 = 220.0;
const ARROW_COLOR: [f32; 4] = [0.95, 0.16, 0.14, 0.92];

#[inline(always)]
fn cubic_bezier(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    t: f32,
) -> (f32, f32) {
    let it = 1.0 - t;
    let a = it * it * it;
    let b = 3.0 * it * it * t;
    let c = 3.0 * it * t * t;
    let d = t * t * t;
    (
        a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
        a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
    )
}

/// Screen-space center of a span's box, if its thread row is laid out.
fn span_box_center(
    frame: &TraceFrame,
    thread_offsets: &BTreeMap<u64, f32>,
    vs: &ViewState,
    span_idx: usize,
    viewport_w: f32,
) -> Option<(f32, f32)> {
    let span = frame.spans.get(span_idx)?;
    let y0 = *thread_offsets.get(&span.thread_id)?;
    let y = y0 - GRAPH_HEIGHT + (span.depth as f32 * ROW_HEIGHT) + vs.pan_y
        + (ROW_HEIGHT - PADDING) * 0.4;
    let x1 = time_to_x(span.start_ns, frame, viewport_w, vs);
    let x2 = time_to_x(span.end_ns(), frame, viewport_w, vs);
    Some(((x1 + x2) * 0.5, y))
}

/// Push the tessellated "S" curve between two box centers plus a rotated
/// arrowhead at the destination. Sees red slightly translucent.
fn push_curve_arrow(s: (f32, f32), e: (f32, f32), rects: &mut Vec<RectInstance>) {
    let bow = ((e.1 - s.1).abs() * 0.45).clamp(CURVE_BOW_MIN, CURVE_BOW_MAX);
    let c1 = (s.0 + bow, s.1);
    let c2 = (e.0 - bow, e.1);

    let mut prev = s;
    let mut tangent = (1.0_f32, 0.0_f32);
    for i in 1..=ARROW_SEGMENTS {
        let t = i as f32 / ARROW_SEGMENTS as f32;
        let p = cubic_bezier(s, c1, c2, e, t);
        let seg = (p.0 - prev.0, p.1 - prev.1);
        let len = (seg.0 * seg.0 + seg.1 * seg.1).sqrt();
        if len > 0.001 {
            let angle = seg.1.atan2(seg.0);
            rects.push(RectInstance {
                pos: [(p.0 + prev.0) * 0.5, (p.1 + prev.1) * 0.5],
                size: [len, ARROW_THICKNESS],
                color: ARROW_COLOR,
                kind: 1,
                rot: angle.to_bits(),
                _pad: [0; 2],
            });
            tangent = seg;
        }
        prev = p;
    }

    // Arrowhead at the destination, pointing along the final tangent.
    let tlen = (tangent.0 * tangent.0 + tangent.1 * tangent.1).sqrt().max(1e-6);
    let dx = tangent.0 / tlen;
    let dy = tangent.1 / tlen;
    let angle = dy.atan2(dx);
    let hx = e.0 - dx * ARROW_HEAD_LEN * 0.5;
    let hy = e.1 - dy * ARROW_HEAD_LEN * 0.5;
    rects.push(RectInstance {
        pos: [hx, hy],
        size: [ARROW_HEAD_LEN, ARROW_HEAD_HALF_WIDTH],
        color: ARROW_COLOR,
        kind: 2,
        rot: angle.to_bits(),
        _pad: [0; 2],
    });
}

/// Build red curved arrows between the double-clicked span and cross-thread
/// spans that block it / wait on it. Returns an empty vec when no cross-thread
/// wait/block relationship is detectable.
///
/// Detection: only interval timing is available (no lock/mutex/waiter data), so
/// the heuristic is end-alignment — B is a dependency of A when B overlaps A in
/// time and B ends within `WAIT_ALIGN_TOLERANCE_NS` of A's end. The span that
/// started later and yet finishes exactly when the earlier one finishes is the
/// waiter (it was gated until the earlier span released it); the earlier one is
/// the block/owner. Arrows point from waiter → blocked-upon span.
pub fn build_dependency_arrows(
    frame: &TraceFrame,
    cache: &SpanCache,
    selected: usize,
    vs: &ViewState,
    viewport_w: f32,
    viewport_h: f32,
) -> Vec<RectInstance> {
    let mut rects = Vec::new();
    let Some(target) = frame.spans.get(selected) else {
        return rects;
    };
    if frame.spans.len() < 2 {
        return rects;
    }
    let a_end = target.end_ns();
    let sorted = &cache.spans_sorted_by_end;
    let outside = |y: f32| y < -viewport_h || y > viewport_h * 2.0;

    // Window of spans ending at (essentially) the same instant as the target.
    let lo =
        sorted.partition_point(|i| frame.spans[*i as usize].end_ns() < a_end.saturating_sub(WAIT_ALIGN_TOLERANCE_NS));
    let hi = sorted
        .partition_point(|i| frame.spans[*i as usize].end_ns() <= a_end.saturating_add(WAIT_ALIGN_TOLERANCE_NS));

    // Keep the best candidate per thread (longest overlap) to limit clutter.
    let mut best: HashMap<u64, (usize, u64)> = HashMap::new();
    for i in lo..hi {
        let idx = sorted[i] as usize;
        if idx == selected {
            continue;
        }
        let b = &frame.spans[idx];
        if b.thread_id == target.thread_id {
            continue;
        }
        if b.start_ns >= a_end || b.end_ns() <= target.start_ns {
            continue;
        }
        let overlap = b
            .end_ns()
            .min(a_end)
            .saturating_sub(b.start_ns.max(target.start_ns));
        if overlap == 0 {
            continue;
        }
        match best.get_mut(&b.thread_id) {
            Some(entry) => {
                if overlap > entry.1 {
                    *entry = (idx, overlap);
                }
            }
            None => {
                best.insert(b.thread_id, (idx, overlap));
            }
        }
    }

    let mut cands: Vec<(usize, u64)> = best.into_values().collect();
    cands.sort_unstable_by_key(|c| std::cmp::Reverse(c.1));
    cands.truncate(MAX_DEPENDENCY_ARROWS);

    for (idx, _overlap) in cands {
        let b = &frame.spans[idx];
        // Direction: the later-starting span (waiter) points at the
        // earlier-starting one (the span the waiter is gated on).
        let (from, to) = if b.start_ns > target.start_ns {
            (idx, selected)
        } else if b.start_ns < target.start_ns {
            (selected, idx)
        } else {
            continue;
        };
        let Some(s) = span_box_center(frame, &cache.thread_offsets, vs, from, viewport_w) else {
            continue;
        };
        let Some(e) = span_box_center(frame, &cache.thread_offsets, vs, to, viewport_w) else {
            continue;
        };
        if (e.1 - s.1).abs() < 2.0 {
            continue;
        }
        // Cull arrows wholly outside the visible area.
        if outside(s.1) && outside(e.1) && !((s.0 >= THREAD_LABEL_WIDTH && s.0 <= viewport_w)
            || (e.0 >= THREAD_LABEL_WIDTH && e.0 <= viewport_w))
        {
            continue;
        }
        push_curve_arrow(s, e, &mut rects);
    }

    rects
}
