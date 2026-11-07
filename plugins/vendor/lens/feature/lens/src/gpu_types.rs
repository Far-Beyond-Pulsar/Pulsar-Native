//! Wire contract with other passes. The lens is resolved once per frame on
//! the GPU; consumers read the result instead of re-matching rows.

/// Graph storage buffer holding one [`ResolvedLens`], written every frame.
pub const RESOLVED_KEY: &str = "lens_resolved";

/// [`ResolvedLens::map_flags`]: some element remaps the image (projection,
/// distortion, shift, prism, ...), so the composite samples through the ray
/// map instead of copying texel for texel.
pub const MAP_REMAP: u32 = 1;
/// [`ResolvedLens::map_flags`]: a lateral chromatic aberration element is
/// present, so each wavelength sample has its own map.
pub const MAP_SPECTRAL: u32 = 2;
/// [`ResolvedLens::map_flags`]: some element attenuates light (vignetting,
/// filters, T-stop loss).
pub const MAP_TRANSMISSION: u32 = 4;
/// [`ResolvedLens::map_flags`]: a `PRESCRIPTION` element names a layer of the
/// bank, so ray map, transmission, pupil and focus consult the baked tables.
pub const MAP_PRESCRIPTION: u32 = 8;

/// [`ResolvedLens::aberration_flags`]: some in-focus aberration, filter or
/// contaminant element is present, so the composite writes the sensor image
/// to an intermediate and the pyramid and `cs_ab_apply` stages run.
pub const AB_ACTIVE: u32 = 1;

/// The view's lens after row matching, autofocus and sanitising, as
/// `cs_resolve` writes it. With no enabled row for the view it holds the
/// neutral 50 mm f/2.8 defaults of [`ResolvedLens::default`] and `enabled`
/// is 0, so optics consumers (the flare iris, field falloff) always have a
/// lens to read.
///
/// Consumers read leading fields by offset (the flare pass reads the first
/// six), so new fields are inserted behind the fields they extend and the
/// layout asserts below pin every offset.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ResolvedLens {
    pub f_number: f32,
    pub focal_length_mm: f32,
    pub sensor_width_mm: f32,
    /// 0 (circular) or 3..=16.
    pub aperture_blades: u32,
    /// Radians.
    pub aperture_rotation: f32,
    /// Amount (0..1) of natural `cos^4` falloff the lens applies to the image,
    /// from its `NATURAL_VIGNETTE` element: 0 with none. The flare pass uses
    /// it for analytic light sources, which are not in the image the lens
    /// already vignetted.
    pub natural_vignette: f32,
    /// 1 when an enabled row matched the view; 0 passes the image through.
    pub enabled: u32,
    /// 0 economical, 1 high, 2 ultra.
    pub quality: u32,
    pub t_stop: f32,
    pub sensor_height_mm: f32,
    /// After autofocus easing.
    pub focus_distance_m: f32,
    pub max_coc_px: f32,
    pub field_curvature: f32,
    pub tilt_angle: f32,
    pub tilt_axis: f32,
    pub element_first: u32,
    pub element_count: u32,
    /// How much wider than the rendered frame, as a factor on the field of
    /// view's extent in both axes about the frame centre, the ray map reaches
    /// at the sensor's edge: >= 1, exactly 1 for an identity map. A camera
    /// that renders `required_overscan` times the frame (then crops) gives
    /// the map real pixels everywhere; see the README.
    pub required_overscan: f32,
    /// [`MAP_REMAP`], [`MAP_SPECTRAL`], [`MAP_TRANSMISSION`].
    pub map_flags: u32,
    /// Wavelength samples of the composite: 1 without lateral chromatic
    /// aberration, else 3, 7 or 16 by `quality`.
    pub spectral_samples: u32,
    /// [`AB_ACTIVE`]. Appended in phase 4; every earlier offset is unchanged.
    pub aberration_flags: u32,
    /// The overscan the view's camera applied (the lens row's `overscan`,
    /// sanitised; 1 without a lens): the lens maps the frame into an image that
    /// shows this many times its tangent extent. Appended with the overscan
    /// support; every earlier offset is unchanged.
    pub overscan: f32,
}

const _: () = assert!(std::mem::size_of::<ResolvedLens>() == 88);
const _: () = assert!(std::mem::offset_of!(ResolvedLens, aberration_flags) == 80);
const _: () = assert!(std::mem::offset_of!(ResolvedLens, overscan) == 84);
const _: () = assert!(std::mem::offset_of!(ResolvedLens, aperture_rotation) == 16);
const _: () = assert!(std::mem::offset_of!(ResolvedLens, natural_vignette) == 20);
const _: () = assert!(std::mem::offset_of!(ResolvedLens, enabled) == 24);
const _: () = assert!(std::mem::offset_of!(ResolvedLens, element_count) == 64);
const _: () = assert!(std::mem::offset_of!(ResolvedLens, required_overscan) == 68);
const _: () = assert!(std::mem::offset_of!(ResolvedLens, spectral_samples) == 76);

impl Default for ResolvedLens {
    fn default() -> Self {
        Self {
            f_number: 2.8,
            focal_length_mm: 50.0,
            sensor_width_mm: 36.0,
            aperture_blades: 6,
            aperture_rotation: 0.0,
            natural_vignette: 0.0,
            enabled: 0,
            quality: 0,
            t_stop: 2.8,
            sensor_height_mm: 24.0,
            focus_distance_m: 1.0e6,
            max_coc_px: 32.0,
            field_curvature: 0.0,
            tilt_angle: 0.0,
            tilt_axis: 0.0,
            element_first: 0,
            element_count: 0,
            required_overscan: 1.0,
            map_flags: 0,
            spectral_samples: 1,
            aberration_flags: 0,
            overscan: 1.0,
        }
    }
}
