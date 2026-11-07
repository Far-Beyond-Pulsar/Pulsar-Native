//! Lookup-table bakes. Every table is a plain struct of dimensions plus a flat
//! `Vec<f32>` whose layout is documented on the struct (and in the README), so
//! a GPU pass can upload `bytemuck::cast_slice(&table.data)` as a texture or
//! storage buffer without further processing.
//!
//! Conventions shared by all bakes:
//! * **Normalized field `r`** is the sensor radius divided by
//!   `Prescription::sensor_half_diagonal_mm`; `0` is the axis, `1` the corner.
//!   Tables are sampled at `r_i = i / (n - 1)` unless a `field_r` array says
//!   otherwise.
//! * Lenses are treated as rotationally symmetric: the field point sits on the
//!   +x axis and the rest of the sensor follows by rotation. "Tangential" is
//!   the radial (x) direction on the sensor, "sagittal" the azimuthal (y).
//! * Lengths are millimetres, wavelengths nanometres, z positive towards the
//!   sensor.

mod focus;
mod pupil;
mod ray_map;
mod vignetting;

pub use focus::{bake_focus, bake_psf, FocusData, PsfTable, PSF_FIXED_FIELDS};
pub use pupil::{bake_footprints, PupilFootprints};
pub use ray_map::{bake_ray_map, RayMapLut};
pub use vignetting::{bake_vignetting, VignettingLut};

use crate::glass::D_LINE_NM;
use crate::prescription::{OpticsError, Prescription};
use crate::spectral::SpectralSampling;
use crate::trace::{Field, Ray, Tracer};

/// Everything the bakes need besides the prescription.
#[derive(Clone, Debug)]
pub struct BakeConfig {
    /// Wavelength samples and RGB weights for the ray map.
    pub spectral: SpectralSampling,
    /// Wavelength used for single-wavelength tables (vignetting, footprints,
    /// PSF, field curvature).
    pub reference_wavelength_nm: f64,
    /// Field samples of the ray map and vignetting tables.
    pub field_samples: usize,
    /// Fields for field curvature and the PSF table.
    pub analysis_fields: usize,
    /// Absolute f-numbers of the vignetting table; empty selects the nominal
    /// f-number followed by the standard full stops above it.
    pub f_numbers: Vec<f64>,
    /// Pupil grid resolution (per axis) for vignetting and PSF/focus fans.
    pub pupil_samples: usize,
    /// Normalized fields of the pupil-footprint masks.
    pub footprint_fields: Vec<f64>,
    /// Footprint mask resolution (per axis).
    pub footprint_resolution: usize,
    /// Footprint supersampling per axis per texel.
    pub footprint_supersample: usize,
    /// f-number used for the footprints (`None` = nominal).
    pub footprint_f_number: Option<f64>,
    /// Defocus offsets (mm, + towards +z behind the sensor) of the PSF table.
    pub defocus_mm: Vec<f64>,
    /// Radial energy profile bins of the PSF table.
    pub psf_profile_bins: usize,
    /// Distance to the focus plane. `None` focuses at infinity; `Some(d)`
    /// refocuses the sensor paraxially for an object `d` mm in front of the
    /// first vertex and uses object points in that plane.
    pub focus_distance_mm: Option<f64>,
}

impl Default for BakeConfig {
    fn default() -> Self {
        BakeConfig {
            spectral: SpectralSampling::default(),
            reference_wavelength_nm: D_LINE_NM,
            field_samples: 33,
            analysis_fields: 9,
            f_numbers: Vec::new(),
            pupil_samples: 24,
            footprint_fields: vec![0.0, 0.5, 0.75, 1.0],
            footprint_resolution: 32,
            footprint_supersample: 2,
            footprint_f_number: None,
            defocus_mm: vec![-0.4, -0.2, -0.1, 0.0, 0.1, 0.2, 0.4],
            psf_profile_bins: 16,
            focus_distance_mm: None,
        }
    }
}

impl BakeConfig {
    /// A cheap configuration for tests and previews.
    pub fn coarse() -> Self {
        BakeConfig {
            spectral: SpectralSampling::uniform(3),
            field_samples: 9,
            analysis_fields: 5,
            pupil_samples: 12,
            footprint_fields: vec![0.0, 1.0],
            footprint_resolution: 16,
            footprint_supersample: 1,
            defocus_mm: vec![-0.2, 0.0, 0.2],
            psf_profile_bins: 8,
            ..Default::default()
        }
    }

    pub(crate) fn resolved_f_numbers(&self, nominal: f64) -> Vec<f64> {
        if !self.f_numbers.is_empty() {
            return self.f_numbers.clone();
        }
        let mut v = vec![nominal];
        v.extend(
            [1.0, 1.4, 2.0, 2.8, 4.0, 5.6, 8.0, 11.0, 16.0]
                .into_iter()
                .filter(|f| *f > nominal * 1.05),
        );
        v
    }
}

/// All bakes together.
#[derive(Clone, Debug)]
pub struct LensBake {
    pub ray_map: RayMapLut,
    pub vignetting: VignettingLut,
    pub footprints: PupilFootprints,
    pub focus: FocusData,
    pub psf: PsfTable,
}

impl LensBake {
    /// Run every bake. The prescription is first refocused when
    /// `config.focus_distance_mm` is set.
    pub fn bake(rx: &Prescription, config: &BakeConfig) -> Result<Self, OpticsError> {
        let focused;
        let rx = match config.focus_distance_mm {
            Some(d) => {
                focused = rx.focused(
                    d,
                    crate::paraxial::FocusMode::Sensor,
                    config.reference_wavelength_nm,
                )?;
                &focused
            }
            None => rx,
        };
        Ok(LensBake {
            ray_map: bake_ray_map(rx, config)?,
            vignetting: bake_vignetting(rx, config)?,
            footprints: bake_footprints(rx, config)?,
            focus: bake_focus(rx, config)?,
            psf: bake_psf(rx, config)?,
        })
    }
}

/// Cell-centred samples of an `n x n` grid that fall inside the unit disk.
pub(crate) fn pupil_grid(n: usize) -> Vec<[f64; 2]> {
    let n = n.max(1);
    let mut v = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            let u = -1.0 + (2 * i + 1) as f64 / n as f64;
            let w = -1.0 + (2 * j + 1) as f64 / n as f64;
            if u * u + w * w <= 1.0 {
                v.push([u, w]);
            }
        }
    }
    v
}

/// Trace the pupil `grid` for `field`; returns the sensor rays of those that
/// pass every clear aperture (`passed`) and the number attempted.
pub(crate) fn trace_pupil(tracer: &Tracer, field: Field, grid: &[[f64; 2]]) -> Vec<Ray> {
    grid.iter()
        .filter_map(|uv| {
            tracer
                .aim(field, *uv)
                .ok()
                .and_then(|r| tracer.trace(r).ok())
        })
        .collect()
}

/// Field tangent for normalized field `r`, or `None` outside lens coverage.
pub(crate) fn field_for(tracer: &Tracer, r: f64, focus_distance: Option<f64>) -> Option<Field> {
    let h = r * tracer.prescription().sensor_half_diagonal_mm;
    let tan = tracer.tan_for_height(h, focus_distance).ok()?;
    Some(Field::meridional(tan, focus_distance))
}
