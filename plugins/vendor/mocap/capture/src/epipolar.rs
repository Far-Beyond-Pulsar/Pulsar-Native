//! Two-view and multi-view geometry for camera self-calibration: the essential
//! matrix from point correspondences (8-point with RANSAC), its decomposition
//! into a relative pose, and weighted multi-view triangulation.
//!
//! Points are *normalised* image coordinates `((u - cx) / f, (v - cy) / f)`, so a
//! camera with `f` pixels of focal length and its principal point at the image
//! centre sees a point `X` at `(X.x / X.z, X.y / X.z)`.

use nalgebra::{DMatrix, Matrix3, Rotation3, Vector3, SVD};

/// A deterministic generator (xorshift), so a calibration is reproducible.
pub struct Rng(u64);

impl Rng {
    /// A generator from `seed` (never zero internally).
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    /// The next value in `0..bound`.
    pub fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound.max(1) as u64) as usize
    }
}

/// A relative pose: `x_b = r * x_a + t` (with `|t| = 1` until scaled).
#[derive(Clone, Copy, Debug)]
pub struct RelativePose {
    pub r: Matrix3<f64>,
    pub t: Vector3<f64>,
}

/// The 8-point estimate of the essential matrix `E` with `b^T E a = 0`.
pub fn essential_8pt(a: &[[f64; 2]], b: &[[f64; 2]]) -> Option<Matrix3<f64>> {
    let n = a.len().min(b.len());
    if n < 8 {
        return None;
    }
    let mut rows = DMatrix::<f64>::zeros(n.max(9), 9);
    for i in 0..n {
        let ([x1, y1], [x2, y2]) = (a[i], b[i]);
        let row = [x2 * x1, x2 * y1, x2, y2 * x1, y2 * y1, y2, x1, y1, 1.0];
        for (k, v) in row.iter().enumerate() {
            rows[(i, k)] = *v;
        }
    }
    let svd = SVD::new(rows, false, true);
    let v_t = svd.v_t?;
    let (index, _) = svd.singular_values.iter().enumerate().min_by(|x, y| x.1.total_cmp(y.1))?;
    let e = Matrix3::from_row_slice(&v_t.row(index.min(v_t.nrows() - 1)).iter().copied().collect::<Vec<_>>());
    // Enforce two equal singular values and a zero third.
    let svd = SVD::new(e, true, true);
    let (u, v_t) = (svd.u?, svd.v_t?);
    let s = (svd.singular_values[0] + svd.singular_values[1]) * 0.5;
    Some(u * Matrix3::from_diagonal(&Vector3::new(s, s, 0.0)) * v_t)
}

/// The first-order geometric (Sampson) error of a correspondence under `e`.
pub fn sampson(e: &Matrix3<f64>, a: [f64; 2], b: [f64; 2]) -> f64 {
    let (pa, pb) = (Vector3::new(a[0], a[1], 1.0), Vector3::new(b[0], b[1], 1.0));
    let (ea, etb) = (e * pa, e.transpose() * pb);
    let num = pb.dot(&ea);
    let den = ea[0] * ea[0] + ea[1] * ea[1] + etb[0] * etb[0] + etb[1] * etb[1];
    if den < 1e-18 {
        f64::INFINITY
    } else {
        (num * num / den).sqrt()
    }
}

/// RANSAC over the 8-point estimate: the matrix and which correspondences fit it
/// within `threshold` (normalised units).
pub fn essential_ransac(a: &[[f64; 2]], b: &[[f64; 2]], threshold: f64, iterations: usize, rng: &mut Rng) -> Option<(Matrix3<f64>, Vec<bool>)> {
    let n = a.len().min(b.len());
    if n < 12 {
        return None;
    }
    let mut best: Option<(Matrix3<f64>, usize)> = None;
    for _ in 0..iterations {
        let picks: Vec<usize> = (0..8).map(|_| rng.below(n)).collect();
        let (sa, sb): (Vec<_>, Vec<_>) = picks.iter().map(|&i| (a[i], b[i])).unzip();
        let Some(e) = essential_8pt(&sa, &sb) else { continue };
        let count = (0..n).filter(|&i| sampson(&e, a[i], b[i]) < threshold).count();
        if best.as_ref().is_none_or(|(_, c)| count > *c) {
            best = Some((e, count));
        }
    }
    let (e, _) = best?;
    let inliers: Vec<bool> = (0..n).map(|i| sampson(&e, a[i], b[i]) < threshold).collect();
    let (ia, ib): (Vec<_>, Vec<_>) = (0..n).filter(|&i| inliers[i]).map(|i| (a[i], b[i])).unzip();
    let refit = essential_8pt(&ia, &ib).unwrap_or(e);
    let inliers = (0..n).map(|i| sampson(&refit, a[i], b[i]) < threshold).collect();
    Some((refit, inliers))
}

/// A point seen by several cameras, triangulated by the weighted DLT. Each view
/// is `(R, t, normalised point, weight)` with `x_cam = R * X + t`.
pub fn triangulate(views: &[(&Matrix3<f64>, &Vector3<f64>, [f64; 2], f64)]) -> Option<Vector3<f64>> {
    if views.len() < 2 {
        return None;
    }
    let mut rows = DMatrix::<f64>::zeros(views.len() * 2 + 2, 4);
    for (i, (r, t, p, w)) in views.iter().enumerate() {
        let (p1, p2, p3) = (r.row(0), r.row(1), r.row(2));
        for k in 0..3 {
            rows[(2 * i, k)] = w * (p[0] * p3[k] - p1[k]);
            rows[(2 * i + 1, k)] = w * (p[1] * p3[k] - p2[k]);
        }
        rows[(2 * i, 3)] = w * (p[0] * t[2] - t[0]);
        rows[(2 * i + 1, 3)] = w * (p[1] * t[2] - t[1]);
    }
    let svd = SVD::new(rows, false, true);
    let v_t = svd.v_t?;
    let (index, _) = svd.singular_values.iter().enumerate().min_by(|x, y| x.1.total_cmp(y.1))?;
    let h = v_t.row(index.min(v_t.nrows() - 1));
    (h[3].abs() > 1e-12).then(|| Vector3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3]))
}

/// The four `(R, t)` candidates of an essential matrix.
fn candidates(e: &Matrix3<f64>) -> Option<[RelativePose; 4]> {
    let svd = SVD::new(*e, true, true);
    let (mut u, mut v_t) = (svd.u?, svd.v_t?);
    if u.determinant() < 0.0 {
        u.column_mut(2).neg_mut();
    }
    if v_t.determinant() < 0.0 {
        v_t.row_mut(2).neg_mut();
    }
    let w = Matrix3::new(0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0);
    let (r1, r2) = (u * w * v_t, u * w.transpose() * v_t);
    let t: Vector3<f64> = u.column(2).into();
    Some([RelativePose { r: r1, t }, RelativePose { r: r1, t: -t }, RelativePose { r: r2, t }, RelativePose { r: r2, t: -t }])
}

/// The relative pose of the camera pair that puts the inliers in front of both
/// cameras (the cheirality test), with the number of points that pass.
pub fn recover_pose(e: &Matrix3<f64>, a: &[[f64; 2]], b: &[[f64; 2]], inliers: &[bool]) -> Option<(RelativePose, usize)> {
    let identity = (Matrix3::<f64>::identity(), Vector3::<f64>::zeros());
    candidates(e)?
        .into_iter()
        .map(|pose| {
            let count = (0..a.len().min(b.len()))
                .filter(|&i| inliers.get(i).copied().unwrap_or(true))
                .filter(|&i| {
                    let x = triangulate(&[(&identity.0, &identity.1, a[i], 1.0), (&pose.r, &pose.t, b[i], 1.0)]);
                    x.is_some_and(|x| x[2] > 0.0 && (pose.r * x + pose.t)[2] > 0.0)
                })
                .count();
            (pose, count)
        })
        .max_by_key(|(_, count)| *count)
}

/// A rotation matrix from a rotation vector (axis times angle).
pub fn rotation_from_vector(v: Vector3<f64>) -> Matrix3<f64> {
    *Rotation3::from_scaled_axis(v).matrix()
}
