//! Multi-camera self-calibration from the subject itself.
//!
//! Nothing about where the cameras stand is assumed. While the subject moves, each
//! camera's body keypoints (COCO-17, image space) are collected in sync. Pairs of
//! cameras that see the same person give relative poses (essential matrix with
//! RANSAC); the poses are chained from camera 0, scaled to metres with human bone
//! length priors, refined by reprojection error, and finally the world is aligned
//! to the subject: +Y along the torso, the floor at the ankles, the origin under
//! the hips, +Z the way the subject faces, the subject's left at +X.
//!
//! Intrinsics are assumed pinhole with the principal point at the image centre; the
//! focal length starts from a 70 degree horizontal field of view and is refined.
//! The metric scale rests on the priors, so it is only as good as the subject is
//! close to them: set `body_scale` (height / 1.75 m) when it is known.

use crate::epipolar::{essential_ransac, recover_pose, rotation_from_vector, triangulate, Rng};
use mocap_format::CameraCalibration;
use nalgebra::{DMatrix, DVector, Matrix3, Vector3};

pub(crate) const KEYPOINTS: usize = 17;

/// A keypoint under this confidence is not used.
pub const MIN_SCORE: f32 = 0.4;
/// Samples needed before a solve is attempted.
pub const MIN_SAMPLES: usize = 40;
/// Samples kept (more adds time, not accuracy).
pub const MAX_SAMPLES: usize = 160;
/// Correspondences a camera pair needs to be related.
const MIN_CORRESPONDENCES: usize = 50;
/// Keypoints two cameras must see in the same sample for the sample to link them.
const MIN_SHARED: usize = 5;
/// How much a keypoint's position is trusted (a webcam detector is steadier on the torso
/// than on the hands and feet), COCO order.
const RELIABILITY: [f64; KEYPOINTS] = [0.8, 0.5, 0.5, 0.5, 0.5, 1.0, 1.0, 0.7, 0.7, 0.5, 0.5, 1.0, 1.0, 0.7, 0.7, 0.5, 0.5];
/// Pixel error beyond which an observation counts for less and less (robust loss).
const ROBUST_PX: f64 = 4.0;
/// Epipolar (Sampson) error, in pixels, under which a correspondence is an inlier.
const RANSAC_PX: f64 = 5.0;
/// The least parallax a calibration may have, degrees.
const MIN_PARALLAX_DEG: f64 = 5.0;
/// Mean image motion (frame fractions) between accepted samples.
const MIN_MOTION: f32 = 0.02;
/// Worst reprojection error (pixels, RMS) a calibration may have.
pub const MAX_RMS_PX: f64 = 8.0;
/// Horizontal field of view the focal length starts from.
const GUESS_HFOV_DEG: f64 = 70.0;

/// Bones (COCO indices) and their length in metres for a 1.75 m adult.
const BONES: [(usize, usize, f64); 10] = [
    (5, 11, 0.50),
    (6, 12, 0.50),
    (5, 7, 0.29),
    (6, 8, 0.29),
    (7, 9, 0.26),
    (8, 10, 0.26),
    (11, 13, 0.42),
    (12, 14, 0.42),
    (13, 15, 0.40),
    (14, 16, 0.40),
];

/// Why a calibration failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CalibrationError {
    #[error("calibration needs at least two cameras")]
    TooFewCameras,
    #[error("only {have} usable samples of {need}: keep moving where all cameras can see you")]
    TooFewSamples { have: usize, need: usize },
    #[error("cameras {0:?} share too little of the subject with the others: move into their view too")]
    Unplaced(Vec<String>),
    #[error("the poses are degenerate (too little motion or depth): turn, step and raise your arms")]
    Degenerate,
    #[error("the cameras are too close together to measure depth (median parallax {degrees:.1} degrees): spread them further around the subject")]
    NarrowBaseline { degrees: f64 },
    #[error("the calibration does not fit the data (reprojection error {rms:.1} px, limit {MAX_RMS_PX} px)")]
    PoorFit { rms: f64 },
}

/// A camera whose pose is being found.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraSpec {
    pub id: String,
    pub width: u32,
    pub height: u32,
}

/// A solved camera: `x_cam = rotation * X_world + translation`, +Z forward, +Y down in the image.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraModel {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub focal_px: f64,
    pub rotation: Matrix3<f64>,
    pub translation: Vector3<f64>,
}

impl CameraModel {
    fn centre(&self) -> (f64, f64) {
        (f64::from(self.width) * 0.5, f64::from(self.height) * 0.5)
    }

    /// Where the camera is, in world metres.
    pub fn position(&self) -> [f64; 3] {
        let p = -(self.rotation.transpose() * self.translation);
        [p[0], p[1], p[2]]
    }

    /// The direction the camera looks, in world space.
    pub fn forward(&self) -> [f64; 3] {
        let f = self.rotation.transpose() * Vector3::new(0.0, 0.0, 1.0);
        [f[0], f[1], f[2]]
    }

    /// The pixel `world` projects to, `None` behind the camera.
    pub fn project(&self, world: Vector3<f64>) -> Option<[f64; 2]> {
        let x = self.rotation * world + self.translation;
        let (cx, cy) = self.centre();
        (x[2] > 1e-3).then(|| [cx + self.focal_px * x[0] / x[2], cy + self.focal_px * x[1] / x[2]])
    }

    fn normalise(&self, p: [f32; 3]) -> [f64; 2] {
        let (cx, cy) = self.centre();
        [(f64::from(p[0]) * f64::from(self.width) - cx) / self.focal_px, (f64::from(p[1]) * f64::from(self.height) - cy) / self.focal_px]
    }

    fn pixel(&self, p: [f32; 3]) -> [f64; 2] {
        [f64::from(p[0]) * f64::from(self.width), f64::from(p[1]) * f64::from(self.height)]
    }

    /// The storable form.
    pub fn to_calibration(&self) -> CameraCalibration {
        let r = &self.rotation;
        CameraCalibration {
            id: self.id.clone(),
            width: self.width,
            height: self.height,
            focal_px: self.focal_px,
            rotation: [r[(0, 0)], r[(0, 1)], r[(0, 2)], r[(1, 0)], r[(1, 1)], r[(1, 2)], r[(2, 0)], r[(2, 1)], r[(2, 2)]],
            translation: [self.translation[0], self.translation[1], self.translation[2]],
        }
    }

    /// From the stored form.
    pub fn from_calibration(c: &CameraCalibration) -> Self {
        Self {
            id: c.id.clone(),
            width: c.width,
            height: c.height,
            focal_px: c.focal_px,
            rotation: Matrix3::from_row_slice(&c.rotation),
            translation: Vector3::from_row_slice(&c.translation),
        }
    }
}

/// The calibrated cameras.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraRig {
    pub cameras: Vec<CameraModel>,
    /// Trimmed RMS reprojection error over the calibration data, in pixels (the worst
    /// tenth of observations, a detector's gross misses, is ignored).
    pub rms_px: f64,
    pub samples: usize,
    /// Trimmed RMS reprojection error of each camera, pixels.
    pub per_camera_rms: Vec<f64>,
    /// Median angle between two cameras' rays to the same keypoint: small means the
    /// cameras are too close together to measure depth well.
    pub parallax_deg: f64,
}

type Keypoints = Vec<[f32; 3]>;

fn good(p: &[f32; 3]) -> bool {
    p[0].is_finite() && p[1].is_finite() && p[2] >= MIN_SCORE
}

/// Triangulates one keypoint from `(camera index, observation)` views, dropping
/// the worst view when three or more disagree. Returns the point and its RMS
/// reprojection error in pixels.
fn triangulate_point(cams: &[CameraModel], views: &[(usize, [f32; 3])], keypoint: usize) -> Option<(Vector3<f64>, f64)> {
    let mut used: Vec<(usize, [f32; 3])> = views.iter().copied().filter(|(_, p)| good(p)).collect();
    loop {
        let input: Vec<_> = used
            .iter()
            .map(|(c, p)| (&cams[*c].rotation, &cams[*c].translation, cams[*c].normalise(*p), f64::from(p[2]) * RELIABILITY[keypoint.min(KEYPOINTS - 1)]))
            .collect();
        let x = triangulate(&input)?;
        let errors: Vec<f64> = used
            .iter()
            .map(|(c, p)| {
                let o = cams[*c].pixel(*p);
                cams[*c].project(x).map_or(1e6, |q| ((q[0] - o[0]).powi(2) + (q[1] - o[1]).powi(2)).sqrt())
            })
            .collect();
        let (worst, max) = errors.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1))?;
        if used.len() >= 3 && max > 12.0 {
            used.remove(worst);
            continue;
        }
        let rms = (errors.iter().map(|e| e * e).sum::<f64>() / errors.len() as f64).sqrt();
        return Some((x, rms));
    }
}

impl CameraRig {
    /// A rig of already-known cameras (for example from a take's stored calibration).
    pub fn from_cameras(cameras: Vec<CameraModel>) -> Self {
        Self { per_camera_rms: vec![0.0; cameras.len()], cameras, rms_px: 0.0, samples: 0, parallax_deg: 90.0 }
    }

    /// Triangulates the 17 keypoints of one frame: `views[i]` is camera `i`'s
    /// keypoints. Each result is `[x, y, z, reprojection error px]`; a keypoint
    /// fewer than two cameras see is `NaN`.
    pub fn triangulate_frame(&self, views: &[Option<&[[f32; 3]]>]) -> Vec<[f32; 4]> {
        (0..KEYPOINTS)
            .map(|k| {
                let seen: Vec<(usize, [f32; 3])> = views.iter().enumerate().filter_map(|(c, v)| (*v).and_then(|v| v.get(k)).map(|p| (c, *p))).collect();
                triangulate_point(&self.cameras, &seen, k).map_or([f32::NAN; 4], |(x, err)| [x[0] as f32, x[1] as f32, x[2] as f32, err as f32])
            })
            .collect()
    }
}

/// Collects synchronized keypoints while the subject moves, then solves.
#[derive(Clone)]
pub struct Calibrator {
    cameras: Vec<CameraSpec>,
    samples: Vec<Vec<Option<Keypoints>>>,
    /// What the subject was asked to do when each sample was taken (a [`crate::Step`] tag).
    tags: Vec<u32>,
    /// Each camera's keypoints in the last kept sample (motion is judged per camera).
    last: Vec<Option<Keypoints>>,
}

impl Calibrator {
    /// A calibrator for `cameras` (index = position in every sample).
    pub fn new(cameras: Vec<CameraSpec>) -> Self {
        let last = vec![None; cameras.len()];
        Self { cameras, samples: Vec::new(), tags: Vec::new(), last }
    }

    /// Samples collected so far.
    pub fn samples(&self) -> usize {
        self.samples.len()
    }

    /// True when enough samples are in to solve.
    pub fn ready(&self) -> bool {
        self.samples.len() >= MIN_SAMPLES
    }

    /// True when no more samples are wanted.
    pub fn full(&self) -> bool {
        self.samples.len() >= MAX_SAMPLES
    }

    /// Offers one synchronized sample: camera `i`'s 17 `(x, y, score)` keypoints,
    /// `None` when it saw nobody. Kept when at least two cameras see the subject
    /// well and it moved since the last kept sample.
    pub fn offer(&mut self, views: &[Option<&[[f32; 3]]>]) -> bool {
        self.offer_tagged(views, u32::MAX)
    }

    /// [`offer`](Self::offer) a sample taken during the step with `tag`.
    pub fn offer_tagged(&mut self, views: &[Option<&[[f32; 3]]>], tag: u32) -> bool {
        if self.full() || views.len() != self.cameras.len() {
            return false;
        }
        // A sample links the cameras when some pair of them sees at least a handful of the same
        // keypoints; no camera has to see the whole body.
        let seen: Vec<Option<Keypoints>> = views.iter().map(|v| v.filter(|k| k.iter().filter(|p| good(p)).count() >= MIN_SHARED).map(<[_]>::to_vec)).collect();
        if !Self::links(&seen) {
            return false;
        }
        // Motion is the mean image displacement of each camera's own keypoints since the last
        // kept sample (a camera's dropout must not look like movement).
        let moves: Vec<f32> = seen
            .iter()
            .zip(&self.last)
            .filter_map(|(now, before)| {
                let (now, before) = (now.as_ref()?, before.as_ref()?);
                let d: Vec<f32> = now.iter().zip(before).filter(|(a, b)| good(a) && good(b)).map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()).collect();
                (!d.is_empty()).then(|| d.iter().sum::<f32>() / d.len() as f32)
            })
            .collect();
        if self.last.iter().any(Option::is_some) && (moves.is_empty() || moves.iter().sum::<f32>() / (moves.len() as f32) < MIN_MOTION) {
            return false;
        }
        for (slot, now) in self.last.iter_mut().zip(&seen) {
            if now.is_some() {
                *slot = now.clone();
            }
        }
        self.samples.push(seen);
        self.tags.push(tag);
        true
    }

    /// True when two of the views share at least [`MIN_SHARED`] well-seen keypoints.
    fn links(views: &[Option<Keypoints>]) -> bool {
        views.iter().enumerate().any(|(i, a)| {
            views.iter().skip(i + 1).any(|b| match (a, b) {
                (Some(a), Some(b)) => a.iter().zip(b).filter(|(p, q)| good(p) && good(q)).count() >= MIN_SHARED,
                _ => false,
            })
        })
    }

    fn guess_focal(spec: &CameraSpec) -> f64 {
        f64::from(spec.width) * 0.5 / (GUESS_HFOV_DEG.to_radians() * 0.5).tan()
    }

    fn models(&self) -> Vec<CameraModel> {
        self.cameras
            .iter()
            .map(|s| CameraModel {
                id: s.id.clone(),
                width: s.width,
                height: s.height,
                focal_px: Self::guess_focal(s),
                rotation: Matrix3::identity(),
                translation: Vector3::zeros(),
            })
            .collect()
    }

    /// Correspondences between cameras `i` and `j`: normalised points and `(sample, keypoint)`.
    fn pair(&self, cams: &[CameraModel], i: usize, j: usize) -> (Vec<[f64; 2]>, Vec<[f64; 2]>, Vec<(usize, usize)>) {
        let (mut a, mut b, mut at) = (Vec::new(), Vec::new(), Vec::new());
        for (s, sample) in self.samples.iter().enumerate() {
            let (Some(pi), Some(pj)) = (&sample[i], &sample[j]) else { continue };
            for k in 0..KEYPOINTS.min(pi.len()).min(pj.len()) {
                if good(&pi[k]) && good(&pj[k]) {
                    a.push(cams[i].normalise(pi[k]));
                    b.push(cams[j].normalise(pj[k]));
                    at.push((s, k));
                }
            }
        }
        (a, b, at)
    }

    /// Solves for every camera's pose. `body_scale` is the subject's height over 1.75 m.
    pub fn solve(&self, body_scale: f64) -> Result<CameraRig, CalibrationError> {
        if self.cameras.len() < 2 {
            return Err(CalibrationError::TooFewCameras);
        }
        if self.samples.len() < MIN_SAMPLES {
            return Err(CalibrationError::TooFewSamples { have: self.samples.len(), need: MIN_SAMPLES });
        }
        let mut cams = self.models();
        let mut placed = vec![false; cams.len()];
        placed[0] = true;
        // Place cameras one by one from the best-linked pair; a pair that cannot be solved is
        // skipped and the next best tried, so one weak link does not sink the calibration.
        let mut blocked: Vec<(usize, usize)> = Vec::new();
        while let Some((i, j)) = self.next_pair(&cams, &placed, &blocked) {
            match self.place(&mut cams, i, j, body_scale) {
                Ok(()) => placed[j] = true,
                Err(_) => blocked.push((i, j)),
            }
        }
        let missing: Vec<String> = cams.iter().zip(&placed).filter(|(_, p)| !**p).map(|(c, _)| c.id.clone()).collect();
        if !missing.is_empty() {
            return Err(CalibrationError::Unplaced(missing));
        }
        self.refine(&mut cams, body_scale);
        let points = self.triangulate_all(&cams);
        align_world(&mut cams, &points, body_scale)?;
        let rms = self.rms(&cams);
        if !rms.is_finite() || rms > MAX_RMS_PX {
            return Err(CalibrationError::PoorFit { rms });
        }
        let parallax = self.parallax(&cams, &self.triangulate_all(&cams));
        if parallax < MIN_PARALLAX_DEG {
            return Err(CalibrationError::NarrowBaseline { degrees: parallax });
        }
        let per_camera_rms = (0..cams.len()).map(|c| self.camera_rms(&cams, c)).collect();
        Ok(CameraRig { cameras: cams, rms_px: rms, samples: self.samples.len(), per_camera_rms, parallax_deg: parallax })
    }

    /// The (placed, unplaced) pair with the most shared keypoints.
    fn next_pair(&self, cams: &[CameraModel], placed: &[bool], blocked: &[(usize, usize)]) -> Option<(usize, usize)> {
        let mut best: Option<(usize, usize, usize)> = None;
        for i in (0..cams.len()).filter(|&i| placed[i]) {
            for j in (0..cams.len()).filter(|&j| !placed[j] && !blocked.contains(&(i, j))) {
                let count = self.pair(cams, i, j).2.len();
                if count >= MIN_CORRESPONDENCES && best.is_none_or(|(_, _, c)| count > c) {
                    best = Some((i, j, count));
                }
            }
        }
        best.map(|(i, j, _)| (i, j))
    }

    /// Finds camera `j`'s pose from placed camera `i`.
    fn place(&self, cams: &mut [CameraModel], i: usize, j: usize, body_scale: f64) -> Result<(), CalibrationError> {
        let (a, b, at) = self.pair(cams, i, j);
        let threshold = RANSAC_PX / cams[i].focal_px.min(cams[j].focal_px);
        let mut rng = Rng::new(0x5EED + (i * 31 + j) as u64);
        let (e, inliers) = essential_ransac(&a, &b, threshold, 800, &mut rng).ok_or(CalibrationError::Degenerate)?;
        let (pose, front) = recover_pose(&e, &a, &b, &inliers).ok_or(CalibrationError::Degenerate)?;
        let count = inliers.iter().filter(|x| **x).count();
        if count < MIN_CORRESPONDENCES / 2 || front * 10 < count * 7 {
            return Err(CalibrationError::Degenerate);
        }
        // Triangulate the inliers with unit baseline in camera i's frame, to scale from body size.
        let id = (Matrix3::<f64>::identity(), Vector3::<f64>::zeros());
        let mut per_sample: Vec<[Option<Vector3<f64>>; KEYPOINTS]> = vec![[None; KEYPOINTS]; self.samples.len()];
        for (n, &(s, k)) in at.iter().enumerate().filter(|(n, _)| inliers[*n]) {
            per_sample[s][k] = triangulate(&[(&id.0, &id.1, a[n], 1.0), (&pose.r, &pose.t, b[n], 1.0)]);
        }
        let scale = prior_scale(&per_sample, body_scale).ok_or(CalibrationError::Degenerate)?;
        let (ri, ti) = (cams[i].rotation, cams[i].translation);
        cams[j].rotation = pose.r * ri;
        cams[j].translation = pose.r * ti + pose.t * scale;
        Ok(())
    }

    fn triangulate_all(&self, cams: &[CameraModel]) -> Vec<[Option<Vector3<f64>>; KEYPOINTS]> {
        self.samples
            .iter()
            .map(|sample| {
                std::array::from_fn(|k| {
                    let views: Vec<(usize, [f32; 3])> = sample.iter().enumerate().filter_map(|(c, v)| v.as_ref().and_then(|v| v.get(k)).map(|p| (c, *p))).collect();
                    triangulate_point(cams, &views, k).map(|(x, _)| x)
                })
            })
            .collect()
    }

    /// Alternates triangulation, metric rescaling and per-camera refinement.
    fn refine(&self, cams: &mut [CameraModel], body_scale: f64) {
        for _ in 0..6 {
            let mut points = self.triangulate_all(cams);
            if let Some(k) = prior_scale(&points, body_scale) {
                for cam in cams.iter_mut() {
                    cam.translation *= k;
                }
                points.iter_mut().flatten().flatten().for_each(|p| *p *= k);
            }
            for c in 0..cams.len() {
                self.refine_camera(cams, c, &points);
            }
        }
    }

    /// Residuals (pixels, weighted) of camera `c` against fixed 3D points.
    fn residuals(&self, cam: &CameraModel, c: usize, points: &[[Option<Vector3<f64>>; KEYPOINTS]]) -> Vec<f64> {
        let mut out = Vec::new();
        for (s, sample) in self.samples.iter().enumerate() {
            let Some(view) = &sample[c] else { continue };
            for (k, obs) in view.iter().enumerate().filter(|(_, p)| good(p)) {
                let Some(x) = points[s][k] else { continue };
                let o = cam.pixel(*obs);
                let w = (f64::from(obs[2]) * RELIABILITY[k]).sqrt();
                match cam.project(x) {
                    Some(q) => {
                        // A soft-clipped residual: a gross detector miss pulls less and less.
                        let (dx, dy) = (q[0] - o[0], q[1] - o[1]);
                        let d = (dx * dx + dy * dy).sqrt();
                        let robust = 1.0 / (1.0 + (d / ROBUST_PX).powi(2)).sqrt();
                        out.extend([w * dx * robust, w * dy * robust]);
                    }
                    None => out.extend([1e3, 1e3]),
                }
            }
        }
        out
    }

    /// Levenberg-Marquardt on one camera's rotation, translation (not the
    /// reference camera's) and focal length.
    fn refine_camera(&self, cams: &mut [CameraModel], c: usize, points: &[[Option<Vector3<f64>>; KEYPOINTS]]) {
        let free: Vec<usize> = if c == 0 { vec![6] } else { (0..7).collect() };
        let apply = |base: &CameraModel, p: &[f64; 7]| {
            let mut m = base.clone();
            m.rotation = rotation_from_vector(Vector3::new(p[0], p[1], p[2])) * base.rotation;
            m.translation = base.translation + Vector3::new(p[3], p[4], p[5]);
            m.focal_px = (base.focal_px + p[6]).max(50.0);
            m
        };
        let base = cams[c].clone();
        let mut lambda = 1e-3;
        let mut current = self.residuals(&base, c, points);
        let mut model = base.clone();
        let mut cost: f64 = current.iter().map(|r| r * r).sum();
        for _ in 0..10 {
            if current.is_empty() {
                return;
            }
            let mut jac = DMatrix::<f64>::zeros(current.len(), free.len());
            for (col, &p) in free.iter().enumerate() {
                let h = if p == 6 { 0.05 } else { 1e-5 };
                let mut delta = [0.0; 7];
                delta[p] = h;
                let probe = self.residuals(&apply(&model, &delta), c, points);
                for (row, (a, b)) in probe.iter().zip(&current).enumerate() {
                    jac[(row, col)] = (a - b) / h;
                }
            }
            let r = DVector::from_vec(current.clone());
            let jt = jac.transpose();
            let (mut jtj, jtr) = (&jt * &jac, &jt * &r);
            for d in 0..jtj.nrows() {
                jtj[(d, d)] *= 1.0 + lambda;
            }
            let Some(step) = jtj.lu().solve(&(-jtr)) else { return };
            let mut delta = [0.0; 7];
            for (col, &p) in free.iter().enumerate() {
                delta[p] = step[col];
            }
            let candidate = apply(&model, &delta);
            let trial = self.residuals(&candidate, c, points);
            let trial_cost: f64 = trial.iter().map(|r| r * r).sum();
            if trial_cost < cost {
                (model, current, cost, lambda) = (candidate, trial, trial_cost, lambda * 0.3);
            } else {
                lambda *= 10.0;
                if lambda > 1e6 {
                    break;
                }
            }
        }
        cams[c] = model;
    }

    /// Trimmed RMS reprojection error of camera `c` alone.
    fn camera_rms(&self, cams: &[CameraModel], c: usize) -> f64 {
        trimmed_rms(self.reprojection_errors(cams, Some(c)))
    }

    /// Squared reprojection errors (pixels) of every usable observation, of camera `only` or all.
    fn reprojection_errors(&self, cams: &[CameraModel], only: Option<usize>) -> Vec<f64> {
        let points = self.triangulate_all(cams);
        let mut errors = Vec::new();
        for (s, sample) in self.samples.iter().enumerate() {
            for (c, view) in sample.iter().enumerate().filter(|(c, _)| only.is_none_or(|o| o == *c)) {
                let Some(view) = view else { continue };
                for (k, obs) in view.iter().enumerate().filter(|(_, p)| good(p)) {
                    let Some(x) = points[s][k] else { continue };
                    let o = cams[c].pixel(*obs);
                    if let Some(q) = cams[c].project(x) {
                        errors.push((q[0] - o[0]).powi(2) + (q[1] - o[1]).powi(2));
                    }
                }
            }
        }
        errors
    }

    /// Median angle (degrees) between the rays of the first two cameras that see a keypoint.
    fn parallax(&self, cams: &[CameraModel], points: &[[Option<Vector3<f64>>; KEYPOINTS]]) -> f64 {
        let mut angles = Vec::new();
        for (s, sample) in self.samples.iter().enumerate() {
            for k in 0..KEYPOINTS {
                let Some(x) = points[s][k] else { continue };
                let seeing: Vec<usize> = (0..cams.len()).filter(|&c| sample[c].as_ref().and_then(|v| v.get(k)).is_some_and(good)).collect();
                if let [a, b, ..] = seeing[..] {
                    let (pa, pb) = (Vector3::from_row_slice(&cams[a].position()), Vector3::from_row_slice(&cams[b].position()));
                    let (ra, rb) = ((x - pa).normalize(), (x - pb).normalize());
                    angles.push(ra.dot(&rb).clamp(-1.0, 1.0).acos().to_degrees());
                }
            }
        }
        median(angles).unwrap_or(0.0)
    }

    /// Trimmed RMS reprojection error in pixels over every usable observation.
    fn rms(&self, cams: &[CameraModel]) -> f64 {
        trimmed_rms(self.reprojection_errors(cams, None))
    }
}

/// The RMS of squared errors after dropping the worst tenth: a detector's gross misses
/// (a keypoint on the wrong limb) must not decide whether the geometry is good.
fn trimmed_rms(mut squared: Vec<f64>) -> f64 {
    if squared.is_empty() {
        return f64::INFINITY;
    }
    squared.sort_by(f64::total_cmp);
    let keep = (squared.len() * 9 / 10).max(1);
    (squared[..keep].iter().sum::<f64>() / keep as f64).sqrt()
}

fn distance(a: &Vector3<f64>, b: &Vector3<f64>) -> f64 {
    (a - b).norm()
}

/// One check of a solved rig against what the subject was asked to do.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied()
}

impl Calibrator {
    /// Checks the solved `rig` against the guided steps: a raised hand must read up in
    /// 3D (which also checks left from right and the up axis), and arms held out must
    /// span about what a body of `body_scale` spans (which checks the metric scale).
    pub fn validate(&self, rig: &CameraRig, body_scale: f64) -> Vec<Check> {
        let frames = |tag: crate::Step| -> Vec<Vec<[f32; 4]>> {
            self.samples
                .iter()
                .zip(&self.tags)
                .filter(|(_, t)| **t == tag.tag())
                .map(|(s, _)| rig.triangulate_frame(&s.iter().map(|v| v.as_deref()).collect::<Vec<_>>()))
                .collect()
        };
        let at = |f: &[[f32; 4]], k: usize| f.get(k).filter(|p| p[0].is_finite()).map(|p| Vector3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])));
        let mut checks = Vec::new();
        for (step, wrist, shoulder, name) in [(crate::Step::RaiseLeftHand, 9, 5, "Left hand reads up"), (crate::Step::RaiseRightHand, 10, 6, "Right hand reads up")] {
            let lift: Vec<f64> = frames(step).iter().filter_map(|f| Some(at(f, wrist)?[1] - at(f, shoulder)?[1])).collect();
            if let Some(m) = median(lift) {
                checks.push(Check { name, passed: m > 0.2, detail: format!("wrist {:.2} m above the shoulder", m) });
            }
        }
        let span: Vec<f64> = frames(crate::Step::ArmsOut).iter().filter_map(|f| Some((at(f, 9)? - at(f, 10)?).norm())).collect();
        if let Some(m) = median(span) {
            let expected = 1.48 * body_scale;
            checks.push(Check { name: "Arm span matches the body", passed: (m / expected - 1.0).abs() < 0.25, detail: format!("{:.2} m, expected about {:.2} m", m, expected) });
        }
        checks
    }
}

/// The factor that makes measured bone lengths match the priors (median ratio).
fn prior_scale(points: &[[Option<Vector3<f64>>; KEYPOINTS]], body_scale: f64) -> Option<f64> {
    let mut ratios: Vec<f64> = Vec::new();
    for sample in points {
        for &(a, b, length) in &BONES {
            if let (Some(p), Some(q)) = (sample[a], sample[b]) {
                let d = distance(&p, &q);
                if d > 1e-9 {
                    ratios.push(length * body_scale / d);
                }
            }
        }
    }
    if ratios.len() < 10 {
        return None;
    }
    ratios.sort_by(f64::total_cmp);
    Some(ratios[ratios.len() / 2])
}

/// Bones whose far end is above the near end when the subject stands, with how
/// much each says about "up".
const UP_CUES: [(usize, usize, f64); 8] = [(11, 5, 3.0), (12, 6, 3.0), (13, 11, 1.5), (14, 12, 1.5), (15, 13, 1.0), (16, 14, 1.0), (5, 0, 1.0), (6, 0, 1.0)];
/// Left and right counterparts, with how much each says about "left".
const LEFT_CUES: [(usize, usize, f64); 6] = [(5, 6, 3.0), (11, 12, 2.0), (3, 4, 1.0), (13, 14, 1.0), (15, 16, 1.0), (1, 2, 0.5)];
/// A keypoint and its height above the floor for a 1.75 m adult, best cue first.
const FLOOR_CUES: [(&[usize], f64); 5] = [(&[15, 16], 0.08), (&[13, 14], 0.50), (&[11, 12], 0.92), (&[5, 6], 1.43), (&[0], 1.62)];

/// Re-expresses the world on the subject: +Y up, the floor at 0, the origin under
/// the subject, +Z the way the subject faces, left at +X. Every direction is
/// averaged from whatever bones the cameras saw, so no camera needs the whole
/// body: up from any bones that point up, left from any left-right pairs, the
/// floor from the lowest body part seen.
fn align_world(cams: &mut [CameraModel], points: &[[Option<Vector3<f64>>; KEYPOINTS]], body_scale: f64) -> Result<(), CalibrationError> {
    let (mut up, mut left) = (Vector3::zeros(), Vector3::zeros());
    let (mut up_cues, mut left_cues) = (0usize, 0usize);
    for p in points {
        for &(lower, upper, weight) in &UP_CUES {
            if let (Some(a), Some(b)) = (p[lower], p[upper]) {
                if let Some(d) = (b - a).try_normalize(1e-9) {
                    up += d * weight;
                    up_cues += 1;
                }
            }
        }
    }
    if up_cues < 5 || up.norm() < 1e-9 {
        return Err(CalibrationError::Degenerate);
    }
    let y = up.normalize();
    for p in points {
        for &(l, r, weight) in &LEFT_CUES {
            if let (Some(a), Some(b)) = (p[l], p[r]) {
                let d = a - b;
                let flat = d - y * d.dot(&y);
                if let Some(d) = flat.try_normalize(1e-6 * d.norm().max(1e-9)) {
                    left += d * weight;
                    left_cues += 1;
                }
            }
        }
    }
    if left_cues < 5 {
        return Err(CalibrationError::Degenerate);
    }
    let x = (left - y * left.dot(&y)).try_normalize(1e-9).ok_or(CalibrationError::Degenerate)?;
    let z = x.cross(&y);
    let rw = Matrix3::from_rows(&[x.transpose(), y.transpose(), z.transpose()]);
    // The floor: per sample, the lowest cue available, taken as its usual height above the floor.
    let mut floors: Vec<f64> = Vec::new();
    let (mut centre, mut seen) = (Vector3::zeros(), 0usize);
    for p in points {
        let known: Vec<Vector3<f64>> = p.iter().flatten().copied().collect();
        if known.is_empty() {
            continue;
        }
        centre += known.iter().sum::<Vector3<f64>>() / known.len() as f64;
        seen += 1;
        for (ids, height) in FLOOR_CUES {
            let heights: Vec<f64> = ids.iter().filter_map(|&k| p[k]).map(|q| y.dot(&q)).collect();
            if let Some(low) = heights.iter().copied().reduce(f64::min) {
                floors.push(low - height * body_scale);
                break;
            }
        }
    }
    if seen == 0 || floors.is_empty() {
        return Err(CalibrationError::Degenerate);
    }
    let centre = centre / seen as f64;
    let floor = median(floors).unwrap_or(0.0);
    let origin = centre - y * y.dot(&centre) + y * floor;
    for cam in cams.iter_mut() {
        cam.translation += cam.rotation * origin;
        cam.rotation *= rw.transpose();
    }
    Ok(())
}
