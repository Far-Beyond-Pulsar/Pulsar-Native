//! What a host gets without authoring any lens data: the example
//! prescriptions of `helio-lens-optics` as a [`PrescriptionBank`], a mask
//! array holding a procedural water-drop layer and a few iris shapes.
//!
//! [`crate::LensPass::with_bundled_resources`] uploads and registers all
//! three ([`PRESCRIPTIONS`](crate::PRESCRIPTIONS), [`MASKS`](crate::MASKS),
//! [`APERTURE_SHAPES`](crate::APERTURE_SHAPES)); an editor offers the same
//! lenses by [`BundledLens`] and layers by the constants below, and the CPU
//! overscan of [`crate::ray_map`] reads their ray maps from
//! [`bundled_rays`].
//!
//! The bakes are traced once per process, on demand or in the background
//! ([`start_baking`]), and shared.

use crate::masks::water_drop_mask;
use crate::prescriptions::{pack_rays, PrescriptionRays};
use helio_lens_optics::{examples, BakeConfig, LensBake, Prescription};
use std::sync::OnceLock;

/// A lens of the bundled bank. The order is the bank's layer order; append,
/// never reorder (authored levels store the index).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BundledLens {
    /// Idealized thin lens, 50 mm f/2: the identity map with `cos^4` falloff.
    ThinLens50F2,
    /// Cooke triplet, 50 mm f/5.
    CookeTriplet50F5,
    /// Six-element double Gauss, 50 mm f/2: barrel distortion, lateral colour,
    /// cat's-eye bokeh, coma.
    #[default]
    DoubleGauss50F2,
}

impl BundledLens {
    pub const ALL: [Self; 3] = [
        Self::ThinLens50F2,
        Self::CookeTriplet50F5,
        Self::DoubleGauss50F2,
    ];

    /// The layer of the bundled [`crate::PrescriptionBank`], for
    /// `LensElementRow::prescription`.
    pub fn layer(self) -> u32 {
        self as u32
    }

    pub fn from_layer(layer: u32) -> Option<Self> {
        Self::ALL.get(layer as usize).copied()
    }

    /// The traced prescription.
    pub fn prescription(self) -> Prescription {
        match self {
            Self::ThinLens50F2 => examples::thin_lens(),
            Self::CookeTriplet50F5 => examples::cooke_triplet(),
            Self::DoubleGauss50F2 => examples::double_gauss(),
        }
    }
}

/// Layer of the bundled mask array holding the water drops: normals in red and
/// green, the cap's height in blue, the footprint in alpha. The `Rainy lens`
/// preset's [`crate::LensRegion::MaskChannel`] and normal layer.
pub const WATER_DROP_MASK_LAYER: u32 = 0;
/// Edge in texels of the bundled masks.
pub const MASK_SIZE: u32 = 512;
/// Edge in texels of the bundled iris shapes.
pub const APERTURE_SHAPE_SIZE: u32 = 128;

/// An iris shape of the bundled array, for `pupil::TEXTURE_LAYER`. Same
/// rules as [`BundledLens`]: append only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BundledAperture {
    /// Five-pointed star.
    #[default]
    Star,
    Heart,
    Triangle,
}

impl BundledAperture {
    pub const ALL: [Self; 3] = [Self::Star, Self::Heart, Self::Triangle];

    pub fn layer(self) -> u32 {
        self as u32
    }

    /// Coverage (0..255, y down) of the shape on a `size` x `size` square
    /// around the pupil circle: the shape fits the circle, one texel of
    /// antialiasing at its edge.
    pub fn coverage(self, size: u32) -> Vec<u8> {
        let n = size.max(1);
        let mut out = Vec::with_capacity((n * n) as usize);
        for y in 0..n {
            for x in 0..n {
                // Pupil units: the circle is the unit disc, y up.
                let p = [
                    (x as f32 + 0.5) / n as f32 * 2.0 - 1.0,
                    1.0 - (y as f32 + 0.5) / n as f32 * 2.0,
                ];
                let d = self.distance(p) * n as f32 * 0.5;
                out.push(((0.5 - d).clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
        }
        out
    }

    /// Signed distance to the shape (negative inside), approximately, in
    /// pupil units.
    fn distance(self, p: [f32; 2]) -> f32 {
        let length = |v: [f32; 2]| (v[0] * v[0] + v[1] * v[1]).sqrt();
        match self {
            Self::Star => {
                // Radius of a five-point star as a function of the angle,
                // between an inner and an outer radius.
                let angle = p[0].atan2(p[1]);
                let sector = std::f32::consts::TAU / 5.0;
                let t = ((angle / sector).rem_euclid(1.0) - 0.5).abs() * 2.0;
                let (outer, inner) = (0.98, 0.42);
                length(p) - (outer + (inner - outer) * t)
            }
            Self::Heart => {
                // (x^2 + y^2 - 1)^3 - x^2 y^3 = 0, fitted to the disc.
                let (x, y) = (p[0] * 1.18, p[1] * 1.18 + 0.1);
                let f = (x * x + y * y - 1.0).powi(3) - x * x * y.powi(3);
                f.signum() * f.abs().cbrt() * 0.5
            }
            Self::Triangle => {
                // Equilateral triangle inscribed in the circle, point up.
                let k = 3.0f32.sqrt();
                let q = [p[0].abs() - 0.86, p[1] + 0.5];
                let q = if q[0] + k * q[1] > 0.0 {
                    [(q[0] - k * q[1]) * 0.5, (-k * q[0] - q[1]) * 0.5]
                } else {
                    q
                };
                let x = q[0] - q[0].clamp(-1.72, 0.0);
                -length([x, q[1]]) * q[1].signum()
            }
        }
    }
}

/// The configuration the bundled lenses are baked with: the full default
/// (33 field samples, 7 wavelengths, 24x24 pupil), which is what the
/// prescriptions are authored for.
pub fn bundled_bake_config() -> BakeConfig {
    BakeConfig::default()
}

fn bakes() -> &'static OnceLock<Vec<LensBake>> {
    static BAKES: OnceLock<Vec<LensBake>> = OnceLock::new();
    &BAKES
}

fn rays() -> &'static OnceLock<Vec<PrescriptionRays>> {
    static RAYS: OnceLock<Vec<PrescriptionRays>> = OnceLock::new();
    &RAYS
}

fn bake_all() -> Vec<LensBake> {
    let config = bundled_bake_config();
    BundledLens::ALL
        .iter()
        .map(|lens| {
            LensBake::bake(&lens.prescription(), &config)
                .unwrap_or_else(|e| panic!("bundled lens {lens:?} bakes: {e:?}"))
        })
        .collect()
}

/// The bakes of [`BundledLens::ALL`], in layer order. The first call traces
/// them (blocking, concurrent callers wait for it): see [`start_baking`] to do
/// that off the calling thread.
pub fn bundled_bakes() -> &'static [LensBake] {
    bakes().get_or_init(bake_all)
}

/// The bakes if they are done; never blocks.
pub fn bundled_bakes_ready() -> Option<&'static [LensBake]> {
    bakes().get().map(Vec::as_slice)
}

/// Starts tracing the bundled lenses on a background thread, so a host can
/// register them without stalling its first frames. Returns at once.
pub fn start_baking() {
    if bakes().get().is_some() {
        return;
    }
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("lens-bake".into())
            .spawn(|| {
                bundled_bakes();
            });
        // Without a thread the first caller that needs the bakes does the work.
        drop(spawned);
    });
}

/// The ray maps of the bundled lenses, in layer order, for the CPU twin of the
/// ray map ([`crate::ray_map::RayMap::with_rays`]). Blocks on the bakes like
/// [`bundled_bakes`].
pub fn bundled_rays() -> &'static [PrescriptionRays] {
    rays().get_or_init(|| pack_rays(&bundled_bakes().iter().collect::<Vec<_>>()))
}

/// The bundled mask array's layers as RGBA8 images of [`MASK_SIZE`]: layer
/// [`WATER_DROP_MASK_LAYER`] is a procedural water-drop layer.
pub fn bundled_mask_layers() -> Vec<Vec<u8>> {
    vec![water_drop_mask(MASK_SIZE, 70, [0.008, 0.035], 7)]
}

/// The bundled iris shapes as coverage images of [`APERTURE_SHAPE_SIZE`], in
/// [`BundledAperture`] order.
pub fn bundled_aperture_layers() -> Vec<Vec<u8>> {
    BundledAperture::ALL
        .iter()
        .map(|shape| shape.coverage(APERTURE_SHAPE_SIZE))
        .collect()
}
