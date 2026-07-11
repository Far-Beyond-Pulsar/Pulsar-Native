use crate::constants::*;
use crate::coordinates::time_to_x;
use crate::lod_tree::{LODTree, MergedSpan};
use crate::rendering::text::{push_text, CHAR_H, CHAR_W};
use crate::rendering::types::{GpuSpan, RectInstance};
use crate::state::ViewState;
use crate::trace_data::TraceFrame;
use std::collections::BTreeMap;
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
                    _pad: [0; 3],
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
                    _pad: [0; 3],
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
                    _pad: [0; 3],
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
                    _pad: [0; 3],
                });
            }
        }
        idx += 1;
    }

    rects
}
