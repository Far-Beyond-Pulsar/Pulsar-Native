//! First-order (paraxial) optics: ray-transfer matrices on `(y, n u)`.

use crate::prescription::{OpticsError, Prescription};

type M2 = [[f64; 2]; 2];

const IDENTITY: M2 = [[1.0, 0.0], [0.0, 1.0]];

/// `left * m`.
#[inline]
fn mul(left: M2, m: M2) -> M2 {
    [
        [
            left[0][0] * m[0][0] + left[0][1] * m[1][0],
            left[0][0] * m[0][1] + left[0][1] * m[1][1],
        ],
        [
            left[1][0] * m[0][0] + left[1][1] * m[1][0],
            left[1][0] * m[0][1] + left[1][1] * m[1][1],
        ],
    ]
}

#[inline]
fn refraction(power: f64) -> M2 {
    [[1.0, 0.0], [-power, 1.0]]
}

#[inline]
fn transfer(t_over_n: f64) -> M2 {
    [[1.0, t_over_n], [0.0, 1.0]]
}

/// First-order properties of a prescription at one wavelength. All lengths
/// are millimetres; positions are signed distances along +z.
#[derive(Clone, Debug)]
pub struct Paraxial {
    /// Wavelength these numbers were computed at.
    pub wavelength_nm: f64,
    /// Effective focal length (image medium assumed to be the glass after the
    /// last surface; air for every shipped lens).
    pub efl: f64,
    /// Back focal length: last vertex to the infinity focus.
    pub bfl: f64,
    /// Absolute z of the paraxial infinity focus.
    pub focus_z: f64,
    /// Absolute z of the sensor plane.
    pub sensor_z: f64,
    /// Entrance pupil position measured from the first vertex (negative =
    /// in front of it).
    pub entrance_pupil_z: f64,
    /// Entrance pupil semi-diameter for the nominal (open) stop.
    pub entrance_pupil_radius: f64,
    /// Exit pupil position measured from the last vertex.
    pub exit_pupil_z: f64,
    /// Exit pupil semi-diameter for the nominal stop.
    pub exit_pupil_radius: f64,
    /// `efl / (2 * entrance_pupil_radius)`.
    pub f_number: f64,
    /// Transfer matrix `[y, nu]` from the first vertex plane (before the first
    /// refraction) to just after the last refraction.
    pub system: [[f64; 2]; 2],
    n_last: f64,
}

impl Paraxial {
    /// Compute first-order data for `rx` at `wavelength_nm`.
    pub fn new(rx: &Prescription, wavelength_nm: f64) -> Result<Self, OpticsError> {
        rx.check(false)?;
        let n = rx.surfaces.len();
        let stop = rx.stop_index();
        let mut idx = Vec::with_capacity(n);
        for (i, s) in rx.surfaces.iter().enumerate() {
            idx.push(
                s.glass
                    .index_at(wavelength_nm)
                    .ok_or(OpticsError::UnknownGlass { surface: i })?,
            );
        }
        let power = |i: usize| -> f64 {
            let s = &rx.surfaces[i];
            match s.thin_focal_length {
                Some(f) => 1.0 / f,
                None => {
                    let n_before = if i == 0 { 1.0 } else { idx[i - 1] };
                    (idx[i] - n_before) * s.curvature()
                }
            }
        };
        let tn = |i: usize| rx.surfaces[i].thickness / idx[i];

        let mut pre = IDENTITY;
        for i in 0..stop {
            pre = mul(transfer(tn(i)), mul(refraction(power(i)), pre));
        }
        let mut total = IDENTITY;
        for i in 0..n {
            total = mul(refraction(power(i)), total);
            if i + 1 < n {
                total = mul(transfer(tn(i)), total);
            }
        }
        let mut post = IDENTITY;
        for i in stop..n {
            post = mul(refraction(power(i)), post);
            if i + 1 < n {
                post = mul(transfer(tn(i)), post);
            }
        }

        let n_last = idx[n - 1];
        let c = total[1][0];
        if c.abs() < 1e-14 {
            return Err(OpticsError::Degenerate(
                "afocal system has no finite focal length",
            ));
        }
        let efl = -n_last / c;
        let bfl = -total[0][0] * n_last / c;
        let z = rx.vertex_z();
        let last_z = z[n - 1];
        let stop_r = rx.surfaces[stop].semi_aperture;

        if pre[0][0].abs() < 1e-14 || post[1][1].abs() < 1e-14 {
            return Err(OpticsError::Degenerate("pupil at infinity"));
        }
        let ep_z = pre[0][1] / pre[0][0];
        let ep_r = stop_r / pre[0][0].abs();
        let xp_z = -post[0][1] / post[1][1];
        let xp_r = (stop_r * (post[0][0] + post[1][0] * xp_z)).abs();
        Ok(Paraxial {
            wavelength_nm,
            efl,
            bfl,
            focus_z: last_z + bfl,
            sensor_z: rx.sensor_z(),
            entrance_pupil_z: ep_z,
            entrance_pupil_radius: ep_r,
            exit_pupil_z: xp_z,
            exit_pupil_radius: xp_r,
            f_number: efl / (2.0 * ep_r),
            system: total,
            n_last,
        })
    }

    /// Distance from the last vertex to the paraxial image of an on-axis
    /// object `object_distance_mm` in front of the first vertex. `None` when
    /// the image is at infinity.
    pub fn image_distance(&self, object_distance_mm: f64) -> Option<f64> {
        let m = self.system;
        let y = m[0][0] * object_distance_mm + m[0][1];
        let nu = m[1][0] * object_distance_mm + m[1][1];
        (nu.abs() > 1e-14).then(|| -y * self.n_last / nu)
    }

    /// Absolute z of the paraxial image of an object at the given distance, in
    /// the same frame as `sensor_z` (first vertex at 0).
    pub fn image_z(&self, rx: &Prescription, object_distance_mm: f64) -> Option<f64> {
        let last = rx.vertex_z().last().copied().unwrap_or(0.0);
        self.image_distance(object_distance_mm).map(|d| last + d)
    }
}

/// How [`Prescription::focused`] moves things to focus on a near object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusMode {
    /// Move the sensor relative to the whole lens (unit focusing).
    Sensor,
    /// Slide surfaces `first..=last` (inclusive, `first >= 1`) along the axis,
    /// keeping the lens front and the sensor fixed (internal focusing).
    Group { first: usize, last: usize },
}

impl Prescription {
    /// Return a copy refocused so an on-axis object `object_distance_mm` in
    /// front of the first vertex is paraxially in focus on the sensor at
    /// `wavelength_nm`. `f64::INFINITY` focuses at infinity.
    pub fn focused(
        &self,
        object_distance_mm: f64,
        mode: FocusMode,
        wavelength_nm: f64,
    ) -> Result<Prescription, OpticsError> {
        let n = self.surfaces.len();
        let image_error = |rx: &Prescription| -> Result<f64, OpticsError> {
            let px = Paraxial::new(rx, wavelength_nm)?;
            let v = if object_distance_mm.is_finite() {
                px.image_distance(object_distance_mm)
                    .ok_or(OpticsError::Degenerate("image at infinity"))?
            } else {
                px.bfl
            };
            Ok(v - rx.surfaces[n - 1].thickness)
        };
        match mode {
            FocusMode::Sensor => {
                let mut rx = self.clone();
                let e = image_error(&rx)?;
                rx.surfaces[n - 1].thickness += e;
                if rx.surfaces[n - 1].thickness < 0.0 {
                    return Err(OpticsError::Invalid(
                        "focus would place sensor inside the lens".into(),
                    ));
                }
                Ok(rx)
            }
            FocusMode::Group { first, last } => {
                if first == 0 || last < first || last >= n {
                    return Err(OpticsError::Invalid("bad focus group range".into()));
                }
                let apply = |delta: f64| -> Prescription {
                    let mut rx = self.clone();
                    rx.surfaces[first - 1].thickness += delta;
                    rx.surfaces[last].thickness -= delta;
                    rx
                };
                let f = |d: f64| image_error(&apply(d));
                let (mut d0, mut d1) = (0.0, 0.05);
                let (mut f0, mut f1) = (f(d0)?, f(d1)?);
                for _ in 0..60 {
                    if f0.abs() < 1e-12 {
                        break;
                    }
                    if (f1 - f0).abs() < 1e-18 {
                        return Err(OpticsError::Degenerate("focus group has no effect"));
                    }
                    let d2 = d1 - f1 * (d1 - d0) / (f1 - f0);
                    d0 = d1;
                    f0 = f1;
                    d1 = d2;
                    f1 = f(d1)?;
                    if f1.abs() < 1e-12 {
                        d0 = d1;
                        f0 = f1;
                    }
                }
                if f0.abs() > 1e-9 {
                    return Err(OpticsError::Degenerate("focus solve did not converge"));
                }
                let rx = apply(d0);
                if rx.surfaces[first - 1].thickness < 0.0 || rx.surfaces[last].thickness < 0.0 {
                    return Err(OpticsError::Invalid(
                        "focus group travel exceeds available space".into(),
                    ));
                }
                Ok(rx)
            }
        }
    }
}
