//! Pure-CPU physical lens model: trace real camera lens prescriptions and bake
//! lookup tables (ray map / distortion, vignetting, pupil footprints, focus
//! and PSF descriptors, spectral weights) for the Helio lens pass. No GPU
//! dependency. See README.md for the model, units and table layouts.

pub mod bake;
pub mod examples;
pub mod glass;
pub mod paraxial;
pub mod prescription;
pub mod spectral;
pub mod trace;

pub use bake::{
    bake_focus, bake_footprints, bake_psf, bake_ray_map, bake_vignetting, BakeConfig, FocusData,
    LensBake, PsfTable, PupilFootprints, RayMapLut, VignettingLut, PSF_FIXED_FIELDS,
};
pub use glass::{catalog_glass, catalog_names, Glass, C_LINE_NM, D_LINE_NM, F_LINE_NM};
pub use paraxial::{FocusMode, Paraxial};
pub use prescription::{OpticsError, Prescription, Surface};
pub use spectral::{rgb_response, SpectralSampling};
pub use trace::{refract, Field, Ray, RayFail, RayFailKind, Tracer};
