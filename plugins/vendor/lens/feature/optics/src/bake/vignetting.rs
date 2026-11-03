//! Relative illumination / vignetting LUT: field x f-number -> transmission.

use super::{field_for, pupil_grid, BakeConfig};
use crate::prescription::{OpticsError, Prescription};
use crate::trace::Tracer;

/// Fraction of on-axis light reaching each sensor radius, per f-number.
///
/// Layout of `data`: `data[j * n_field + i]` for f-number `f_numbers[j]` and
/// normalized sensor radius `r_i = i / (n_field - 1)`. Values lie in `[0, 1]`,
/// are exactly `1` on axis and non-increasing in `i`.
///
/// Each value is `natural * optical`:
/// * `natural = cos^4(theta')` with `theta'` the image-space chief-ray angle
///   (the classical cos^4 law evaluated on the real traced chief ray);
/// * `optical` = fraction of the stop disk (uniform grid) whose aimed rays
///   survive every clear aperture, divided by the same fraction on axis.
///
/// The optical term is made monotone with a running minimum along the field to
/// remove sampling noise; real lenses are monotone there anyway. As a texture:
/// width `n_field`, height `n_fnum`, `R32Float`.
#[derive(Clone, Debug, PartialEq)]
pub struct VignettingLut {
    pub n_field: usize,
    pub n_fnum: usize,
    pub f_numbers: Vec<f32>,
    pub data: Vec<f32>,
}

impl VignettingLut {
    /// Transmission at f-number sample `j`, field sample `i`.
    pub fn at(&self, j: usize, i: usize) -> f32 {
        self.data[j * self.n_field + i]
    }

    /// Raw bytes for upload.
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.data)
    }
}

/// Bake the vignetting table at the reference wavelength.
pub fn bake_vignetting(
    rx: &Prescription,
    config: &BakeConfig,
) -> Result<VignettingLut, OpticsError> {
    let n_field = config.field_samples.max(2);
    let f_numbers = config.resolved_f_numbers(rx.f_number);
    let grid = pupil_grid(config.pupil_samples);
    let dist = config.focus_distance_mm;
    let base = Tracer::new(rx, config.reference_wavelength_nm)?;

    // Field tangents come from the nominal stop; the chief ray does not
    // depend on the iris size.
    let fields: Vec<_> = (0..n_field)
        .map(|i| field_for(&base, i as f64 / (n_field - 1) as f64, dist))
        .collect();

    let mut data = vec![0.0f32; f_numbers.len() * n_field];
    for (j, &fno) in f_numbers.iter().enumerate() {
        let tracer = Tracer::new(rx, config.reference_wavelength_nm)?.with_f_number(fno);
        let mut on_axis = 1.0;
        let mut running = 1.0f64;
        for (i, field) in fields.iter().enumerate() {
            let t = match field {
                None => 0.0,
                Some(field) => {
                    let passed = grid
                        .iter()
                        .filter(|uv| {
                            tracer
                                .aim(*field, **uv)
                                .ok()
                                .and_then(|r| tracer.trace(r).ok())
                                .is_some()
                        })
                        .count();
                    let frac = passed as f64 / grid.len() as f64;
                    if i == 0 {
                        on_axis = frac.max(1e-12);
                    }
                    let natural = tracer
                        .chief_ray(*field)
                        .map(|r| r.dir[2].powi(4))
                        .unwrap_or(0.0);
                    running = running.min((frac / on_axis).min(1.0));
                    natural * running
                }
            };
            data[j * n_field + i] = t as f32;
        }
        // Natural falloff is monotone for a sane lens; enforce it anyway so
        // the table never brightens towards the corner.
        for i in 1..n_field {
            data[j * n_field + i] = data[j * n_field + i].min(data[j * n_field + i - 1]);
        }
        data[j * n_field] = 1.0f32.min(data[j * n_field]);
    }
    Ok(VignettingLut {
        n_field,
        n_fnum: f_numbers.len(),
        f_numbers: f_numbers.iter().map(|f| *f as f32).collect(),
        data,
    })
}
