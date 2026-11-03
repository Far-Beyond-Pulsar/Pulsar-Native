//! Ray map / distortion LUT: sensor radius x wavelength -> scene direction.

use super::BakeConfig;
use crate::prescription::{OpticsError, Prescription};
use crate::trace::Tracer;

/// Inverse (sensor -> scene) radial mapping per wavelength.
///
/// Layout of `data` (row-major, `f32`):
/// `data[(plane * n_wave + w) * n_field + i]`
/// * `plane 0`: **tan(theta)**, tangent of the scene field angle (object at
///   infinity) or `height / distance` of the focused object plane, whose chief
///   ray lands at normalized sensor radius `r_i = i / (n_field - 1)` for
///   wavelength `w`. Scene direction for a sensor pixel at polar angle `phi`
///   and radius `r` is `normalize(tan * cos phi, tan * sin phi, 1)`, linearly
///   interpolating between `i`.
/// * `plane 1`: relative distortion
///   `y / (s_ref * tan) - 1`, where `y` is the sensor radius in mm and
///   `s_ref` the small-field `y / tan` of the reference wavelength. The
///   reference wavelength therefore shows pure geometric distortion
///   (barrel < 0 < pincushion) and the difference between wavelengths is
///   lateral chromatic aberration. At `i = 0` the value is the limit for
///   vanishing field.
///
/// As a texture: width `n_field`, height `2 * n_wave`, one `R32Float` channel
/// (row = `plane * n_wave + w`).
///
/// Entries beyond `valid_field` (the lens cannot image that far) repeat the
/// last valid value.
#[derive(Clone, Debug, PartialEq)]
pub struct RayMapLut {
    pub n_field: usize,
    pub n_wave: usize,
    pub wavelengths_nm: Vec<f32>,
    pub reference_wavelength_nm: f32,
    pub sensor_half_diagonal_mm: f32,
    /// Largest normalized field with genuinely traced data, in `[0, 1]`.
    pub valid_field: f32,
    pub data: Vec<f32>,
}

impl RayMapLut {
    #[inline]
    fn index(&self, plane: usize, w: usize, i: usize) -> usize {
        (plane * self.n_wave + w) * self.n_field + i
    }

    /// Tangent of the scene angle for wavelength `w` at field sample `i`.
    pub fn tan_theta(&self, w: usize, i: usize) -> f32 {
        self.data[self.index(0, w, i)]
    }

    /// Relative distortion for wavelength `w` at field sample `i`.
    pub fn distortion(&self, w: usize, i: usize) -> f32 {
        self.data[self.index(1, w, i)]
    }

    /// Normalized field of sample `i`.
    pub fn field_r(&self, i: usize) -> f32 {
        i as f32 / (self.n_field - 1).max(1) as f32
    }

    /// Raw bytes for upload.
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.data)
    }
}

/// Bake the ray map for `rx` (already focused for `config.focus_distance_mm`).
pub fn bake_ray_map(rx: &Prescription, config: &BakeConfig) -> Result<RayMapLut, OpticsError> {
    let n_field = config.field_samples.max(2);
    let waves = &config.spectral.wavelengths_nm;
    let n_wave = waves.len();
    let dist = config.focus_distance_mm;
    let half_diag = rx.sensor_half_diagonal_mm;

    let ref_tracer = Tracer::new(rx, config.reference_wavelength_nm)?;
    let eps = 1e-3;
    let s_ref = ref_tracer
        .chief_height(eps, dist)
        .map_err(|_| OpticsError::Degenerate("reference chief ray failed on axis"))?
        / eps;

    let mut data = vec![0.0f32; 2 * n_wave * n_field];
    let mut valid = n_field - 1;
    for (w, &lambda) in waves.iter().enumerate() {
        let tracer = Tracer::new(rx, lambda)?;
        let mut last_ok = 0usize;
        let mut last = (0.0f32, 0.0f32);
        for i in 0..n_field {
            let y = half_diag * i as f64 / (n_field - 1) as f64;
            let entry = if i == 0 {
                tracer
                    .chief_height(eps, dist)
                    .ok()
                    .map(|h| (0.0, (h / (s_ref * eps) - 1.0) as f32))
            } else {
                tracer
                    .tan_for_height(y, dist)
                    .ok()
                    .map(|t| (t as f32, (y / (s_ref * t) - 1.0) as f32))
            };
            let (t, d) = match entry {
                Some(v) => {
                    last_ok = i;
                    last = v;
                    v
                }
                None => last,
            };
            data[(w) * n_field + i] = t;
            data[(n_wave + w) * n_field + i] = d;
        }
        valid = valid.min(last_ok);
    }
    Ok(RayMapLut {
        n_field,
        n_wave,
        wavelengths_nm: waves.iter().map(|w| *w as f32).collect(),
        reference_wavelength_nm: config.reference_wavelength_nm as f32,
        sensor_half_diagonal_mm: half_diag as f32,
        valid_field: valid as f32 / (n_field - 1) as f32,
        data,
    })
}
