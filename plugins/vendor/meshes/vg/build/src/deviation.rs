//! Measured object-space deviation of a simplified group.
//!
//! Every cluster carries *reference points*: samples of the original
//! (level-0) surface that it stands in for. When a group is simplified, each
//! reference point of its input clusters is measured against the simplified
//! surface; the maximum distance is the group's error. Because the points
//! are original geometry (not the previous level's vertices), the measured
//! value already contains everything accumulated by earlier levels, so errors
//! neither need to be summed nor scaled by a safety factor: parent error is
//! just `max(children, measured)` (plus the strict-ordering epsilon).
//!
//! Points then follow their nearest simplified triangle into the produced
//! cluster, so the next level keeps measuring against the original surface.

use crate::math::*;

/// Maximum reference points a cluster passes on to its consumer group.
pub const MAX_REFS_PER_CLUSTER: usize = 256;

/// Squared distance from `p` to triangle `abc` (Ericson, RTCD 5.1.5).
pub fn point_tri_dist2(p: V3, a: V3, b: V3, c: V3) -> f32 {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let q = |q: V3| {
        let d = sub(p, q);
        dot(d, d)
    };
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return q(a);
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return q(b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return q(add(a, scale(ab, d1 / (d1 - d3))));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return q(c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return q(add(a, scale(ac, d2 / (d2 - d6))));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return q(add(b, scale(sub(c, b), w)));
    }
    let denom = 1.0 / (va + vb + vc);
    q(add(add(a, scale(ab, vb * denom)), scale(ac, vc * denom)))
}

/// Small uniform grid over the triangles of one simplified group.
struct TriGrid {
    tris: Vec<[V3; 3]>,
    lo: V3,
    cell: f32,
    dims: [i32; 3],
    start: Vec<u32>,
    items: Vec<u32>,
}

impl TriGrid {
    fn new(tris: Vec<[V3; 3]>) -> Self {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        let mut area = 0.0f64;
        for t in &tris {
            for v in t {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            area += 0.5 * length(cross(sub(t[1], t[0]), sub(t[2], t[0]))) as f64;
        }
        let ext = (0..3).map(|k| hi[k] - lo[k]).fold(0.0f32, f32::max);
        let by_area = (2.0 * area / tris.len().max(1) as f64).sqrt() as f32;
        let cell = by_area.max(ext / 24.0).max(1e-12);
        let dims: [i32; 3] = std::array::from_fn(|k| (((hi[k] - lo[k]) / cell) as i32 + 1).max(1));
        let ncell = (dims[0] * dims[1] * dims[2]) as usize;
        let mut g = Self {
            tris,
            lo,
            cell,
            dims,
            start: vec![0; ncell + 1],
            items: Vec::new(),
        };
        let ranges: Vec<([i32; 3], [i32; 3])> = g
            .tris
            .iter()
            .map(|t| {
                let mut a = [f32::INFINITY; 3];
                let mut b = [f32::NEG_INFINITY; 3];
                for v in t {
                    for k in 0..3 {
                        a[k] = a[k].min(v[k]);
                        b[k] = b[k].max(v[k]);
                    }
                }
                (g.key(a), g.key(b))
            })
            .collect();
        for (a, b) in &ranges {
            for x in a[0]..=b[0] {
                for y in a[1]..=b[1] {
                    for z in a[2]..=b[2] {
                        let ci = g.cell_index(x, y, z);
                        g.start[ci + 1] += 1;
                    }
                }
            }
        }
        for i in 0..ncell {
            g.start[i + 1] += g.start[i];
        }
        g.items = vec![0; g.start[ncell] as usize];
        let mut fill = g.start.clone();
        for (i, (a, b)) in ranges.iter().enumerate() {
            for x in a[0]..=b[0] {
                for y in a[1]..=b[1] {
                    for z in a[2]..=b[2] {
                        let c = g.cell_index(x, y, z);
                        g.items[fill[c] as usize] = i as u32;
                        fill[c] += 1;
                    }
                }
            }
        }
        g
    }

    fn cell_index(&self, x: i32, y: i32, z: i32) -> usize {
        ((z * self.dims[1] + y) * self.dims[0] + x) as usize
    }

    fn key(&self, p: V3) -> [i32; 3] {
        std::array::from_fn(|k| {
            (((p[k] - self.lo[k]) / self.cell).floor() as i32).clamp(0, self.dims[k] - 1)
        })
    }

    /// Nearest triangle to `p`: (distance, index).
    fn nearest(&self, p: V3) -> (f32, u32) {
        let c = self.key(p);
        let max_r = self.dims[0].max(self.dims[1]).max(self.dims[2]);
        let mut best = (f32::INFINITY, u32::MAX);
        for r in 0..=max_r {
            if r > 0 && best.0 <= (r - 1) as f32 * self.cell {
                break;
            }
            for z in (c[2] - r).max(0)..=(c[2] + r).min(self.dims[2] - 1) {
                for y in (c[1] - r).max(0)..=(c[1] + r).min(self.dims[1] - 1) {
                    let shell = (z - c[2]).abs() == r || (y - c[1]).abs() == r;
                    let step = if shell { 1 } else { (2 * r).max(1) };
                    let mut x = c[0] - r;
                    while x <= c[0] + r {
                        if x >= 0 && x < self.dims[0] {
                            let cell = self.cell_index(x, y, z);
                            for &i in &self.items
                                [self.start[cell] as usize..self.start[cell + 1] as usize]
                            {
                                let t = &self.tris[i as usize];
                                let d = point_tri_dist2(p, t[0], t[1], t[2]).sqrt();
                                if d < best.0 {
                                    best = (d, i);
                                }
                            }
                        }
                        x += step;
                    }
                }
            }
        }
        best
    }
}

pub struct Measured {
    /// Max distance of simplified-surface samples (edge midpoints, centroids)
    /// to the group's input surface: catches long triangles bridging detail
    /// that no reference point sits under.
    pub forward: f32,
    /// Max distance of any reference point to the simplified surface.
    pub error: f32,
    /// Reference points assigned to each output piece (nearest triangle).
    pub piece_refs: Vec<Vec<V3>>,
}

/// Measure `refs` against the simplified `pieces` (triangles per produced
/// cluster) and distribute the points (capped) to the pieces.
pub fn measure(refs: &[V3], input: &[[V3; 3]], pieces: &[&[[u32; 3]]], pos: &[V3]) -> Measured {
    let mut tris = Vec::new();
    let mut owner = Vec::new();
    for (pi, p) in pieces.iter().enumerate() {
        for t in p.iter() {
            tris.push(t.map(|v| pos[v as usize]));
            owner.push(pi as u32);
        }
    }
    let mut per_piece: Vec<Vec<(f32, V3)>> = vec![Vec::new(); pieces.len()];
    let mut error = 0.0f32;
    let mut forward = 0.0f32;
    if !input.is_empty() {
        let g = TriGrid::new(input.to_vec());
        for t in &tris {
            let m = |a: V3, b: V3| scale(add(a, b), 0.5);
            let c = scale(add(add(t[0], t[1]), t[2]), 1.0 / 3.0);
            for p in [c, m(t[0], t[1]), m(t[1], t[2]), m(t[2], t[0])] {
                forward = forward.max(g.nearest(p).0);
            }
        }
    }
    if !tris.is_empty() {
        let grid = TriGrid::new(tris);
        for &p in refs {
            let (d, t) = grid.nearest(p);
            error = error.max(d);
            per_piece[owner[t as usize] as usize].push((d, p));
        }
    }
    let piece_refs = per_piece.into_iter().map(cap_refs).collect();
    Measured {
        forward,
        error,
        piece_refs,
    }
}

/// Keep at most [`MAX_REFS_PER_CLUSTER`] points: the worst half by measured
/// distance (the ones that define the error) plus an even stride over the
/// rest (so the whole surface stays covered). Deterministic.
fn cap_refs(mut pts: Vec<(f32, V3)>) -> Vec<V3> {
    if pts.len() <= MAX_REFS_PER_CLUSTER {
        return pts.into_iter().map(|p| p.1).collect();
    }
    let keep_worst = MAX_REFS_PER_CLUSTER / 2;
    let mut idx: Vec<u32> = (0..pts.len() as u32).collect();
    idx.sort_unstable_by(|&a, &b| {
        pts[b as usize]
            .0
            .total_cmp(&pts[a as usize].0)
            .then(a.cmp(&b))
    });
    let mut taken = vec![false; pts.len()];
    for &i in &idx[..keep_worst] {
        taken[i as usize] = true;
    }
    let rest: Vec<usize> = (0..pts.len()).filter(|&i| !taken[i]).collect();
    let want = MAX_REFS_PER_CLUSTER - keep_worst;
    for k in 0..want {
        taken[rest[k * rest.len() / want]] = true;
    }
    let mut out = Vec::with_capacity(MAX_REFS_PER_CLUSTER);
    for (i, p) in pts.drain(..).enumerate() {
        if taken[i] {
            out.push(p.1);
        }
    }
    out
}

/// Distance of every point to the nearest of `tris`.
pub fn point_deviations(points: &[V3], tris: &[[V3; 3]]) -> Vec<f32> {
    if tris.is_empty() {
        return vec![f32::INFINITY; points.len()];
    }
    let grid = TriGrid::new(tris.to_vec());
    points.iter().map(|&p| grid.nearest(p).0).collect()
}
