//! CPU twin of the lens's ray map (`ray_map` in `lens.wgsl`), for hosts that
//! need the map before the frame is rendered, chiefly
//! [`required_overscan`], and for tests that check the shader against the
//! optics instead of against itself.
//!
//! The map takes a sensor position `q` (image-height units about the frame
//! centre, y down) to the position on the rendered rectilinear image the
//! sensor looks at, for one wavelength. The two implementations are kept
//! line for line; the tests compare them on the GPU.

use crate::components::*;
use crate::prescriptions::{PrescriptionBank, PrescriptionRays};

const MAX_ANGLE: f32 = 1.4835;
const WATER_INDEX: f32 = 1.333;
/// Largest overscan reported; the GPU caps it the same way. A camera applies
/// at most [`APPLIED_OVERSCAN_CAP`] of it.
pub const OVERSCAN_CAP: f32 = 16.0;
/// Most a camera widens its field of view by for a lens ([`applied_overscan`]).
/// Overscanning renders the same pixel count over a wider view, so the final
/// frame has `1 / overscan` of the pixels per unit of angle at its centre: 2
/// halves the effective resolution, which is as much as the picture keeps
/// recognisable. A map that reaches further (an extreme fisheye) clamps to the
/// edge of the widened image as it always did.
pub const APPLIED_OVERSCAN_CAP: f32 = 2.0;
/// Overscan below this factor is not applied (an identity map measures a few
/// ulp above 1).
const OVERSCAN_EPSILON: f32 = 1.0e-3;

/// One wavelength sample: its position `t = (lambda - 550 nm) / 150 nm` and
/// the weight of each colour channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectralSample {
    pub t: f32,
    pub weights: [f32; 3],
}

/// Wavelength samples of a `quality` (0: 3, 1: 7, 2 and up: 16).
pub fn spectral_sample_count(quality: u32) -> usize {
    match quality {
        0 => 3,
        1 => 7,
        _ => 16,
    }
}

/// Response of the red, green and blue channels at `lambda` nanometres.
pub(crate) fn sensor_weights(lambda: f32) -> [f32; 3] {
    let lobe = |center: f32, width: f32| {
        let x = (lambda - center) / width;
        (-0.5 * x * x).exp()
    };
    [lobe(610.0, 40.0), lobe(545.0, 35.0), lobe(460.0, 30.0)]
}

/// `n` wavelength samples over 400..700 nm with the channels' weights
/// normalised to sum to 1 per channel, so a neutral lens reconstructs the
/// image exactly.
pub fn spectral_samples(n: usize) -> Vec<SpectralSample> {
    let n = n.clamp(1, 16);
    if n == 1 {
        return vec![SpectralSample {
            t: 0.0,
            weights: [1.0; 3],
        }];
    }
    let lambdas: Vec<f32> = (0..n)
        .map(|j| 400.0 + (j as f32 + 0.5) * 300.0 / n as f32)
        .collect();
    let raw: Vec<[f32; 3]> = lambdas.iter().map(|&l| sensor_weights(l)).collect();
    let mut total = [0.0f32; 3];
    for w in &raw {
        for c in 0..3 {
            total[c] += w[c];
        }
    }
    lambdas
        .iter()
        .zip(&raw)
        .map(|(&l, w)| SpectralSample {
            t: (l - 550.0) / 150.0,
            weights: [w[0] / total[0], w[1] / total[1], w[2] / total[2]],
        })
        .collect()
}

type V2 = [f32; 2];

fn bounded(x: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if x.is_finite() {
        x.clamp(lo, hi)
    } else {
        fallback
    }
}
fn length(v: V2) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}
fn dot(a: V2, b: V2) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}
fn mix(a: V2, b: V2, t: f32) -> V2 {
    [a[0] * (1.0 - t) + b[0] * t, a[1] * (1.0 - t) + b[1] * t]
}
/// The bank layer a `PRESCRIPTION` element names (`rx_header`).
fn prescription_layer(e: &LensElementRow) -> usize {
    (bounded(e.params[0], 0.0, 4096.0, 0.0) + 0.5) as usize
}
/// Ray-map elements a covering prescription replaces (`rx_replaces_map`).
fn replaced_by_prescription(kind: u32) -> bool {
    (element_kind::PROJECTION..=element_kind::ANAMORPHIC_DISTORTION).contains(&kind)
        || kind == element_kind::LATERAL_CA
}
fn port_index(n: f32) -> f32 {
    if n > 0.0 {
        n.clamp(1.05, 2.5)
    } else {
        WATER_INDEX
    }
}
fn along(x: V2, r: f32, theta: f32) -> V2 {
    if r < 1.0e-5 {
        return x;
    }
    let k = theta.clamp(0.0, MAX_ANGLE).tan() / r;
    [x[0] * k, x[1] * k]
}

fn kannala_brandt_angle(p: &[f32; 16], r: f32) -> f32 {
    let mut theta = r;
    for _ in 0..8 {
        let t2 = theta * theta;
        let g = theta * (1.0 + t2 * (p[1] + t2 * (p[2] + t2 * (p[3] + t2 * p[4])))) - r;
        let d = 1.0 + t2 * (3.0 * p[1] + t2 * (5.0 * p[2] + t2 * (7.0 * p[3] + t2 * (9.0 * p[4]))));
        if d.abs() < 1.0e-4 {
            break;
        }
        theta = (theta - g / d).clamp(0.0, MAX_ANGLE);
    }
    theta
}

fn panini_inverse(p: &[f32; 16], x: V2) -> V2 {
    let d = p[1].clamp(0.0, 8.0);
    let s = p[2].clamp(0.0, 1.0);
    let k = x[0] / (d + 1.0);
    let phi = ((k * d / (1.0 + k * k).sqrt()).clamp(-1.0, 1.0).asin() + k.atan())
        .clamp(-MAX_ANGLE, MAX_ANGLE);
    let c = phi.cos();
    let squeeze = (d + 1.0) / (d + c);
    let vertical = x[1] / (1.0 * (1.0 - s) + squeeze * s);
    [phi.tan(), vertical / c]
}

fn apply_projection(p: &[f32; 16], x: V2) -> V2 {
    let r = length(x);
    let theta = match (bounded(p[0], 0.0, 15.0, 0.0) + 0.5) as u32 {
        1 => r,
        2 => 2.0 * (r * 0.5).min(1.0).asin(),
        3 => r.min(1.0).asin(),
        4 => 2.0 * (r * 0.5).atan(),
        5 => kannala_brandt_angle(p, r),
        6 => return panini_inverse(p, x),
        7 => {
            let phi = x[0].clamp(-MAX_ANGLE, MAX_ANGLE);
            return [phi.tan(), x[1] / phi.cos()];
        }
        _ => return x,
    };
    along(x, r, theta)
}

fn brown_conrady_forward(p: &[f32; 16], x: V2) -> V2 {
    let r2 = dot(x, x);
    let r4 = r2 * r2;
    let r6 = r4 * r2;
    let numerator = 1.0 + p[0] * r2 + p[1] * r4 + p[2] * r6;
    let mut denominator = 1.0 + p[3] * r2 + p[4] * r4 + p[5] * r6;
    if denominator.abs() < 1.0e-3 {
        denominator = 1.0e-3;
    }
    let radial = numerator / denominator;
    let (p1, p2) = (p[6], p[7]);
    [
        x[0] * radial + 2.0 * p1 * x[0] * x[1] + p2 * (r2 + 2.0 * x[0] * x[0]),
        x[1] * radial + p1 * (r2 + 2.0 * x[1] * x[1]) + 2.0 * p2 * x[0] * x[1],
    ]
}

fn anamorphic_forward(p: &[f32; 16], x: V2) -> V2 {
    let r2 = dot(x, x);
    [
        x[0] * (1.0 + p[0] * r2 + p[1] * r2 * r2),
        x[1] * (1.0 + p[2] * r2 + p[3] * r2 * r2),
    ]
}

/// The OpenCV forward model of a [`element_kind::BROWN_CONRADY`] element:
/// where the sensor records the ideal point `x` (focal-length units).
pub fn brown_conrady_distort(params: &[f32; 16], x: [f32; 2]) -> [f32; 2] {
    brown_conrady_forward(params, x)
}

fn undistort(kind: u32, p: &[f32; 16], target: V2) -> V2 {
    let distort = |x: V2| {
        if kind == element_kind::BROWN_CONRADY {
            brown_conrady_forward(p, x)
        } else {
            anamorphic_forward(p, x)
        }
    };
    let mut x = target;
    let h = 1.0e-3;
    for _ in 0..8 {
        let d = distort(x);
        let f = [d[0] - target[0], d[1] - target[1]];
        let a = distort([x[0] + h, x[1]]);
        let b = distort([x[0] - h, x[1]]);
        let c = distort([x[0], x[1] + h]);
        let e = distort([x[0], x[1] - h]);
        let jx = [(a[0] - b[0]) / (2.0 * h), (a[1] - b[1]) / (2.0 * h)];
        let jy = [(c[0] - e[0]) / (2.0 * h), (c[1] - e[1]) / (2.0 * h)];
        let det = jx[0] * jy[1] - jy[0] * jx[1];
        if det.abs() < 1.0e-5 {
            break;
        }
        let step = [
            ((jy[1] * f[0] - jy[0] * f[1]) / det).clamp(-0.5, 0.5),
            ((jx[0] * f[1] - jx[1] * f[0]) / det).clamp(-0.5, 0.5),
        ];
        x = [x[0] - step[0], x[1] - step[1]];
    }
    x
}

fn flat_port_map(p: &[f32; 16], x: V2, t: f32) -> V2 {
    let n = (port_index(p[0]) - p[1] * t).max(1.01);
    let r = length(x);
    let sin_air = r / (1.0 + r * r).sqrt();
    along(x, r, (sin_air / n).clamp(0.0, 1.0).asin())
}

fn dome_port_map(p: &[f32; 16], x: V2, t: f32) -> V2 {
    let n = (port_index(p[2]) - p[3] * t).max(1.01);
    let ratio = (p[1] / p[0].max(1.0e-3)).clamp(-0.95, 0.95);
    let r = length(x);
    let theta = r.atan();
    let sin_incidence = ratio * theta.sin();
    along(
        x,
        r,
        theta - sin_incidence.clamp(-1.0, 1.0).asin() + (sin_incidence / n).clamp(-1.0, 1.0).asin(),
    )
}

/// The lens as the ray map sees it. Build with [`RayMap::new`].
#[derive(Clone, Debug)]
pub struct RayMap {
    elements: Vec<LensElementRow>,
    /// Focal-length units per image-height unit.
    scale: f32,
    focus_dpt: f32,
    focal_length_mm: f32,
    sensor_height_mm: f32,
    aspect: f32,
    f_number: f32,
    /// The bank's ray maps by layer, with [`RayMap::with_bank`].
    rays: Vec<PrescriptionRays>,
}

/// The first prism covering a position: its index in the stack and its
/// region weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrismSelection {
    pub index: usize,
    pub weight: f32,
    pub copies: u32,
}

impl RayMap {
    /// The map of `lens` and its `elements` (the lens's own stack, in order,
    /// not the whole buffer) for an image of the given width over height.
    /// Focus breathing follows `lens.focus_distance_m`: an autofocusing lens
    /// is resolved on the GPU only. `PRESCRIPTION` elements are inert, as on
    /// the GPU without a bank; [`Self::with_bank`] brings theirs.
    pub fn new(lens: &LensRow, elements: &[LensElementRow], aspect: f32) -> Self {
        Self::build(lens, elements, aspect, &[])
    }

    /// [`Self::new`] with the baked lenses of `bank`, see [`Self::with_rays`].
    pub fn with_bank(
        lens: &LensRow,
        elements: &[LensElementRow],
        aspect: f32,
        bank: &PrescriptionBank,
    ) -> Self {
        Self::build(lens, elements, aspect, bank.rays())
    }

    /// [`Self::new`] with the baked lenses `rays` (one per bank layer), so
    /// `PRESCRIPTION` elements act as they do on the GPU: they replace the
    /// parametric ray map where they cover, and a full-frame one sets the focal
    /// length to the bake's. `USER` elements stay unknown to the CPU: their
    /// WGSL runs on the GPU only, so a user ray map's reach is not part of the
    /// result.
    pub fn with_rays(
        lens: &LensRow,
        elements: &[LensElementRow],
        aspect: f32,
        rays: &[PrescriptionRays],
    ) -> Self {
        Self::build(lens, elements, aspect, rays)
    }

    fn build(
        lens: &LensRow,
        elements: &[LensElementRow],
        aspect: f32,
        rays: &[PrescriptionRays],
    ) -> Self {
        let mut focal = bounded(lens.focal_length_mm, 8.0, 600.0, 50.0);
        let sensor_width = bounded(lens.sensor_width_mm, 4.0, 70.0, 36.0);
        let sensor_height = if lens.sensor_height_mm > 0.0 {
            bounded(lens.sensor_height_mm, 3.0, 50.0, sensor_width * 2.0 / 3.0)
        } else {
            sensor_width * 2.0 / 3.0
        };
        let count = (lens.element_count as usize)
            .min(MAX_LENS_ELEMENTS as usize)
            .min(elements.len());
        let elements = &elements[..count];
        let mut f_number = bounded(lens.f_number, 0.7, 32.0, 2.8);
        let rays = rays.to_vec();
        // A prescription over the whole frame is the view's lens: its
        // effective focal length, and no wider an aperture than it was baked at.
        let full_frame = elements.iter().find_map(|e| {
            (e.kind == element_kind::PRESCRIPTION && e.region_kind == region_kind::FULL_FRAME)
                .then(|| rays.get(prescription_layer(e)))
                .flatten()
        });
        if let Some(lens_rays) = full_frame {
            focal = bounded(lens_rays.efl_mm(), 8.0, 600.0, focal);
            f_number = f_number.clamp(lens_rays.min_f_number().max(0.7), 32.0);
        }
        let focal_m = focal * 1.0e-3;
        let focus = bounded(lens.focus_distance_m, 0.0, 1.0e6, 1.0e6).max(focal_m * 1.001);
        Self {
            elements: elements.to_vec(),
            scale: sensor_width / aspect / focal,
            focus_dpt: 1.0 / focus.clamp(focal_m * 1.001, 1.0e6),
            focal_length_mm: focal,
            sensor_height_mm: sensor_height,
            aspect,
            f_number,
            rays,
        }
    }

    pub fn focal_length_mm(&self) -> f32 {
        self.focal_length_mm
    }

    pub fn sensor_height_mm(&self) -> f32 {
        self.sensor_height_mm
    }

    /// Focal-length units per image-height unit.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    fn region_distance(e: &LensElementRow, q: V2) -> f32 {
        let r = &e.region;
        match e.region_kind {
            region_kind::FULL_FRAME => -1.0e3,
            region_kind::HALF_PLANE => r[1] - dot(q, [r[0].cos(), r[0].sin()]),
            region_kind::ELLIPSE => {
                let radii = [r[2].max(1.0e-4), r[3].max(1.0e-4)];
                let p = [q[0] - r[0], q[1] - r[1]];
                let (c, s) = (r[4].cos(), r[4].sin());
                let local = [c * p[0] + s * p[1], -s * p[0] + c * p[1]];
                (length([local[0] / radii[0], local[1] / radii[1]]) - 1.0) * radii[0].min(radii[1])
            }
            region_kind::POLYGON => {
                let n = (e.region_count as usize).min(8);
                if n < 3 {
                    return 1.0e3;
                }
                let vertex = |i: usize| [r[2 * i], r[2 * i + 1]];
                let mut area = 0.0;
                for i in 0..n {
                    let (a, b) = (vertex(i), vertex((i + 1) % n));
                    area += a[0] * b[1] - b[0] * a[1];
                }
                let side = if area > 0.0 { 1.0 } else { -1.0 };
                let mut d = -1.0e3f32;
                for i in 0..n {
                    let (a, b) = (vertex(i), vertex((i + 1) % n));
                    let edge = [b[0] - a[0], b[1] - a[1]];
                    let len = length(edge).max(1.0e-6);
                    let outward = [side * edge[1] / len, -side * edge[0] / len];
                    d = d.max(dot([q[0] - a[0], q[1] - a[1]], outward));
                }
                d
            }
            region_kind::RADIAL_BAND => {
                let rr = length([q[0] - r[2], q[1] - r[3]]);
                (r[0] - rr).max(rr - r[1])
            }
            _ => 1.0e3,
        }
    }

    /// Coverage of the element's region at `q`, 0..1.
    pub fn region_weight(&self, e: &LensElementRow, q: V2) -> f32 {
        let feather = if e.feather >= 0.0 { e.feather } else { 0.002 };
        let t = (0.5 - Self::region_distance(e, q) / feather.max(1.0e-5)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    fn map_element(&self, e: &LensElementRow, x: V2, t: f32) -> V2 {
        let p = &e.params;
        match e.kind {
            element_kind::PROJECTION => apply_projection(p, x),
            element_kind::BROWN_CONRADY | element_kind::ANAMORPHIC_DISTORTION => {
                undistort(e.kind, p, x)
            }
            element_kind::DIVISION => {
                let r2 = dot(x, x);
                let k = (1.0 + p[0] * r2 + p[1] * r2 * r2).max(0.1);
                [x[0] / k, x[1] / k]
            }
            element_kind::ANAMORPHIC_SQUEEZE => {
                let s = if p[0] > 0.0 {
                    p[0].clamp(0.1, 8.0)
                } else {
                    1.0
                };
                [x[0] * s, x[1]]
            }
            element_kind::SHIFT => [x[0] + p[0] * self.scale, x[1] + p[1] * self.scale],
            element_kind::KEYSTONE => {
                let k = (1.0 + p[0] * x[0] + p[1] * x[1]).max(0.1);
                [x[0] / k, x[1] / k]
            }
            element_kind::FOCUS_BREATHING => {
                let k = (1.0 + p[0] * (self.focus_dpt - p[1])).max(0.1);
                [x[0] / k, x[1] / k]
            }
            element_kind::LATERAL_CA => {
                let k = (1.0 + t * (p[0] + t * p[1])).max(0.1);
                [x[0] / k, x[1] / k]
            }
            element_kind::FLAT_PORT => flat_port_map(p, x, t),
            element_kind::DOME_PORT => dome_port_map(p, x, t),
            _ => x,
        }
    }

    fn prism_count(e: &LensElementRow) -> u32 {
        (bounded(e.params[0], 2.0, 8.0, 2.0) + 0.5) as u32
    }

    fn prism_copy(&self, e: &LensElementRow, x: V2, copy: u32) -> V2 {
        let p = &e.params;
        let n = Self::prism_count(e);
        let magnitude = bounded(p[1], -4.0, 4.0, 0.0) * self.scale;
        let angle = bounded(p[2], -100.0, 100.0, 0.0);
        let offset = if p[3] > 0.5 {
            let a = angle + 2.0 * std::f32::consts::PI * copy as f32 / n as f32;
            [a.cos() * magnitude, a.sin() * magnitude]
        } else {
            let along = (copy as f32 - 0.5 * (n - 1) as f32) * magnitude;
            [angle.cos() * along, angle.sin() * along]
        };
        let spin = bounded(p[4], -100.0, 100.0, 0.0) * copy as f32;
        let (c, s) = (spin.cos(), spin.sin());
        [
            c * x[0] - s * x[1] + offset[0],
            s * x[0] + c * x[1] + offset[1],
        ]
    }

    /// Weight of copy `copy` of a prism element: `ratio^copy`, normalised.
    pub fn prism_weight(e: &LensElementRow, copy: u32) -> f32 {
        let n = Self::prism_count(e);
        let ratio = if e.params[5] > 0.0 {
            bounded(e.params[5], 0.05, 20.0, 1.0)
        } else {
            1.0
        };
        let (mut total, mut mine, mut w) = (0.0, 1.0, 1.0);
        for i in 0..n {
            if i == copy {
                mine = w;
            }
            total += w;
            w *= ratio;
        }
        mine / total
    }

    /// The first prism element covering `q`.
    pub fn find_prism(&self, q: V2) -> Option<PrismSelection> {
        self.elements.iter().enumerate().find_map(|(index, e)| {
            if e.kind != element_kind::PRISM {
                return None;
            }
            let weight = self.region_weight(e, q);
            (weight > 0.0).then(|| PrismSelection {
                index,
                weight,
                copies: Self::prism_count(e),
            })
        })
    }

    /// The prism element of a selection.
    pub fn prism_element(&self, selection: &PrismSelection) -> &LensElementRow {
        &self.elements[selection.index]
    }

    /// The first `PRESCRIPTION` element (index in the stack) whose region
    /// covers `q`, with its coverage (`find_prescription` in `lens.wgsl`).
    fn find_prescription(&self, q: V2) -> Option<(usize, f32)> {
        self.elements.iter().enumerate().find_map(|(index, e)| {
            if e.kind != element_kind::PRESCRIPTION
                || self.rays.get(prescription_layer(e)).is_none()
            {
                return None;
            }
            let weight = self.region_weight(e, q);
            (weight > 0.0).then_some((index, weight))
        })
    }

    /// The scene position the sensor position `q` looks at for wavelength
    /// position `t`, through copy `copy` of the prism `prism` (if any).
    pub fn scene_position(&self, q: V2, t: f32, prism: Option<(usize, u32)>) -> V2 {
        let mut x = [q[0] * self.scale, q[1] * self.scale];
        // Where a prescription covers, its map replaces the parametric lens maps.
        let rx = self.find_prescription(q);
        let rx_weight = rx.map_or(0.0, |(_, w)| w);
        for (i, e) in self.elements.iter().enumerate() {
            if e.kind == element_kind::PRESCRIPTION {
                if let Some((index, weight)) = rx {
                    if index == i {
                        let rays = &self.rays[prescription_layer(e)];
                        x = mix(x, rays.map(x, t, self.focal_length_mm), weight);
                    }
                }
                continue;
            }
            if e.kind < element_kind::PROJECTION || e.kind > element_kind::DOME_PORT {
                continue;
            }
            if e.kind == element_kind::PRISM {
                if let Some((index, copy)) = prism {
                    if index == i {
                        x = self.prism_copy(e, x, copy);
                    }
                }
                continue;
            }
            let mut w = self.region_weight(e, q);
            if rx_weight > 0.0 && replaced_by_prescription(e.kind) {
                w *= 1.0 - rx_weight;
            }
            if w <= 0.0 {
                continue;
            }
            x = mix(x, self.map_element(e, x, t), w);
        }
        let scene = [x[0] / self.scale, x[1] / self.scale];
        if scene[0].abs() < 1.0e4 && scene[1].abs() < 1.0e4 {
            scene
        } else {
            q
        }
    }

    /// How far beyond the rendered frame the map reaches at the frame's
    /// boundary: the largest scene extent over the frame's half extent, per
    /// axis, over the spectrum's ends and every prism copy; at least 1.
    /// Sampled at 64 boundary points (16 per edge, corners included), like
    /// the GPU's `required_overscan`. A map that folds back inside the frame
    /// (extreme distortion) can reach further than its boundary does.
    pub fn required_overscan(&self) -> f32 {
        let mut reach = 1.0f32;
        for lid in 0..64u32 {
            let k = (lid % 16) as f32 / 16.0;
            let uv = match lid / 16 {
                0 => [k, 0.0],
                1 => [1.0, k],
                2 => [1.0 - k, 1.0],
                _ => [0.0, 1.0 - k],
            };
            let q = [(uv[0] - 0.5) * self.aspect, uv[1] - 0.5];
            let prism = self.find_prism(q);
            let copies = prism.map_or(1, |p| p.copies);
            for c in 0..copies {
                for j in 0..3 {
                    let s = self.scene_position(q, j as f32 - 1.0, prism.map(|p| (p.index, c)));
                    reach = reach.max((s[0].abs() / (0.5 * self.aspect)).max(s[1].abs() / 0.5));
                }
            }
        }
        if reach.is_finite() {
            reach.min(OVERSCAN_CAP)
        } else {
            OVERSCAN_CAP
        }
    }

    /// Natural + optical + hood + filter transmission at the sensor position
    /// `q`, per channel (`lens.wgsl` `transmission`). `t_stop` is the lens's.
    pub fn transmission(&self, q: V2, t_stop: f32) -> [f32; 3] {
        let ray = self.scene_position(q, 0.0, None);
        let ray = [ray[0] * self.scale, ray[1] * self.scale];
        let cos_squared = 1.0 / (1.0 + dot(ray, ray));
        let field_r = length(q) / (0.5 * (1.0 + self.aspect * self.aspect).sqrt());
        let mut t = [1.0f32; 3];
        let mul = |t: &mut [f32; 3], f: [f32; 3]| {
            for c in 0..3 {
                t[c] *= f[c];
            }
        };
        let lerp = |a: f32, b: f32, w: f32| a * (1.0 - w) + b * w;
        for e in &self.elements {
            if e.kind < element_kind::NATURAL_VIGNETTE || e.kind > element_kind::T_STOP_LOSS {
                continue;
            }
            let w = self.region_weight(e, q);
            if w <= 0.0 {
                continue;
            }
            let p = &e.params;
            match e.kind {
                element_kind::NATURAL_VIGNETTE => {
                    let natural =
                        lerp(1.0, cos_squared * cos_squared, bounded(p[0], 0.0, 1.0, 0.0));
                    mul(&mut t, [lerp(1.0, natural, w); 3]);
                }
                element_kind::OPTICAL_VIGNETTE => {
                    let mut barrel = if p[1] > 0.0 {
                        bounded(p[1], 0.5, 4.0, 1.0)
                    } else {
                        1.0
                    };
                    if p[2] > 0.0 {
                        barrel *= self.f_number / bounded(p[2], 0.7, 64.0, self.f_number);
                    }
                    let d = (bounded(p[0], 0.0, 1.5, 0.0) * field_r).min(1.8);
                    mul(&mut t, [lerp(1.0, circle_overlap(d, barrel), w); 3]);
                }
                element_kind::MECHANICAL_VIGNETTE => {
                    let feather = bounded(p[3], 0.0, 4.0, 0.02).max(1.0e-3);
                    let mut sd = length(q) - p[1];
                    if p[0] > 0.5 {
                        let (c, s) = (p[4].cos(), p[4].sin());
                        let local = [c * q[0] + s * q[1], -s * q[0] + c * q[1]];
                        let dd = [local[0].abs() - p[1], local[1].abs() - p[2]];
                        sd = length([dd[0].max(0.0), dd[1].max(0.0)]) + dd[0].max(dd[1]).min(0.0);
                    }
                    let u = (0.5 - sd / feather).clamp(0.0, 1.0);
                    mul(&mut t, [lerp(1.0, u * u * (3.0 - 2.0 * u), w); 3]);
                }
                element_kind::ND_FILTER => {
                    mul(&mut t, [(-bounded(p[0], 0.0, 32.0, 0.0) * w).exp2(); 3]);
                }
                element_kind::GRADUATED_ND => {
                    let soft = bounded(p[3], 0.0, 8.0, 0.0).max(0.002);
                    let n = [p[1].cos(), p[1].sin()];
                    let g = smoothstep(-0.5 * soft, 0.5 * soft, dot(q, n) - p[2]);
                    let stops = (-bounded(p[0], 0.0, 32.0, 0.0)).exp2();
                    let mut f = [1.0f32; 3];
                    for c in 0..3 {
                        f[c] = lerp(1.0, p[4 + c].clamp(0.0, 1.0) * stops, g * w);
                    }
                    mul(&mut t, f);
                }
                element_kind::COLOR_FILTER => {
                    let amount = bounded(p[3], 0.0, 1.0, 0.0) * w;
                    mul(
                        &mut t,
                        [0, 1, 2].map(|c| lerp(1.0, p[c].clamp(0.0, 1.0), amount)),
                    );
                }
                element_kind::T_STOP_LOSS => {
                    let ratio = self.f_number / t_stop.max(0.1);
                    mul(
                        &mut t,
                        [lerp(1.0, ratio * ratio, bounded(p[0], 0.0, 1.0, 0.0) * w); 3],
                    );
                }
                _ => {}
            }
        }
        t
    }
}

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Area of the unit circle inside a circle of radius `b` whose centre is `d`
/// away, as a fraction of the unit circle's area.
pub fn circle_overlap(d: f32, b: f32) -> f32 {
    if d >= 1.0 + b {
        return 0.0;
    }
    if d <= (1.0 - b).abs() {
        return (b * b).min(1.0);
    }
    let a1 = ((d * d + 1.0 - b * b) / (2.0 * d)).clamp(-1.0, 1.0).acos();
    let a2 = ((d * d + b * b - 1.0) / (2.0 * d * b))
        .clamp(-1.0, 1.0)
        .acos();
    let k = ((-d + 1.0 + b) * (d + 1.0 - b) * (d - 1.0 + b) * (d + 1.0 + b))
        .max(0.0)
        .sqrt();
    (a1 + b * b * a2 - 0.5 * k) / std::f32::consts::PI
}

/// How much wider than the rendered frame the lens's ray map reaches: the
/// factor by which a camera must widen its field of view (its tangent
/// extent, about the frame centre) so the map never samples beyond the
/// rendered image. 1 for a map that stays inside the frame, e.g. the
/// identity. `aspect` is the image's width over height; `elements` is the
/// lens's stack (`lens.element_first ..`). See [`RayMap::required_overscan`].
pub fn required_overscan(lens: &LensRow, elements: &[LensElementRow], aspect: f32) -> f32 {
    RayMap::new(lens, elements, aspect).required_overscan()
}

/// [`required_overscan`] with the baked lenses `rays` (one per bank layer, see
/// [`PrescriptionBank::rays`] and [`crate::bundled_rays`]), so `PRESCRIPTION`
/// elements count: the CPU twin of the GPU's `ResolvedLens::required_overscan`
/// for everything but `USER` elements and mask regions, which only the GPU
/// knows (a user ray map's reach is not measured here; document its overscan
/// need and let its host widen the camera).
pub fn required_overscan_with_rays(
    lens: &LensRow,
    elements: &[LensElementRow],
    aspect: f32,
    rays: &[PrescriptionRays],
) -> f32 {
    RayMap::with_rays(lens, elements, aspect, rays).required_overscan()
}

/// [`required_overscan_with_rays`] with the lenses of `bank`.
pub fn required_overscan_with_bank(
    lens: &LensRow,
    elements: &[LensElementRow],
    aspect: f32,
    bank: &PrescriptionBank,
) -> f32 {
    required_overscan_with_rays(lens, elements, aspect, bank.rays())
}

/// The overscan a camera applies for this lens: the factor the field of view's
/// tangent extent is widened by about the frame centre, and the value to write
/// to [`LensRow::overscan`] so the lens crops it back. It is the
/// required overscan, 1 for a map that stays in the frame (or a lens that is
/// off), at most [`APPLIED_OVERSCAN_CAP`]. The camera and the lens row must
/// use the same value; compute it once from the authored lens. `rays` are the
/// baked lenses `PRESCRIPTION` elements name (empty: they are inert).
pub fn applied_overscan(
    lens: &LensRow,
    elements: &[LensElementRow],
    aspect: f32,
    rays: &[PrescriptionRays],
) -> f32 {
    if lens.enabled == 0 {
        return 1.0;
    }
    let required = required_overscan_with_rays(lens, elements, aspect, rays);
    if required.is_finite() && required > 1.0 + OVERSCAN_EPSILON {
        required.min(APPLIED_OVERSCAN_CAP)
    } else {
        1.0
    }
}
