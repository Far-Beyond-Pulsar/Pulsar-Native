//! Lens prescription: an ordered list of surfaces, serde load/save, validation.

use crate::glass::Glass;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Errors from loading, validating or tracing a prescription.
#[derive(Debug)]
pub enum OpticsError {
    Parse(serde_json::Error),
    /// The prescription has no surfaces.
    Empty,
    /// Exactly one surface must have `stop = true`.
    StopCount(usize),
    /// A surface glass cannot be evaluated (unknown catalog name, bad model).
    UnknownGlass {
        surface: usize,
    },
    /// A field holds a non-finite or out-of-range value.
    Invalid(String),
    /// The paraxial system is degenerate (e.g. afocal) for the requested query.
    Degenerate(&'static str),
}

impl fmt::Display for OpticsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpticsError::Parse(e) => write!(f, "prescription parse error: {e}"),
            OpticsError::Empty => write!(f, "prescription has no surfaces"),
            OpticsError::StopCount(n) => {
                write!(f, "prescription needs exactly one aperture stop, found {n}")
            }
            OpticsError::UnknownGlass { surface } => {
                write!(f, "surface {surface} has an unknown or invalid glass")
            }
            OpticsError::Invalid(s) => write!(f, "invalid prescription: {s}"),
            OpticsError::Degenerate(s) => write!(f, "degenerate optical system: {s}"),
        }
    }
}

impl std::error::Error for OpticsError {}

impl From<serde_json::Error> for OpticsError {
    fn from(e: serde_json::Error) -> Self {
        OpticsError::Parse(e)
    }
}

/// One refracting (or ideal, or stop) surface. Light travels towards +z; the
/// surface vertex sits at the running sum of previous `thickness` values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    /// Radius of curvature in mm, positive when the centre of curvature lies
    /// on the +z (image) side. `0` means flat (JSON cannot hold infinity).
    #[serde(default)]
    pub radius: f64,
    /// Distance in mm from this vertex to the next surface (or, for the last
    /// surface, to the sensor plane).
    pub thickness: f64,
    /// Medium *after* this surface.
    #[serde(default)]
    pub glass: Glass,
    /// Clear semi-aperture in mm; rays landing outside are vignetted. For the
    /// stop this is the open-iris radius.
    pub semi_aperture: f64,
    /// Conic constant `k` (0 = sphere, -1 = paraboloid).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub conic: f64,
    /// Even-asphere coefficients `A4, A6, A8, A10, A12` added to the conic sag
    /// as `sum A_{2p} r^{2p}`.
    #[serde(default, skip_serializing_if = "is_zero_arr")]
    pub aspheric: [f64; 5],
    /// Marks the aperture stop (exactly one per prescription).
    #[serde(default, skip_serializing_if = "is_false")]
    pub stop: bool,
    /// Turns the surface into an ideal paraxial thin lens of this focal length
    /// (mm): a plane that maps `tan'(angle) = tan(angle) - h / f` exactly.
    /// `radius`, `conic`, `aspheric` and index change are ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thin_focal_length: Option<f64>,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn is_zero_arr(v: &[f64; 5]) -> bool {
    v.iter().all(|x| *x == 0.0)
}
fn is_false(v: &bool) -> bool {
    !*v
}

impl Surface {
    /// A plain spherical surface.
    pub fn spherical(radius: f64, thickness: f64, glass: Glass, semi_aperture: f64) -> Self {
        Surface {
            radius,
            thickness,
            glass,
            semi_aperture,
            conic: 0.0,
            aspheric: [0.0; 5],
            stop: false,
            thin_focal_length: None,
        }
    }

    /// Curvature `1/R`, zero for flat surfaces.
    #[inline]
    pub fn curvature(&self) -> f64 {
        if self.radius == 0.0 || !self.radius.is_finite() {
            0.0
        } else {
            1.0 / self.radius
        }
    }

    #[inline]
    fn has_asphere(&self) -> bool {
        self.aspheric.iter().any(|a| *a != 0.0)
    }

    /// Sag `z(r^2)` of the full surface; `NaN` outside the conic's domain.
    pub fn sag(&self, r2: f64) -> f64 {
        let c = self.curvature();
        let mut z = if c == 0.0 {
            0.0
        } else {
            let s = (1.0 - (1.0 + self.conic) * c * c * r2).sqrt();
            c * r2 / (1.0 + s)
        };
        let mut rp = r2 * r2;
        for a in self.aspheric {
            z += a * rp;
            rp *= r2;
        }
        z
    }

    /// `dz / d(r^2)`.
    fn dsag_dr2(&self, r2: f64) -> f64 {
        let c = self.curvature();
        let mut d = if c == 0.0 {
            0.0
        } else {
            let s = (1.0 - (1.0 + self.conic) * c * c * r2).sqrt();
            c / (1.0 + s) + c * r2 * (1.0 + self.conic) * c * c / (2.0 * s * (1.0 + s) * (1.0 + s))
        };
        let mut rp = r2;
        let mut p = 2.0;
        for a in self.aspheric {
            d += a * p * rp;
            rp *= r2;
            p += 1.0;
        }
        d
    }

    /// Smallest positive ray parameter `t` where `o + t d` (vertex-local
    /// coordinates) meets the surface, or `None` on a miss.
    pub fn intersect(&self, o: [f64; 3], d: [f64; 3]) -> Option<f64> {
        let thin = self.thin_focal_length.is_some();
        let c = if thin { 0.0 } else { self.curvature() };
        let k1 = 1.0 + self.conic;
        // Quadric c (x^2 + y^2 + (1+k) z^2) - 2 z = 0.
        let a = c * (d[0] * d[0] + d[1] * d[1] + k1 * d[2] * d[2]);
        let b = 2.0 * (c * (o[0] * d[0] + o[1] * d[1] + k1 * o[2] * d[2]) - d[2]);
        let cc = c * (o[0] * o[0] + o[1] * o[1] + k1 * o[2] * o[2]) - 2.0 * o[2];
        let disc = b * b - 4.0 * a * cc;
        if disc < 0.0 {
            return None;
        }
        let den = -b + disc.sqrt();
        if den.abs() < 1e-300 {
            return None;
        }
        let mut t = 2.0 * cc / den;
        if !thin && self.has_asphere() {
            for _ in 0..12 {
                let p = [o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]];
                let r2 = p[0] * p[0] + p[1] * p[1];
                let f = p[2] - self.sag(r2);
                let fp = d[2] - self.dsag_dr2(r2) * 2.0 * (p[0] * d[0] + p[1] * d[1]);
                if !f.is_finite() || fp.abs() < 1e-300 {
                    return None;
                }
                let dt = f / fp;
                t -= dt;
                if dt.abs() < 1e-13 {
                    break;
                }
            }
        }
        (t.is_finite() && t > 0.0).then_some(t)
    }

    /// Unit surface normal at local `(x, y)`, pointing towards +z.
    pub fn normal(&self, x: f64, y: f64) -> [f64; 3] {
        if self.thin_focal_length.is_some() {
            return [0.0, 0.0, 1.0];
        }
        let g = 2.0 * self.dsag_dr2(x * x + y * y);
        let n = [-g * x, -g * y, 1.0];
        let l = (n[0] * n[0] + n[1] * n[1] + 1.0).sqrt();
        [n[0] / l, n[1] / l, n[2] / l]
    }
}

/// A complete sequential lens: metadata plus surfaces, object space is air.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prescription {
    pub name: String,
    /// Where the numbers came from.
    #[serde(default)]
    pub source: String,
    /// Nominal effective focal length in mm (metadata; the tracer computes the
    /// real value paraxially).
    pub focal_length_mm: f64,
    /// Nominal f-number at the open stop. Used to convert f-numbers into stop
    /// scales.
    pub f_number: f64,
    /// Half-diagonal of the sensor in mm; defines normalized field 1.0.
    pub sensor_half_diagonal_mm: f64,
    pub surfaces: Vec<Surface>,
}

impl Prescription {
    /// Parse and validate JSON.
    pub fn from_json(text: &str) -> Result<Self, OpticsError> {
        let p: Prescription = serde_json::from_str(text)?;
        p.validate()?;
        Ok(p)
    }

    /// Pretty-printed JSON.
    pub fn to_json(&self) -> Result<String, OpticsError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// An ideal thin lens at the stop with the sensor at its focal plane.
    pub fn ideal_thin_lens(
        focal_length_mm: f64,
        f_number: f64,
        sensor_half_diagonal_mm: f64,
    ) -> Self {
        Prescription {
            name: format!("Ideal thin lens {focal_length_mm} mm f/{f_number}"),
            source: "Idealized paraxial thin lens (zero aberration fallback)".into(),
            focal_length_mm,
            f_number,
            sensor_half_diagonal_mm,
            surfaces: vec![Surface {
                radius: 0.0,
                thickness: focal_length_mm,
                glass: Glass::Air,
                semi_aperture: focal_length_mm / (2.0 * f_number),
                conic: 0.0,
                aspheric: [0.0; 5],
                stop: true,
                thin_focal_length: Some(focal_length_mm),
            }],
        }
    }

    /// Check structural validity.
    pub fn validate(&self) -> Result<(), OpticsError> {
        self.check(true)
    }

    /// Validation that tolerates negative thicknesses (used while solving
    /// for focus travel, where intermediate trial states may overlap).
    pub(crate) fn check(&self, strict_thickness: bool) -> Result<(), OpticsError> {
        if self.surfaces.is_empty() {
            return Err(OpticsError::Empty);
        }
        let stops = self.surfaces.iter().filter(|s| s.stop).count();
        if stops != 1 {
            return Err(OpticsError::StopCount(stops));
        }
        for (i, s) in self.surfaces.iter().enumerate() {
            let finite = s.radius.is_finite()
                && s.thickness.is_finite()
                && s.conic.is_finite()
                && s.aspheric.iter().all(|a| a.is_finite());
            if !finite
                || (strict_thickness && s.thickness < 0.0)
                || s.semi_aperture.is_nan()
                || s.semi_aperture <= 0.0
            {
                return Err(OpticsError::Invalid(format!(
                    "surface {i}: non-finite value, negative thickness or non-positive aperture"
                )));
            }
            if let Some(f) = s.thin_focal_length {
                if !f.is_finite() || f == 0.0 {
                    return Err(OpticsError::Invalid(format!(
                        "surface {i}: bad thin focal length"
                    )));
                }
            }
            if s.glass.index_at(550.0).is_none() {
                return Err(OpticsError::UnknownGlass { surface: i });
            }
        }
        let bad = |v: f64| v.is_nan() || v <= 0.0;
        if bad(self.f_number) || bad(self.sensor_half_diagonal_mm) || bad(self.focal_length_mm) {
            return Err(OpticsError::Invalid(
                "focal length, f-number and sensor size must be positive".into(),
            ));
        }
        Ok(())
    }

    /// Index of the aperture stop surface.
    pub fn stop_index(&self) -> usize {
        self.surfaces.iter().position(|s| s.stop).unwrap_or(0)
    }

    /// Absolute z of every vertex; the first vertex is at z = 0.
    pub fn vertex_z(&self) -> Vec<f64> {
        let mut z = 0.0;
        self.surfaces
            .iter()
            .map(|s| {
                let v = z;
                z += s.thickness;
                v
            })
            .collect()
    }

    /// Absolute z of the sensor plane.
    pub fn sensor_z(&self) -> f64 {
        self.surfaces.iter().map(|s| s.thickness).sum()
    }

    /// Copy with every length multiplied by `factor` (radii, thicknesses,
    /// apertures, aspheric coefficients `A_2p` by `factor^(1-2p)`, focal
    /// length and sensor size).
    pub fn scaled(&self, factor: f64) -> Self {
        let mut p = self.clone();
        p.focal_length_mm *= factor;
        p.sensor_half_diagonal_mm *= factor;
        for s in &mut p.surfaces {
            s.radius *= factor;
            s.thickness *= factor;
            s.semi_aperture *= factor;
            s.thin_focal_length = s.thin_focal_length.map(|f| f * factor);
            for (j, a) in s.aspheric.iter_mut().enumerate() {
                *a *= factor.powi(1 - 2 * (j as i32 + 2));
            }
        }
        p
    }
}
