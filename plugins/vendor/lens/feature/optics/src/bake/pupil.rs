//! Pupil footprints: the unvignetted aperture shape per field (cat's eye).

use super::{field_for, BakeConfig};
use crate::prescription::{OpticsError, Prescription};
use crate::trace::Tracer;

/// Coverage masks of the stop disk after all clear apertures.
///
/// Layout of `data`: `data[(f * resolution + v) * resolution + u]` for field
/// `field_r[f]`, texel column `u`, row `v`. Texel centres map to stop
/// coordinates `x = -1 + 2 (u + 0.5) / resolution`, `y = -1 + 2 (v + 0.5) /
/// resolution` where the stop disk is the unit disk, **+x is the radial
/// (tangential) direction towards the corner**, +y is sagittal. A value is the
/// fraction (supersampled) of the texel that is transmitted: `1` open, `0`
/// blocked or outside the stop. As a texture: `resolution x (resolution *
/// n_fields)` `R32Float`, or an array texture with `n_fields` layers.
///
/// Multiply by the stop's physical radius (or the blur radius in pixels) to
/// scale the mask to a bokeh disc. Fields the lens cannot image are all zero.
#[derive(Clone, Debug, PartialEq)]
pub struct PupilFootprints {
    pub resolution: usize,
    pub n_fields: usize,
    pub field_r: Vec<f32>,
    pub f_number: f32,
    pub data: Vec<f32>,
}

impl PupilFootprints {
    /// Mask of field `f` as a `resolution * resolution` slice.
    pub fn mask(&self, f: usize) -> &[f32] {
        let n = self.resolution * self.resolution;
        &self.data[f * n..(f + 1) * n]
    }

    /// Open area of field `f` relative to the field-0 mask area.
    pub fn relative_area(&self, f: usize) -> f32 {
        let a0: f32 = self.mask(0).iter().sum();
        let a: f32 = self.mask(f).iter().sum();
        if a0 > 0.0 {
            a / a0
        } else {
            0.0
        }
    }

    /// Raw bytes for upload.
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.data)
    }
}

/// Bake the footprints at the reference wavelength.
pub fn bake_footprints(
    rx: &Prescription,
    config: &BakeConfig,
) -> Result<PupilFootprints, OpticsError> {
    let res = config.footprint_resolution.max(2);
    let ss = config.footprint_supersample.max(1);
    let fno = config.footprint_f_number.unwrap_or(rx.f_number);
    let base = Tracer::new(rx, config.reference_wavelength_nm)?;
    let tracer = Tracer::new(rx, config.reference_wavelength_nm)?.with_f_number(fno);
    let dist = config.focus_distance_mm;
    let mut data = vec![0.0f32; config.footprint_fields.len() * res * res];
    for (f, &r) in config.footprint_fields.iter().enumerate() {
        let Some(field) = field_for(&base, r, dist) else {
            continue;
        };
        for v in 0..res {
            for u in 0..res {
                let mut hit = 0usize;
                for sj in 0..ss {
                    for si in 0..ss {
                        let x =
                            -1.0 + 2.0 * (u as f64 + (si as f64 + 0.5) / ss as f64) / res as f64;
                        let y =
                            -1.0 + 2.0 * (v as f64 + (sj as f64 + 0.5) / ss as f64) / res as f64;
                        if x * x + y * y <= 1.0
                            && tracer
                                .aim(field, [x, y])
                                .ok()
                                .and_then(|ray| tracer.trace(ray).ok())
                                .is_some()
                        {
                            hit += 1;
                        }
                    }
                }
                data[(f * res + v) * res + u] = hit as f32 / (ss * ss) as f32;
            }
        }
    }
    Ok(PupilFootprints {
        resolution: res,
        n_fields: config.footprint_fields.len(),
        field_r: config.footprint_fields.iter().map(|r| *r as f32).collect(),
        f_number: fno as f32,
        data,
    })
}
