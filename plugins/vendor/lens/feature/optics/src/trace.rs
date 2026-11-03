//! Real (non-paraxial) ray tracing through a prescription.
//!
//! Coordinates: the optical axis is +z, the first vertex is at z = 0, x is the
//! meridional (radial field) direction for rotationally symmetric bakes and
//! lengths are millimetres. A [`Tracer`] fixes the wavelength (the per-surface
//! indices are resolved once in [`Tracer::new`]) so [`Tracer::trace`] itself
//! does not allocate.

use crate::paraxial::Paraxial;
use crate::prescription::{OpticsError, Prescription};

pub type V3 = [f64; 3];

#[inline]
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn normalize(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}

/// A ray: a point and a unit direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: V3,
    pub dir: V3,
}

/// Why a ray stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RayFailKind {
    /// Missed the surface (outside the conic's domain).
    Miss,
    /// Landed outside the clear semi-aperture.
    Aperture,
    /// Total internal reflection.
    TotalInternalReflection,
    /// The ray turned around (non-positive path length / direction).
    Backward,
    /// Pupil aiming did not converge.
    AimFailed,
}

/// A terminated ray: the failing surface index (`surfaces.len()` = sensor) and
/// the reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RayFail {
    pub surface: usize,
    pub kind: RayFailKind,
}

/// Vector Snell refraction. `n` is any unit normal (its sign is irrelevant),
/// `eta = n1 / n2`. Returns the unit transmitted direction or `None` on total
/// internal reflection.
pub fn refract(d: V3, n: V3, eta: f64) -> Option<V3> {
    let mut nn = n;
    let mut cos_i = dot(d, n);
    if cos_i < 0.0 {
        nn = [-n[0], -n[1], -n[2]];
        cos_i = -cos_i;
    }
    let k = 1.0 - eta * eta * (1.0 - cos_i * cos_i);
    if k < 0.0 {
        return None;
    }
    let g = k.sqrt() - eta * cos_i;
    Some(normalize([
        eta * d[0] + g * nn[0],
        eta * d[1] + g * nn[1],
        eta * d[2] + g * nn[2],
    ]))
}

/// Where the light comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Field {
    /// Object at infinity; `tan` is the tangent of the field angle in x and y
    /// (direction `normalize(tan_x, tan_y, 1)`).
    Infinity { tan: [f64; 2] },
    /// Object point at transverse position `height` in a plane `distance`
    /// in front of the first vertex.
    Point { height: [f64; 2], distance: f64 },
}

impl Field {
    /// Meridional field of tangent `tan` (x only): the chief ray travels
    /// towards +x with slope `tan` and lands at +x on the sensor. With
    /// `distance` the object sits in a plane at that distance, at height
    /// `-distance * tan` (a real lens inverts the image).
    pub fn meridional(tan: f64, distance: Option<f64>) -> Field {
        match distance {
            None => Field::Infinity { tan: [tan, 0.0] },
            Some(d) => Field::Point {
                height: [-d * tan, 0.0],
                distance: d,
            },
        }
    }
}

/// A prescription bound to one wavelength, stop scale and clipping mode.
#[derive(Clone, Debug)]
pub struct Tracer<'a> {
    rx: &'a Prescription,
    wavelength_nm: f64,
    n_after: Vec<f64>,
    z: Vec<f64>,
    stop: usize,
    stop_scale: f64,
    clip: bool,
    sensor_z: f64,
    launch_margin: f64,
    ep_z: f64,
    ep_r: f64,
}

impl<'a> Tracer<'a> {
    /// Bind `rx` at `wavelength_nm`. Clipping against clear apertures is on.
    pub fn new(rx: &'a Prescription, wavelength_nm: f64) -> Result<Self, OpticsError> {
        rx.validate()?;
        // Pupil data only seeds ray aiming; afocal systems (plates, prisms)
        // have no finite paraxial pupils, so fall back to the stop itself.
        let (ep_z, ep_r) = match Paraxial::new(rx, wavelength_nm) {
            Ok(px) => (px.entrance_pupil_z, px.entrance_pupil_radius),
            Err(_) => (0.0, rx.surfaces[rx.stop_index()].semi_aperture),
        };
        let mut n_after = Vec::with_capacity(rx.surfaces.len());
        for (i, s) in rx.surfaces.iter().enumerate() {
            n_after.push(
                s.glass
                    .index_at(wavelength_nm)
                    .ok_or(OpticsError::UnknownGlass { surface: i })?,
            );
        }
        let max_ap = rx
            .surfaces
            .iter()
            .map(|s| s.semi_aperture)
            .fold(0.0, f64::max);
        Ok(Tracer {
            rx,
            wavelength_nm,
            n_after,
            z: rx.vertex_z(),
            stop: rx.stop_index(),
            stop_scale: 1.0,
            clip: true,
            sensor_z: rx.sensor_z(),
            launch_margin: 1.0 + 2.0 * max_ap,
            ep_z,
            ep_r,
        })
    }

    /// Multiply the stop radius (a stop scale `s` gives f-number `F / s`).
    pub fn with_stop_scale(mut self, scale: f64) -> Self {
        self.stop_scale = scale;
        self
    }

    /// Set the stop for an absolute f-number using the prescription's nominal
    /// `f_number` metadata.
    pub fn with_f_number(self, f_number: f64) -> Self {
        let s = self.rx.f_number / f_number;
        self.with_stop_scale(s)
    }

    /// Enable or disable clipping against clear semi-apertures in
    /// [`Tracer::trace`].
    pub fn with_clip(mut self, clip: bool) -> Self {
        self.clip = clip;
        self
    }

    pub fn wavelength_nm(&self) -> f64 {
        self.wavelength_nm
    }

    pub fn prescription(&self) -> &Prescription {
        self.rx
    }

    /// Current stop radius in mm.
    pub fn stop_radius(&self) -> f64 {
        self.rx.surfaces[self.stop].semi_aperture * self.stop_scale
    }

    /// Absolute z of the sensor plane.
    pub fn sensor_z(&self) -> f64 {
        self.sensor_z
    }

    #[inline]
    fn n_before(&self, i: usize) -> f64 {
        if i == 0 {
            1.0
        } else {
            self.n_after[i - 1]
        }
    }

    #[inline]
    fn aperture(&self, i: usize) -> f64 {
        let a = self.rx.surfaces[i].semi_aperture;
        if i == self.stop {
            a * self.stop_scale
        } else {
            a
        }
    }

    /// Intersect surface `i`, returning the local hit point (z is the sag).
    #[inline]
    fn hit(&self, i: usize, ray: &Ray) -> Result<(f64, V3), RayFail> {
        let s = &self.rx.surfaces[i];
        let o = [ray.origin[0], ray.origin[1], ray.origin[2] - self.z[i]];
        let t = s.intersect(o, ray.dir).ok_or(RayFail {
            surface: i,
            kind: RayFailKind::Miss,
        })?;
        Ok((
            t,
            [
                o[0] + t * ray.dir[0],
                o[1] + t * ray.dir[1],
                o[2] + t * ray.dir[2],
            ],
        ))
    }

    /// Intersect, optionally clip and refract at surface `i`.
    fn step(&self, i: usize, ray: &mut Ray, clip: bool) -> Result<(), RayFail> {
        let s = &self.rx.surfaces[i];
        let (_, p) = self.hit(i, ray)?;
        if clip {
            let a = self.aperture(i);
            if p[0] * p[0] + p[1] * p[1] > a * a * (1.0 + 1e-12) {
                return Err(RayFail {
                    surface: i,
                    kind: RayFailKind::Aperture,
                });
            }
        }
        ray.origin = [p[0], p[1], p[2] + self.z[i]];
        if let Some(f) = s.thin_focal_length {
            if ray.dir[2] <= 0.0 {
                return Err(RayFail {
                    surface: i,
                    kind: RayFailKind::Backward,
                });
            }
            let tx = ray.dir[0] / ray.dir[2] - p[0] / f;
            let ty = ray.dir[1] / ray.dir[2] - p[1] / f;
            ray.dir = normalize([tx, ty, 1.0]);
        } else {
            let n1 = self.n_before(i);
            let n2 = self.n_after[i];
            if n1 != n2 {
                let n = s.normal(p[0], p[1]);
                ray.dir = refract(ray.dir, n, n1 / n2).ok_or(RayFail {
                    surface: i,
                    kind: RayFailKind::TotalInternalReflection,
                })?;
            }
            if ray.dir[2] <= 0.0 {
                return Err(RayFail {
                    surface: i,
                    kind: RayFailKind::Backward,
                });
            }
        }
        Ok(())
    }

    fn run<F: FnMut(usize, &Ray)>(
        &self,
        mut ray: Ray,
        clip: bool,
        mut visit: F,
    ) -> Result<Ray, RayFail> {
        for i in 0..self.rx.surfaces.len() {
            self.step(i, &mut ray, clip)?;
            visit(i, &ray);
        }
        let t = (self.sensor_z - ray.origin[2]) / ray.dir[2];
        if t.is_nan() || t < 0.0 {
            return Err(RayFail {
                surface: self.rx.surfaces.len(),
                kind: RayFailKind::Backward,
            });
        }
        ray.origin = [
            ray.origin[0] + t * ray.dir[0],
            ray.origin[1] + t * ray.dir[1],
            self.sensor_z,
        ];
        Ok(ray)
    }

    /// Trace to the sensor plane. The result's origin is the sensor hit and
    /// its direction the arrival direction. Clips against clear apertures when
    /// clipping is enabled.
    pub fn trace(&self, ray: Ray) -> Result<Ray, RayFail> {
        self.run(ray, self.clip, |_, _| {})
    }

    /// Like [`Tracer::trace`] but ignoring every clear aperture (used to find
    /// chief rays and for aberration-only analysis).
    pub fn trace_unclipped(&self, ray: Ray) -> Result<Ray, RayFail> {
        self.run(ray, false, |_, _| {})
    }

    /// Trace calling `visit(surface, ray_after_surface)` at each surface.
    pub fn trace_with<F: FnMut(usize, &Ray)>(&self, ray: Ray, visit: F) -> Result<Ray, RayFail> {
        self.run(ray, self.clip, visit)
    }

    /// Where an (unclipped) ray crosses the stop plane: local `(x, y)`.
    pub fn stop_hit(&self, ray: Ray) -> Result<[f64; 2], RayFail> {
        let mut r = ray;
        for i in 0..self.stop {
            self.step(i, &mut r, false)?;
        }
        let (_, p) = self.hit(self.stop, &r)?;
        Ok([p[0], p[1]])
    }

    fn make_ray(&self, field: &Field, p: [f64; 2]) -> Ray {
        match *field {
            Field::Infinity { tan } => {
                let dir = normalize([tan[0], tan[1], 1.0]);
                let k = self.launch_margin / dir[2];
                Ray {
                    origin: [p[0] - dir[0] * k, p[1] - dir[1] * k, -dir[2] * k],
                    dir,
                }
            }
            Field::Point { height, distance } => {
                let o = [height[0], height[1], -distance];
                Ray {
                    origin: o,
                    dir: normalize([p[0] - o[0], p[1] - o[1], -o[2]]),
                }
            }
        }
    }

    /// Build the ray from `field` that crosses the stop at `stop_uv * R_stop`
    /// (`stop_uv` in the unit disk), solving for the launch point with a 2D
    /// Newton iteration on the real (unclipped) trace.
    pub fn aim(&self, field: Field, stop_uv: [f64; 2]) -> Result<Ray, RayFail> {
        let rs = self.stop_radius();
        let target = [stop_uv[0] * rs, stop_uv[1] * rs];
        let rep = self.ep_r * self.stop_scale;
        let e = [stop_uv[0] * rep, stop_uv[1] * rep, self.ep_z];
        // Paraxial initial guess at the z = 0 plane.
        let mut p = match field {
            Field::Infinity { tan } => [e[0] - tan[0] * e[2], e[1] - tan[1] * e[2]],
            Field::Point { height, distance } => {
                let k = distance / (distance + e[2]);
                [
                    height[0] + (e[0] - height[0]) * k,
                    height[1] + (e[1] - height[1]) * k,
                ]
            }
        };
        let fail = RayFail {
            surface: self.stop,
            kind: RayFailKind::AimFailed,
        };
        let tol = 1e-9 * rs.max(1e-3);
        let eval = |p: [f64; 2]| -> Result<[f64; 2], RayFail> {
            let h = self.stop_hit(self.make_ray(&field, p))?;
            Ok([h[0] - target[0], h[1] - target[1]])
        };
        let mut r = eval(p)?;
        let step_h = 1e-6
            * self
                .rx
                .surfaces
                .iter()
                .map(|s| s.semi_aperture)
                .fold(1.0, f64::max);
        for _ in 0..40 {
            if r[0].abs().max(r[1].abs()) < tol {
                return Ok(self.make_ray(&field, p));
            }
            let rx_ = eval([p[0] + step_h, p[1]])?;
            let ry_ = eval([p[0], p[1] + step_h])?;
            let j = [
                [(rx_[0] - r[0]) / step_h, (ry_[0] - r[0]) / step_h],
                [(rx_[1] - r[1]) / step_h, (ry_[1] - r[1]) / step_h],
            ];
            let det = j[0][0] * j[1][1] - j[0][1] * j[1][0];
            if det.abs() < 1e-18 {
                return Err(fail);
            }
            let dp = [
                (j[1][1] * r[0] - j[0][1] * r[1]) / det,
                (-j[1][0] * r[0] + j[0][0] * r[1]) / det,
            ];
            // Backtrack if the full step leaves the lens or increases error.
            let mut lambda = 1.0;
            let mut accepted = false;
            for _ in 0..8 {
                let np = [p[0] - lambda * dp[0], p[1] - lambda * dp[1]];
                if let Ok(nr) = eval(np) {
                    if nr[0].hypot(nr[1]) < r[0].hypot(r[1]) || nr[0].hypot(nr[1]) < tol {
                        p = np;
                        r = nr;
                        accepted = true;
                        break;
                    }
                }
                lambda *= 0.5;
            }
            if !accepted {
                return Err(fail);
            }
        }
        if r[0].abs().max(r[1].abs()) < 1e-6 * rs.max(1e-3) {
            Ok(self.make_ray(&field, p))
        } else {
            Err(fail)
        }
    }

    /// Chief ray (through the stop centre) for `field`, traced unclipped to the
    /// sensor.
    pub fn chief_ray(&self, field: Field) -> Result<Ray, RayFail> {
        let r = self.aim(field, [0.0, 0.0])?;
        self.trace_unclipped(r)
    }

    /// Radial image height (mm) of the chief ray of a meridional field, i.e.
    /// the sensor x coordinate.
    pub fn chief_height(&self, tan: f64, object_distance: Option<f64>) -> Result<f64, RayFail> {
        Ok(self
            .chief_ray(Field::meridional(tan, object_distance))?
            .origin[0])
    }

    /// Solve for the meridional field tangent whose chief ray lands at
    /// `height_mm` on the sensor. Fails (aim, TIR, miss) when the lens does not
    /// cover that height.
    pub fn tan_for_height(
        &self,
        height_mm: f64,
        object_distance: Option<f64>,
    ) -> Result<f64, RayFail> {
        if height_mm <= 0.0 {
            return Ok(0.0);
        }
        let mut lo = 0.0;
        let mut hi = 0.05;
        loop {
            match self.chief_height(hi, object_distance) {
                Ok(h) if h < height_mm => {
                    lo = hi;
                    hi *= 1.5;
                    if hi > 30.0 {
                        return Err(RayFail {
                            surface: self.rx.surfaces.len(),
                            kind: RayFailKind::AimFailed,
                        });
                    }
                }
                Ok(_) => break,
                Err(e) if lo == 0.0 => return Err(e),
                Err(_) => break,
            }
        }
        for _ in 0..70 {
            let mid = 0.5 * (lo + hi);
            match self.chief_height(mid, object_distance) {
                Ok(h) if h < height_mm => lo = mid,
                _ => hi = mid,
            }
        }
        let t = 0.5 * (lo + hi);
        // A failed bracket end would converge on the coverage limit; verify.
        match self.chief_height(t, object_distance) {
            Ok(h) if (h - height_mm).abs() < 1e-6 * height_mm.max(1e-3) => Ok(t),
            Ok(_) => Err(RayFail {
                surface: self.rx.surfaces.len(),
                kind: RayFailKind::AimFailed,
            }),
            Err(e) => Err(e),
        }
    }
}
