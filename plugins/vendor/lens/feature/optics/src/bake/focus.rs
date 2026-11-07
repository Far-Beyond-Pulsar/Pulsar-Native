//! Focus and defocus data: chromatic focus shift, field curvature, PSF
//! descriptors.

use super::{field_for, pupil_grid, trace_pupil, BakeConfig};
use crate::paraxial::Paraxial;
use crate::prescription::{OpticsError, Prescription};
use crate::trace::{Field, Ray, Tracer};

/// Focus position of the best-focus plane relative to the sensor plane, in mm
/// along +z (positive = behind the sensor).
///
/// * `axial_paraxial_mm[w]` / `axial_real_mm[w]`: on-axis focus shift for
///   `wavelengths_nm[w]`, paraxial and from a real-ray (full open stop)
///   least-squares fit. Their difference is spherical aberration; variation
///   with `w` is longitudinal chromatic aberration.
/// * `tangential_mm[i]`, `sagittal_mm[i]`, `best_mm[i]` at the reference
///   wavelength and normalized field `field_r[i]`: best focus of the
///   radial-direction spread, azimuthal spread and both together. Their
///   difference is astigmatism; `best_mm` against `i` is field curvature.
///
/// All arrays are plain `f32` vectors indexed as named.
#[derive(Clone, Debug, PartialEq)]
pub struct FocusData {
    pub reference_wavelength_nm: f32,
    pub wavelengths_nm: Vec<f32>,
    pub axial_paraxial_mm: Vec<f32>,
    pub axial_real_mm: Vec<f32>,
    pub n_field: usize,
    pub field_r: Vec<f32>,
    pub tangential_mm: Vec<f32>,
    pub sagittal_mm: Vec<f32>,
    pub best_mm: Vec<f32>,
}

/// Number of fixed scalars at the start of each PSF record.
pub const PSF_FIXED_FIELDS: usize = 5;

/// Compact geometric PSF descriptors per field and defocus.
///
/// Layout of `data`: `data[(i * n_defocus + k) * stride + c]` with
/// `stride = PSF_FIXED_FIELDS + n_profile`, field `field_r[i]`, defocus
/// `defocus_mm[k]` (plane shifted by that much along +z from the sensor), at
/// the reference wavelength, full open stop (`f_number`), rays uniform over
/// the stop disk and passing every clear aperture. Offsets are measured from
/// the chief ray's intersection with the shifted plane; `x` is tangential
/// (radial), `y` sagittal.
///
/// * `c = 0` `rms_mm`: RMS spot radius about the spot centroid.
/// * `c = 1` `rms_tangential_mm`: RMS in x about the centroid.
/// * `c = 2` `rms_sagittal_mm`: RMS in y about the centroid.
/// * `c = 3` `centroid_shift_mm`: centroid x minus chief ray x (signed; the
///   coma/lateral-shift direction, +x = towards the corner).
/// * `c = 4` `edge_radius_mm`: largest ray distance from the chief ray.
/// * `c = 5 + b` `profile[b]`: encircled energy, the fraction of rays within
///   `(b + 1) / n_profile * edge_radius_mm` of the chief ray (last bin = 1).
///   A profile that rises slowly then jumps at the edge is a "soap bubble"
///   bokeh disc (over-corrected spherical aberration); a front-loaded profile
///   is a soft, centre-bright disc.
///
/// Records with no transmitted rays (or fields outside coverage) are zero.
#[derive(Clone, Debug, PartialEq)]
pub struct PsfTable {
    pub n_field: usize,
    pub n_defocus: usize,
    pub n_profile: usize,
    pub stride: usize,
    pub f_number: f32,
    pub field_r: Vec<f32>,
    pub defocus_mm: Vec<f32>,
    pub data: Vec<f32>,
}

impl PsfTable {
    /// The record for field `i`, defocus `k`.
    pub fn record(&self, i: usize, k: usize) -> &[f32] {
        let o = (i * self.n_defocus + k) * self.stride;
        &self.data[o..o + self.stride]
    }

    /// Raw bytes for upload.
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.data)
    }
}

fn analysis_field_r(i: usize, n: usize) -> f64 {
    i as f64 / (n - 1).max(1) as f64
}

/// Least-squares focus shifts `(tangential, sagittal, both)` for sensor rays.
fn best_focus(rays: &[Ray]) -> Option<(f64, f64, f64)> {
    if rays.len() < 3 {
        return None;
    }
    let n = rays.len() as f64;
    let mean = |f: &dyn Fn(&Ray) -> f64| rays.iter().map(f).sum::<f64>() / n;
    let (mx, my) = (mean(&|r| r.origin[0]), mean(&|r| r.origin[1]));
    let (msx, msy) = (
        mean(&|r| r.dir[0] / r.dir[2]),
        mean(&|r| r.dir[1] / r.dir[2]),
    );
    let (mut cx, mut cy, mut sx, mut sy) = (0.0, 0.0, 0.0, 0.0);
    for r in rays {
        let (dx, dy) = (r.origin[0] - mx, r.origin[1] - my);
        let (ax, ay) = (r.dir[0] / r.dir[2] - msx, r.dir[1] / r.dir[2] - msy);
        cx += dx * ax;
        cy += dy * ay;
        sx += ax * ax;
        sy += ay * ay;
    }
    let t = if sx > 1e-30 { -cx / sx } else { 0.0 };
    let s = if sy > 1e-30 { -cy / sy } else { 0.0 };
    let b = if sx + sy > 1e-30 {
        -(cx + cy) / (sx + sy)
    } else {
        0.0
    };
    Some((t, s, b))
}

/// Bake chromatic focus shift and field curvature.
pub fn bake_focus(rx: &Prescription, config: &BakeConfig) -> Result<FocusData, OpticsError> {
    let waves = &config.spectral.wavelengths_nm;
    let grid = pupil_grid(config.pupil_samples);
    let dist = config.focus_distance_mm;
    let sensor_z = rx.sensor_z();
    let last_z = rx.vertex_z().last().copied().unwrap_or(0.0);

    let mut axial_paraxial = Vec::with_capacity(waves.len());
    let mut axial_real = Vec::with_capacity(waves.len());
    for &lambda in waves {
        let px = Paraxial::new(rx, lambda)?;
        let image = match dist {
            Some(d) => px.image_distance(d).unwrap_or(px.bfl),
            None => px.bfl,
        };
        axial_paraxial.push((last_z + image - sensor_z) as f32);
        let tracer = Tracer::new(rx, lambda)?;
        let field = Field::meridional(0.0, dist);
        let rays = trace_pupil(&tracer, field, &grid);
        axial_real.push(best_focus(&rays).map(|b| b.2 as f32).unwrap_or(f32::NAN));
    }

    let n = config.analysis_fields.max(2);
    let base = Tracer::new(rx, config.reference_wavelength_nm)?;
    let (mut tang, mut sag, mut best) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
    for i in 0..n {
        let Some(field) = field_for(&base, analysis_field_r(i, n), dist) else {
            continue;
        };
        let rays = trace_pupil(&base, field, &grid);
        if let Some((t, s, b)) = best_focus(&rays) {
            tang[i] = t as f32;
            sag[i] = s as f32;
            best[i] = b as f32;
        }
    }
    Ok(FocusData {
        reference_wavelength_nm: config.reference_wavelength_nm as f32,
        wavelengths_nm: waves.iter().map(|w| *w as f32).collect(),
        axial_paraxial_mm: axial_paraxial,
        axial_real_mm: axial_real,
        n_field: n,
        field_r: (0..n).map(|i| analysis_field_r(i, n) as f32).collect(),
        tangential_mm: tang,
        sagittal_mm: sag,
        best_mm: best,
    })
}

/// Bake the PSF descriptor table.
pub fn bake_psf(rx: &Prescription, config: &BakeConfig) -> Result<PsfTable, OpticsError> {
    let n = config.analysis_fields.max(2);
    let defocus = &config.defocus_mm;
    let nb = config.psf_profile_bins.max(1);
    let stride = PSF_FIXED_FIELDS + nb;
    let grid = pupil_grid(config.pupil_samples);
    let dist = config.focus_distance_mm;
    let tracer = Tracer::new(rx, config.reference_wavelength_nm)?;
    let mut data = vec![0.0f32; n * defocus.len() * stride];
    for i in 0..n {
        let Some(field) = field_for(&tracer, analysis_field_r(i, n), dist) else {
            continue;
        };
        let Ok(chief) = tracer.chief_ray(field) else {
            continue;
        };
        let rays = trace_pupil(&tracer, field, &grid);
        if rays.is_empty() {
            continue;
        }
        for (k, &dz) in defocus.iter().enumerate() {
            let at = |r: &Ray| {
                (
                    r.origin[0] + dz * r.dir[0] / r.dir[2],
                    r.origin[1] + dz * r.dir[1] / r.dir[2],
                )
            };
            let c = at(&chief);
            let pts: Vec<(f64, f64)> = rays
                .iter()
                .map(|r| {
                    let p = at(r);
                    (p.0 - c.0, p.1 - c.1)
                })
                .collect();
            let m = pts.len() as f64;
            let (mx, my) = (
                pts.iter().map(|p| p.0).sum::<f64>() / m,
                pts.iter().map(|p| p.1).sum::<f64>() / m,
            );
            let vx = pts.iter().map(|p| (p.0 - mx).powi(2)).sum::<f64>() / m;
            let vy = pts.iter().map(|p| (p.1 - my).powi(2)).sum::<f64>() / m;
            let radii: Vec<f64> = pts.iter().map(|p| p.0.hypot(p.1)).collect();
            let edge = radii.iter().cloned().fold(0.0, f64::max);
            let rec = &mut data[(i * defocus.len() + k) * stride..][..stride];
            rec[0] = (vx + vy).sqrt() as f32;
            rec[1] = vx.sqrt() as f32;
            rec[2] = vy.sqrt() as f32;
            rec[3] = mx as f32;
            rec[4] = edge as f32;
            for b in 0..nb {
                let lim = edge * (b + 1) as f64 / nb as f64;
                let inside = radii.iter().filter(|r| **r <= lim * (1.0 + 1e-12)).count();
                rec[PSF_FIXED_FIELDS + b] = inside as f32 / m as f32;
            }
        }
    }
    Ok(PsfTable {
        n_field: n,
        n_defocus: defocus.len(),
        n_profile: nb,
        stride,
        f_number: rx.f_number as f32,
        field_r: (0..n).map(|i| analysis_field_r(i, n) as f32).collect(),
        defocus_mm: defocus.iter().map(|d| *d as f32).collect(),
        data,
    })
}
