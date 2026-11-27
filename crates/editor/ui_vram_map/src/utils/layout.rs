//! Treemap layouts: rectangles whose areas are proportional to sizes.
//!
//! [`squarify`] is Bruls, Huizing and van Wijk's squarified treemap: rows of
//! boxes laid along the shorter side, a row closed when adding the next box
//! would make its worst aspect ratio worse. Boxes come out close to square,
//! which is what makes sizes comparable by eye. [`strip`] keeps the input
//! order (reading order, still near square), for children whose order means
//! something (allocations by offset inside a memory block).

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }

    /// Shrunk by `top` at the top and `rest` on the other three sides; `None`
    /// when nothing is left.
    pub fn inset(&self, top: f32, rest: f32) -> Option<Rect> {
        let w = self.w - 2.0 * rest;
        let h = self.h - top - rest;
        (w > 0.5 && h > 0.5).then(|| Rect::new(self.x + rest, self.y + top, w, h))
    }
}

/// Squarified layout of `sizes` in `rect`, one rectangle per size in input
/// order. Zero sizes get empty rectangles.
pub fn squarify(sizes: &[f64], rect: Rect) -> Vec<Rect> {
    let mut out = vec![Rect::new(rect.x, rect.y, 0.0, 0.0); sizes.len()];
    let total: f64 = sizes.iter().filter(|s| **s > 0.0).sum();
    if total <= 0.0 || rect.w <= 0.0 || rect.h <= 0.0 {
        return out;
    }
    let scale = f64::from(rect.w) * f64::from(rect.h) / total;
    let mut order: Vec<usize> = (0..sizes.len()).filter(|&i| sizes[i] > 0.0).collect();
    order.sort_by(|&a, &b| sizes[b].total_cmp(&sizes[a]));
    let areas: Vec<f64> = order.iter().map(|&i| sizes[i] * scale).collect();

    let (mut x, mut y) = (f64::from(rect.x), f64::from(rect.y));
    let (mut w, mut h) = (f64::from(rect.w), f64::from(rect.h));
    let mut start = 0;
    while start < areas.len() {
        let side = w.min(h).max(f64::MIN_POSITIVE);
        let mut end = start + 1;
        let mut sum = areas[start];
        let mut worst = worst_ratio(&areas[start..end], sum, side);
        while end < areas.len() {
            let next_sum = sum + areas[end];
            let next = worst_ratio(&areas[start..=end], next_sum, side);
            if next > worst {
                break;
            }
            worst = next;
            sum = next_sum;
            end += 1;
        }
        // The last row takes whatever is left, so rounding never leaves a gap.
        let last = end == areas.len();
        if w >= h {
            let thickness = if last { w } else { (sum / h).min(w) };
            let mut cy = y;
            for (k, &area) in areas[start..end].iter().enumerate() {
                let length = if k + 1 == end - start { y + h - cy } else { area / thickness };
                out[order[start + k]] = Rect::new(x as f32, cy as f32, thickness as f32, length as f32);
                cy += length;
            }
            x += thickness;
            w -= thickness;
        } else {
            let thickness = if last { h } else { (sum / w).min(h) };
            let mut cx = x;
            for (k, &area) in areas[start..end].iter().enumerate() {
                let length = if k + 1 == end - start { x + w - cx } else { area / thickness };
                out[order[start + k]] = Rect::new(cx as f32, y as f32, length as f32, thickness as f32);
                cx += length;
            }
            y += thickness;
            h -= thickness;
        }
        start = end;
    }
    out
}

/// The worst aspect ratio of a row of `areas` (summing to `sum`) laid along a
/// side of length `side`.
fn worst_ratio(areas: &[f64], sum: f64, side: f64) -> f64 {
    let (min, max) = areas
        .iter()
        .fold((f64::INFINITY, 0.0f64), |(lo, hi), &a| (lo.min(a), hi.max(a)));
    let side2 = side * side;
    let sum2 = sum * sum;
    (side2 * max / sum2).max(sum2 / (side2 * min))
}

/// `sizes` in input order as a strip treemap (Bederson, Shneiderman and
/// Wattenberg): boxes fill strips in reading order, left to right, a strip
/// closed when the next box would raise its mean aspect ratio, strips stacked
/// top to bottom (columns, filled top to bottom, when `rect` is taller than
/// wide). Boxes stay near square while the order is kept, so an address range
/// reads like text.
pub fn strip(sizes: &[f64], rect: Rect) -> Vec<Rect> {
    let mut out = vec![Rect::new(rect.x, rect.y, 0.0, 0.0); sizes.len()];
    let total: f64 = sizes.iter().map(|s| s.max(0.0)).sum();
    if total <= 0.0 || rect.w <= 0.0 || rect.h <= 0.0 {
        return out;
    }
    // Work in a frame whose strips run along `along` and stack along `across`.
    let rows_horizontal = rect.w >= rect.h;
    let (along, across) = if rows_horizontal {
        (f64::from(rect.w), f64::from(rect.h))
    } else {
        (f64::from(rect.h), f64::from(rect.w))
    };
    let scale = along * across / total;
    let areas: Vec<f64> = sizes.iter().map(|s| s.max(0.0) * scale).collect();
    let mean_ratio = |row: &[f64]| {
        let sum: f64 = row.iter().sum();
        if sum <= 0.0 {
            return 1.0;
        }
        let thickness = sum / along;
        let live: Vec<f64> = row.iter().copied().filter(|a| *a > 0.0).collect();
        live.iter()
            .map(|a| {
                let length = a / thickness;
                (length / thickness).max(thickness / length)
            })
            .sum::<f64>()
            / live.len().max(1) as f64
    };
    let mut rows: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    while start < areas.len() {
        let mut end = start + 1;
        let mut ratio = mean_ratio(&areas[start..end]);
        while end < areas.len() {
            let next = mean_ratio(&areas[start..=end]);
            if next > ratio {
                break;
            }
            ratio = next;
            end += 1;
        }
        rows.push((start, end));
        start = end;
    }
    let mut offset = 0.0f64;
    for (index, &(start, end)) in rows.iter().enumerate() {
        let sum: f64 = areas[start..end].iter().sum();
        // The last strip takes what is left, so rounding leaves no gap.
        let thickness = if index + 1 == rows.len() { across - offset } else { sum / along };
        let mut at = 0.0f64;
        for i in start..end {
            let length = if i + 1 == end {
                along - at
            } else if thickness > 0.0 {
                areas[i] / thickness
            } else {
                0.0
            };
            out[i] = if rows_horizontal {
                Rect::new(rect.x + at as f32, rect.y + offset as f32, length as f32, thickness as f32)
            } else {
                Rect::new(rect.x + offset as f32, rect.y + at as f32, thickness as f32, length as f32)
            };
            at += length;
        }
        offset += thickness;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(r: &Rect) -> f64 {
        f64::from(r.w) * f64::from(r.h)
    }

    #[test]
    fn areas_follow_the_sizes_and_fill_the_rectangle() {
        let sizes = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let rect = Rect::new(0.0, 0.0, 6.0, 4.0);
        let rects = squarify(&sizes, rect);
        let total: f64 = rects.iter().map(area).sum();
        assert!((total - 24.0).abs() < 1e-3, "{total}");
        for (size, r) in sizes.iter().zip(&rects) {
            assert!((area(r) - size).abs() < 1e-3, "{size} -> {r:?}");
            assert!(r.x >= -1e-4 && r.y >= -1e-4);
            assert!(r.x + r.w <= 6.0 + 1e-3 && r.y + r.h <= 4.0 + 1e-3);
        }
    }

    #[test]
    fn boxes_come_out_close_to_square() {
        let sizes = vec![1.0; 16];
        let rects = squarify(&sizes, Rect::new(0.0, 0.0, 100.0, 100.0));
        for r in &rects {
            let ratio = r.w.max(r.h) / r.w.min(r.h);
            assert!(ratio < 1.6, "{r:?}");
        }
    }

    #[test]
    fn boxes_do_not_overlap() {
        let sizes = [50.0, 20.0, 9.0, 9.0, 5.0, 3.0, 2.0, 1.0, 1.0];
        let rects = squarify(&sizes, Rect::new(0.0, 0.0, 300.0, 120.0));
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                let overlap_w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
                let overlap_h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
                assert!(overlap_w <= 1e-3 || overlap_h <= 1e-3, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn zero_sizes_get_empty_boxes() {
        let rects = squarify(&[0.0, 1.0], Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(area(&rects[0]), 0.0);
        let rects = strip(&[0.0, 1.0], Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(area(&rects[0]), 0.0);
    }

    #[test]
    fn strips_keep_reading_order_fill_the_rectangle_and_stay_near_square() {
        // 64 equal allocations in a 4:1 block: a grid of near-square boxes,
        // not 64 slivers.
        let sizes = vec![1.0; 64];
        let rect = Rect::new(0.0, 0.0, 400.0, 100.0);
        let rects = strip(&sizes, rect);
        let total: f64 = rects.iter().map(area).sum();
        assert!((total - 40_000.0).abs() < 1.0, "{total}");
        for r in &rects {
            let ratio = r.w.max(r.h) / r.w.min(r.h);
            assert!(ratio < 2.0, "{r:?}");
        }
        // Reading order: each box starts right of the one before, or on a
        // lower row.
        for pair in rects.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert!(b.y > a.y + 1e-3 || (b.x > a.x && (b.y - a.y).abs() < 1e-3), "{a:?} then {b:?}");
        }
        // Unequal sizes keep their areas (sizes sum to the rectangle's 24).
        let sizes = [5.0, 1.0, 1.0, 8.0, 2.0, 3.0, 4.0];
        let rects = strip(&sizes, Rect::new(0.0, 0.0, 6.0, 4.0));
        for (size, r) in sizes.iter().zip(&rects) {
            assert!((area(r) - size).abs() < 1e-3, "{size} -> {r:?}");
        }
    }
}
