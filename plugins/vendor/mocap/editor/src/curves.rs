//! Turning a channel column into polylines for a lane of a given width.
//!
//! Zoomed out (several frames per pixel) a lane draws each pixel's min-max
//! envelope, so a spike is never lost to decimation and the point count is
//! bounded by the lane width however long the take is; zoomed in it draws one
//! point per frame. Gaps (`NaN`) split the line.

/// The view a lane is drawn for.
#[derive(Clone, Copy, Debug)]
pub struct CurveSpec {
    /// The frame at the lane's left edge.
    pub first_frame: f64,
    pub frames_per_px: f64,
    pub width: f32,
    pub height: f32,
    /// Sample values mapped to the lane's bottom and top.
    pub lo: f32,
    pub hi: f32,
}

impl CurveSpec {
    fn y(&self, value: f32) -> f32 {
        let span = (self.hi - self.lo).max(1e-6);
        let t = ((value - self.lo) / span).clamp(-0.05, 1.05);
        self.height - t * self.height
    }
}

/// The polylines of `column` for `spec`, in lane pixels (origin top-left).
pub fn curve_polylines(column: &[f32], spec: &CurveSpec) -> Vec<Vec<[f32; 2]>> {
    if column.is_empty() || spec.width <= 0.0 || spec.frames_per_px <= 0.0 {
        return Vec::new();
    }
    if spec.frames_per_px <= 1.0 {
        per_frame(column, spec)
    } else {
        envelope(column, spec)
    }
}

fn per_frame(column: &[f32], spec: &CurveSpec) -> Vec<Vec<[f32; 2]>> {
    let first = (spec.first_frame.floor().max(0.0) as usize).min(column.len());
    let last = ((spec.first_frame + f64::from(spec.width) * spec.frames_per_px).ceil() as usize + 1).min(column.len());
    let mut lines = Vec::new();
    let mut current: Vec<[f32; 2]> = Vec::new();
    for (frame, value) in column.iter().enumerate().take(last).skip(first) {
        if value.is_finite() {
            let x = ((frame as f64 - spec.first_frame) / spec.frames_per_px) as f32;
            current.push([x, spec.y(*value)]);
        } else if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn envelope(column: &[f32], spec: &CurveSpec) -> Vec<Vec<[f32; 2]>> {
    let mut lines = Vec::new();
    let mut current: Vec<[f32; 2]> = Vec::new();
    for px in 0..spec.width.ceil() as usize {
        let from = (spec.first_frame + px as f64 * spec.frames_per_px).floor().max(0.0) as usize;
        let to = ((spec.first_frame + (px + 1) as f64 * spec.frames_per_px).ceil() as usize).min(column.len());
        if from >= column.len() {
            break;
        }
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for value in column[from..to.max(from + 1).min(column.len())].iter().filter(|v| v.is_finite()) {
            lo = lo.min(*value);
            hi = hi.max(*value);
        }
        if lo > hi {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            continue;
        }
        let x = px as f32 + 0.5;
        current.push([x, spec.y(lo)]);
        current.push([x, spec.y(hi)]);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// The `(lo, hi)` a channel's lane is scaled to: the whole take's range,
/// padded; a fixed `[-1, 1]` for unit-range data.
pub fn value_range(columns: &[&[f32]], fixed: Option<(f32, f32)>) -> (f32, f32) {
    if let Some(range) = fixed {
        return range;
    }
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for value in columns.iter().flat_map(|c| c.iter()).filter(|v| v.is_finite()) {
        lo = lo.min(*value);
        hi = hi.max(*value);
    }
    if lo > hi {
        return (-1.0, 1.0);
    }
    let pad = ((hi - lo) * 0.08).max(1e-3);
    (lo - pad, hi + pad)
}

/// A tick spacing in frames that keeps labels at least `min_px` apart.
pub fn tick_step(frames_per_px: f64, min_px: f64) -> u64 {
    const STEPS: [u64; 14] = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600, 7200, 18000];
    let wanted = frames_per_px * min_px;
    STEPS.iter().copied().find(|s| *s as f64 >= wanted).unwrap_or(36_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(first: f64, fpp: f64, width: f32) -> CurveSpec {
        CurveSpec { first_frame: first, frames_per_px: fpp, width, height: 100.0, lo: 0.0, hi: 1.0 }
    }

    #[test]
    fn zoomed_in_draws_a_point_per_frame_and_splits_at_gaps() {
        let column = [0.0, 0.5, f32::NAN, 1.0, 0.5];
        let lines = curve_polylines(&column, &spec(0.0, 0.25, 100.0));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 2);
        assert_eq!(lines[1].len(), 2);
        assert_eq!(lines[0][0], [0.0, 100.0]);
        assert_eq!(lines[1][0][0], 12.0);
    }

    #[test]
    fn zoomed_out_is_bounded_by_the_width_and_keeps_a_spike() {
        let mut column = vec![0.0f32; 1_000_000];
        column[543_210] = 1.0;
        let lines = curve_polylines(&column, &spec(0.0, 1000.0, 1000.0));
        let points: usize = lines.iter().map(Vec::len).sum();
        assert!(points <= 2000, "{points} points");
        let top = lines.iter().flatten().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        assert!(top < 1.0, "the one-sample spike reaches the top: {top}");
    }

    #[test]
    fn a_view_past_the_end_draws_nothing() {
        assert!(curve_polylines(&[0.0; 10], &spec(50.0, 1.0, 100.0)).is_empty());
        assert!(curve_polylines(&[], &spec(0.0, 1.0, 100.0)).is_empty());
    }

    #[test]
    fn ranges_pad_and_fall_back() {
        assert_eq!(value_range(&[&[f32::NAN]], None), (-1.0, 1.0));
        assert_eq!(value_range(&[&[0.0, 1.0]], Some((-1.0, 1.0))), (-1.0, 1.0));
        let (lo, hi) = value_range(&[&[0.0, 1.0]], None);
        assert!(lo < 0.0 && hi > 1.0);
    }

    #[test]
    fn ticks_keep_labels_apart() {
        assert_eq!(tick_step(1.0, 60.0), 60);
        assert_eq!(tick_step(0.1, 60.0), 10);
        assert!(tick_step(1e9, 60.0) >= 18000);
    }
}
