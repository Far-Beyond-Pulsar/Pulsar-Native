//! A camera's lens, as authored in the editor.
//!
//! `CameraLensComponent` is the reflected face of Helio's physically based
//! lens (`helio_pass_lens`): optics (focal length, f-number, sensor, iris),
//! focus (manual or autofocus), the Scheimpflug tilt and field curvature of
//! the focal surface, and an ordered stack of optical elements (split
//! diopters, a Lensbaby sweet spot) with the image region each acts on.
//! Named presets ([`LENS_PRESETS`]) are data over the same fields.
//!
//! Like `CameraPostProcessComponent`, the authored value derives its GPU
//! rows on every insert, write, removal and replay, matched to a render view
//! by `view_id` (the editor viewport is view 0):
//!
//! - [`CameraLensSourceRow`] (`camera_lens_sources`), at the instance
//!   entity's row: the pass's [`LensComponent`] row, bit for bit. The
//!   environment join gates it (attached, instance enabled, owner current)
//!   and publishes it as `camera_lenses`, which the lens pass reads. The
//!   instance's enabled switch in the inspector therefore turns the lens off.
//! - `lens_elements`: a block of [`LENS_ELEMENT_SLOTS`] [`LensElementComponent`]
//!   rows per lens, from a compact allocator, because the pass addresses a
//!   lens's elements as one contiguous range (`element_first .. +
//!   element_count`) and the entity index of a component instance is sparse.
//!   The block is freed, and its rows zeroed, when the component leaves its
//!   object. A lens that is off has no elements to read.
//!
//! The lens is owned by the camera: no post-process volume blends it, and it
//! is not gated by the owner's visibility.
//!
//! A lens whose ray map looks past the frame (a fisheye, a barrel distortion)
//! needs the camera to render a wider view. [`camera_lens_view_overscan`] gives
//! the host that factor for its frame camera (`helio::Camera::with_overscan`)
//! and writes the same factor into the lens's `camera_lens_sources` row
//! (`LensComponent::overscan`), so the pass crops the wider image back. The
//! factor depends on the image aspect, which the host supplies per frame.
use std::collections::HashMap;
use std::sync::Mutex;

use engine_class_derive::{engine_class, register_world_component};
use helio_pass_lens::{
    BundledLens, FOCUS_AUTO_POINT, FOCUS_MANUAL, LensComponent, LensElementComponent, LensRegion,
    MAX_LENS_ELEMENTS, ProjectionModel, bundled_rays, element_kind, ray_map::applied_overscan,
    user,
};
use pulsar_reflection::{ReflectError, ReflectResult, Reflectable, pulsar_type};
use pulsar_scenedb::gpu::{GpuMirrorHandle, SceneGpuStore};
use pulsar_scenedb_derive::SceneStore;
use serde::{Deserialize, Deserializer, Serialize};

pub const CAMERA_LENS_CLASS_NAME: &str = "CameraLensComponent";

/// Most optical elements one authored lens stacks (the pass allows
/// [`MAX_LENS_ELEMENTS`]; this is the size of the block each lens owns in
/// the `lens_elements` buffer).
pub const LENS_ELEMENT_SLOTS: usize = 8;
const _: () = assert!(LENS_ELEMENT_SLOTS as u32 <= MAX_LENS_ELEMENTS);

/// The SceneDB buffers this component writes: the lens source rows and the
/// lens pass's element rows.
/// The authored lens's source rows, which the environment join publishes as
/// the `camera_lenses` the pass reads.
pub const CAMERA_LENS_SOURCES_BUFFER: &str = "camera_lens_sources";
pub const LENS_ELEMENTS_BUFFER: &str = "lens_elements";

/// Registers the lens GPU columns on the host's store; the lens stays off
/// without this.
pub fn register_lens_gpu_columns(store: &mut SceneGpuStore, device: &std::sync::Arc<wgpu::Device>) {
    CameraLensSourceRow::register_gpu_columns_growable(store, 4, device);
    LensElementComponent::register_gpu_columns_growable(
        store,
        4 * LENS_ELEMENT_SLOTS as u32,
        device,
    );
}

/// The packed GPU component a lens buffer holds, for a host that checks its
/// registered buffers' schemas; `None` for any other key.
pub fn lens_buffer_component_id(key: &str) -> Option<pulsar_scenedb::component::ComponentId> {
    match key {
        CAMERA_LENS_SOURCES_BUFFER => Some(CameraLensSourceRow::packed_gpu_component_id()),
        LENS_ELEMENTS_BUFFER => Some(LensElementComponent::packed_gpu_component_id()),
        _ => None,
    }
}

/// An enum that reads from its variant name or its index (the reflection
/// JSON form), as `CurveAlgorithm` does.
macro_rules! lens_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Reflectable)]
        pub enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(untagged)]
                enum Repr {
                    Name(String),
                    Index(u64),
                }
                let parsed = match Repr::deserialize(deserializer)? {
                    Repr::Name(name) => Self::ALL
                        .iter()
                        .copied()
                        .find(|v| format!("{v:?}").eq_ignore_ascii_case(&name)),
                    Repr::Index(index) => Self::ALL.get(index as usize).copied(),
                };
                parsed.ok_or_else(|| {
                    serde::de::Error::custom(concat!("unknown ", stringify!($name)))
                })
            }
        }
    };
}

/// A camera lens's source row: the pass's [`LensComponent`] row, bit for
/// bit (`enabled` 0 when the authored component is disabled). The
/// environment join places it into `camera_lenses`.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "camera_lens_sources")]
pub struct CameraLensSourceRow {
    /// The 96 bytes of the row, in six four-word groups (reflection
    /// registers only these array shapes).
    #[gpu]
    pub words0: [u32; 4],
    #[gpu]
    pub words1: [u32; 4],
    #[gpu]
    pub words2: [u32; 4],
    #[gpu]
    pub words3: [u32; 4],
    #[gpu]
    pub words4: [u32; 4],
    #[gpu]
    pub words5: [u32; 4],
}
const _: () =
    assert!(std::mem::size_of::<CameraLensSourceRow>() == std::mem::size_of::<LensComponent>());

impl CameraLensSourceRow {
    pub fn of(lens: &LensComponent) -> Self {
        bytemuck::pod_read_unaligned(bytemuck::bytes_of(lens))
    }
}

lens_enum! {
    /// How the lens finds its focus distance.
    LensFocusMode {
        /// Stays at the focus distance.
        #[default]
        Manual,
        /// Refocuses on the depth around the autofocus point.
        AutoPoint,
    }
}

lens_enum! {
    /// Gather quality of the defocus.
    LensGatherQuality {
        /// 24 taps, 3 spectral samples.
        #[default]
        Economical,
        /// 56 taps, 7 spectral samples.
        High,
        /// 56 taps and 16 spectral samples for lateral chromatic aberration
        /// and underwater dispersion.
        Ultra,
    }
}

lens_enum! {
    /// What an optical element does.
    LensElementKind {
        /// Brings a distance into focus inside its region: a split diopter
        /// filter whose power puts that distance in focus (`1/s' = 1/s + D`).
        #[default]
        Diopter,
        /// Lensbaby-style sweet spot: normal focus inside the region and
        /// extra defocus outside it.
        SweetSpot,
        /// The pupil function inside the region: iris shape, anamorphic
        /// squeeze, cat's eye, swirl, spherical aberration, onion rings,
        /// apodization, central obstruction (bokeh).
        Pupil,
        /// Longitudinal chromatic aberration inside the region: a focus
        /// shift in dioptres per colour channel.
        LongitudinalCa,
        /// Ray map: how the sensor records the angle of each ray
        /// (equidistant, fisheye, Panini, ...).
        Projection,
        /// Ray map: Brown-Conrady (OpenCV) radial and tangential distortion.
        BrownConrady,
        /// Ray map: division-model distortion.
        Division,
        /// Ray map: anamorphic "mumps" distortion, separate x and y radial
        /// coefficients.
        AnamorphicDistortion,
        /// Ray map: anamorphic squeeze / desqueeze of the field of view.
        AnamorphicSqueeze,
        /// Ray map: lens shift (optical centre offset).
        Shift,
        /// Ray map: keystone / perspective correction.
        Keystone,
        /// Ray map: focus breathing (magnification follows the focus).
        FocusBreathing,
        /// Ray map: weighted displaced / rotated copies of the image
        /// (multi-image prism, kaleidoscope).
        Prism,
        /// Ray map: spectral lateral chromatic aberration.
        LateralCa,
        /// Ray map: underwater flat port.
        FlatPort,
        /// Ray map: underwater dome port.
        DomePort,
        /// Transmission: natural cos^4 falloff.
        NaturalVignette,
        /// Transmission: optical (cat's eye) vignetting by the lens barrel.
        OpticalVignette,
        /// Transmission: mechanical vignetting by a hood or matte box.
        MechanicalVignette,
        /// Transmission: neutral density.
        NdFilter,
        /// Transmission: graduated neutral density.
        GraduatedNd,
        /// Transmission: colour filter.
        ColorFilter,
        /// Transmission: light lost between the f-number and the T-stop.
        TStopLoss,
        /// Sensor image: spherical-aberration glow.
        SphericalGlow,
        /// Sensor image: coma, a tail pointing away from the axis.
        Coma,
        /// Sensor image: astigmatism, an elliptical point spread.
        Astigmatism,
        /// Sensor image: corner softness (MTF falloff).
        CornerSoftness,
        /// Sensor image: generic diffusion filter.
        Diffusion,
        /// Sensor image: Pro-Mist (wide halo, lifted shadows).
        ProMist,
        /// Sensor image: Black Pro-Mist (halo close to highlights).
        BlackProMist,
        /// Sensor image: Glimmerglass (small halo with fine glints).
        Glimmerglass,
        /// Sensor image: star / cross-screen filter.
        StarFilter,
        /// Sensor image: film halation, a red-orange highlight halo.
        Halation,
        /// Sensor image: water drops refracting the image (give it a mask
        /// region).
        WaterDrops,
        /// Sensor image: smear / Vaseline, directional diffusion.
        Smear,
        /// Sensor image: fog / breath, diffusion that follows a mask region.
        Fog,
        /// Sensor image: dirt, local diffusion with a slight loss.
        Dirt,
        /// A baked real lens (one of the bundled prescriptions): inside the
        /// region its ray map, vignetting, bokeh shape, field curvature and
        /// in-focus point spread come from tracing the lens instead of from
        /// the parametric elements.
        Prescription,
        /// A user-written element: runs the hooks of the WGSL snippet the
        /// project compiled into the lens shader. Inert without one.
        User,
    }
}

lens_enum! {
    /// A lens of the bundled prescription bank
    /// ([`helio_pass_lens::BundledLens`]); the order is the bank's layer order.
    LensPrescriptionChoice {
        /// Idealized thin lens, 50 mm f/2: the identity map with natural falloff.
        ThinLens50F2,
        /// Cooke triplet, 50 mm f/5.
        CookeTriplet50F5,
        /// Six-element double Gauss, 50 mm f/2: barrel distortion, lateral
        /// colour, cat's-eye bokeh and coma.
        #[default]
        DoubleGauss50F2,
    }
}

impl LensPrescriptionChoice {
    /// The bundled lens this chooses.
    pub fn bundled(self) -> BundledLens {
        match self {
            Self::ThinLens50F2 => BundledLens::ThinLens50F2,
            Self::CookeTriplet50F5 => BundledLens::CookeTriplet50F5,
            Self::DoubleGauss50F2 => BundledLens::DoubleGauss50F2,
        }
    }

    /// Its layer of the bundled bank, for the element's `params[0]`.
    pub fn layer(self) -> u32 {
        self.bundled().layer()
    }
}

lens_enum! {
    /// How the sensor records the angle theta of each ray.
    LensProjectionModel {
        /// `r = tan(theta)`: the identity.
        #[default]
        Rectilinear,
        /// `r = theta`.
        Equidistant,
        /// `r = 2 sin(theta / 2)`.
        Equisolid,
        /// `r = sin(theta)`.
        Orthographic,
        /// `r = 2 tan(theta / 2)`.
        Stereographic,
        /// Kannala-Brandt polynomial in theta.
        KannalaBrandt,
        /// Panini (distance and vertical compression).
        Panini,
        /// Horizontal equidistant, vertical rectilinear.
        Cylindrical,
    }
}

impl LensProjectionModel {
    fn to_pass(self) -> ProjectionModel {
        match self {
            Self::Rectilinear => ProjectionModel::Rectilinear,
            Self::Equidistant => ProjectionModel::Equidistant,
            Self::Equisolid => ProjectionModel::Equisolid,
            Self::Orthographic => ProjectionModel::Orthographic,
            Self::Stereographic => ProjectionModel::Stereographic,
            Self::KannalaBrandt => ProjectionModel::KannalaBrandt,
            Self::Panini => ProjectionModel::Panini,
            Self::Cylindrical => ProjectionModel::Cylindrical,
        }
    }
}

lens_enum! {
    /// Where on the frame an element acts. Regions live in image-height
    /// units centred on the frame (x right, y down; the frame spans
    /// `-aspect/2 .. aspect/2` by `-0.5 .. 0.5`).
    LensRegionShape {
        /// The whole frame.
        FullFrame,
        /// The side of a line: `dot(p, (cos a, sin a)) > offset`.
        #[default]
        HalfPlane,
        /// An ellipse at the centre with radii and a rotation.
        Ellipse,
        /// A convex polygon of 3 or 4 vertices.
        Polygon,
        /// The ring between an inner and an outer radius.
        RadialBand,
        /// One channel of a layer of the mask array (water-drop footprints,
        /// dirt, fog): covers where the texel is bright.
        Mask,
    }
}

/// One optical element of a lens stack. Only the fields of its kind and
/// region shape are read.
///
/// The element is a flat set of clearly named fields rather than a
/// sub-struct per kind: the inspector shows registered scalar, bool and enum
/// fields (and a `Vec` of this one registered struct), not nested structs.
/// Fields group into categories by kind (Diopter, Pupil, Distortion,
/// Diffusion ...), each documented with its unit, and the `kind` selects which
/// group is read. Kinds that share a quantity share a field (the diffusion
/// family one strength, radius and tail; the tints one colour). Zero in a
/// "(0: default)" field takes the pass's default for the kind.
#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("General", category_color = "#3AA0FF")]
#[category("Region", category_color = "#E07050")]
#[category("Diopter", category_color = "#E0B040")]
#[category("Sweet Spot", category_color = "#E0B040")]
#[category("Pupil", category_color = "#50C878")]
#[category("Aperture Texture", category_color = "#50C878")]
#[category("Longitudinal CA", category_color = "#C080E0")]
#[category("Projection", category_color = "#3AA0FF")]
#[category("Distortion", category_color = "#E0B040")]
#[category("Image Geometry", category_color = "#50C878")]
#[category("Prism", category_color = "#C080E0")]
#[category("Lateral CA", category_color = "#E07050")]
#[category("Underwater Port", category_color = "#3AA0FF")]
#[category("Vignetting", category_color = "#E0B040")]
#[category("Filters", category_color = "#50C878")]
#[category("Aberrations", category_color = "#C080E0")]
#[category("Diffusion", category_color = "#E07050")]
#[category("Star Filter", category_color = "#3AA0FF")]
#[category("Halation", category_color = "#E0B040")]
#[category("Contaminants", category_color = "#50C878")]
#[category("Mask Region", category_color = "#C080E0")]
#[category("Prescription", category_color = "#3AA0FF")]
#[category("User Element", category_color = "#E07050")]
#[serde(default)]
pub struct LensElementProps {
    /// A disabled element stays in the stack but does nothing.
    #[property(category = "General")]
    pub enabled: bool,
    /// What the element does; only the fields of its kind group are read.
    #[property(category = "General")]
    pub kind: LensElementKind,
    /// The region's soft edge is derived from the aperture (a diopter
    /// seam narrows as the lens stops down) unless this is off.
    #[property(category = "Region")]
    pub auto_feather: bool,
    /// Soft edge width in image heights when `auto_feather` is off.
    #[property(min = 0.0, max = 1.0, step = 0.002, category = "Region")]
    pub feather: f32,
    /// Where on the frame the element acts.
    #[property(category = "Region")]
    pub shape: LensRegionShape,
    /// Half-plane: direction of the covered side in degrees (0: the right
    /// half; 90: the bottom half). Ellipse: its rotation.
    #[property(min = -180.0, max = 180.0, step = 0.5, category = "Region")]
    pub angle_deg: f32,
    /// Half-plane: line offset from the centre in image heights.
    #[property(min = -1.0, max = 1.0, step = 0.005, category = "Region")]
    pub offset: f32,
    /// Ellipse and radial band: centre x in image heights.
    #[property(min = -1.0, max = 1.0, step = 0.005, category = "Region")]
    pub center_x: f32,
    /// Ellipse and radial band: centre y in image heights.
    #[property(min = -1.0, max = 1.0, step = 0.005, category = "Region")]
    pub center_y: f32,
    /// Ellipse: x radius in image heights.
    #[property(min = 0.0, max = 2.0, step = 0.005, category = "Region")]
    pub radius_x: f32,
    /// Ellipse: y radius in image heights.
    #[property(min = 0.0, max = 2.0, step = 0.005, category = "Region")]
    pub radius_y: f32,
    /// Radial band: inner radius in image heights.
    #[property(min = 0.0, max = 2.0, step = 0.005, category = "Region")]
    pub inner_radius: f32,
    /// Radial band: outer radius in image heights.
    #[property(min = 0.0, max = 2.0, step = 0.005, category = "Region")]
    pub outer_radius: f32,
    /// Polygon: 3 or 4 vertices (the pass takes up to 8).
    #[property(min = 3.0, max = 4.0, step = 1.0, category = "Region")]
    pub polygon_vertices: u32,
    /// Polygon: vertex 0 x in image heights.
    #[property(min = -2.0, max = 2.0, step = 0.005, category = "Region")]
    pub p0_x: f32,
    /// Polygon: vertex 0 y in image heights (y down).
    #[property(min = -2.0, max = 2.0, step = 0.005, category = "Region")]
    pub p0_y: f32,
    /// Polygon: vertex 1 x in image heights.
    #[property(min = -2.0, max = 2.0, step = 0.005, category = "Region")]
    pub p1_x: f32,
    /// Polygon: vertex 1 y in image heights (y down).
    #[property(min = -2.0, max = 2.0, step = 0.005, category = "Region")]
    pub p1_y: f32,
    /// Polygon: vertex 2 x in image heights.
    #[property(min = -2.0, max = 2.0, step = 0.005, category = "Region")]
    pub p2_x: f32,
    /// Polygon: vertex 2 y in image heights (y down).
    #[property(min = -2.0, max = 2.0, step = 0.005, category = "Region")]
    pub p2_y: f32,
    /// Polygon: vertex 3 x in image heights.
    #[property(min = -2.0, max = 2.0, step = 0.005, category = "Region")]
    pub p3_x: f32,
    /// Polygon: vertex 3 y in image heights (y down).
    #[property(min = -2.0, max = 2.0, step = 0.005, category = "Region")]
    pub p3_y: f32,
    /// Mask region: layer of the mask array.
    #[property(min = 0.0, max = 63.0, step = 1.0, category = "Mask Region")]
    pub mask_layer: u32,
    /// Mask region: channel of the layer (0 red, 1 green, 2 blue, 3 alpha).
    #[property(min = 0.0, max = 3.0, step = 1.0, category = "Mask Region")]
    pub mask_channel: u32,
    /// Mask region: cover where the texel is dark instead of bright.
    #[property(category = "Mask Region")]
    pub mask_invert: bool,
    /// Mask region: magnification of the mask about the frame centre (1: the frame).
    #[property(min = 0.05, max = 16.0, step = 0.01, category = "Mask Region")]
    pub mask_scale: f32,
    /// Mask region: horizontal shift of the mask in frame widths (unit-square uv).
    #[property(min = -1.0, max = 1.0, step = 0.005, category = "Mask Region")]
    pub mask_offset_u: f32,
    /// Mask region: vertical shift of the mask in frame heights (unit-square uv).
    #[property(min = -1.0, max = 1.0, step = 0.005, category = "Mask Region")]
    pub mask_offset_v: f32,
    /// Mask region: repeat the mask outside the frame-sized tile; off covers nothing there.
    #[property(category = "Mask Region")]
    pub mask_tile: bool,
    /// Diopter: distance in metres the region brings into focus; each pixel
    /// there still blurs by its own depth. The filter's power follows the
    /// lens's focus (`D = 1/focus - 1/s`), so focus pulls keep it sharp here.
    #[property(
        min = 0.05,
        max = 1000.0,
        step = 0.05,
        label = "Sharp at (m)",
        category = "Diopter"
    )]
    pub focus_m: f32,
    /// Diopter: how dark the seam is at its centre (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Diopter")]
    pub seam_darkening: f32,
    /// Diopter: distance of the filter in front of the lens in metres; it
    /// sets how soft the seam is (0: 0.08 m).
    #[property(min = 0.0, max = 1.0, step = 0.005, category = "Diopter")]
    pub mount_distance_m: f32,
    /// Sweet spot: extra defocus outside the region, in pixels.
    #[property(min = 0.0, max = 64.0, step = 0.5, category = "Sweet Spot")]
    pub extra_blur_px: f32,
    /// Pupil: use the lens's own iris (blades and rotation) for the shape.
    #[property(category = "Pupil")]
    pub inherit_iris: bool,
    /// Pupil: iris blades (0: circle, 3..16).
    #[property(min = 0.0, max = 16.0, step = 1.0, category = "Pupil")]
    pub blades: u32,
    /// Pupil: how curved the blade edges are (0 straight, 1 a circle).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Pupil")]
    pub blade_curvature: f32,
    /// Pupil: iris rotation in degrees.
    #[property(min = -180.0, max = 180.0, step = 1.0, category = "Pupil")]
    pub iris_rotation_deg: f32,
    /// Pupil: f-number at and above which the blades are as curved as the
    /// curvature says; wide open the iris is round (0: off).
    #[property(min = 0.0, max = 32.0, step = 0.1, category = "Pupil")]
    pub round_until_f_number: f32,
    /// Anamorphic squeeze ratio (1.33, 1.5, 1.8, 2.0). Pupil: bokeh this many
    /// times taller than wide. Anamorphic squeeze element: this many times
    /// more field of view horizontally.
    #[property(min = 1.0, max = 3.0, step = 0.01, category = "Pupil")]
    pub squeeze: f32,
    /// Cat's eye strength (optical vignetting toward the corners). Pupil
    /// and optical vignette elements; give both the same numbers.
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Pupil")]
    pub cats_eye: f32,
    /// Radius of the barrel clipping the cat's eye, relative to the iris
    /// (0: 1). Pupil and optical vignette elements.
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Pupil")]
    pub barrel_radius: f32,
    /// Pupil: swirl, a tangential stretch growing with the field radius.
    #[property(min = -2.0, max = 2.0, step = 0.01, category = "Pupil")]
    pub swirl: f32,
    /// Pupil: spherical aberration (positive: bright rim; negative: soft
    /// centre-weighted).
    #[property(min = -1.0, max = 1.0, step = 0.01, category = "Pupil")]
    pub spherical: f32,
    /// Pupil: how far the spherical aberration flips in front of the focus (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Pupil")]
    pub spherical_asymmetry: f32,
    /// Pupil: onion-ring ripple amplitude.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Pupil")]
    pub onion_amplitude: f32,
    /// Pupil: onion-ring ripples across the radius (0: 6).
    #[property(min = 0.0, max = 32.0, step = 0.5, category = "Pupil")]
    pub onion_rings: f32,
    /// Pupil: Gaussian apodization, in pupil radii (0: off).
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Pupil")]
    pub apodization: f32,
    /// Pupil: central obstruction in pupil radii (mirror-lens donut).
    #[property(min = 0.0, max = 0.95, step = 0.01, category = "Pupil")]
    pub obstruction: f32,
    /// Pupil: layer of the aperture texture array used as the iris shape.
    #[property(min = 0.0, max = 63.0, step = 1.0, category = "Aperture Texture")]
    pub aperture_texture_layer: u32,
    /// Pupil: how far the aperture texture replaces the polygon (0 ignores it).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Aperture Texture")]
    pub aperture_texture_amount: f32,
    /// Longitudinal CA: red focus shift in dioptres (negative red and blue:
    /// purple foreground fringes, green background ones).
    #[property(min = -2.0, max = 2.0, step = 0.005, category = "Longitudinal CA")]
    pub red_shift_dpt: f32,
    /// Longitudinal CA: green focus shift in dioptres.
    #[property(min = -2.0, max = 2.0, step = 0.005, category = "Longitudinal CA")]
    pub green_shift_dpt: f32,
    /// Longitudinal CA: blue focus shift in dioptres.
    #[property(min = -2.0, max = 2.0, step = 0.005, category = "Longitudinal CA")]
    pub blue_shift_dpt: f32,
    /// Projection: how the sensor records the angle of each ray.
    #[property(category = "Projection")]
    pub projection_model: LensProjectionModel,
    /// Projection (Kannala-Brandt): coefficient k1 in `r = theta + k1 theta^3 + k2 theta^5 + k3 theta^7 + k4 theta^9` (theta in radians).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Projection")]
    pub kb_k1: f32,
    /// Projection (Kannala-Brandt): coefficient k2 in `r = theta + k1 theta^3 + k2 theta^5 + k3 theta^7 + k4 theta^9` (theta in radians).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Projection")]
    pub kb_k2: f32,
    /// Projection (Kannala-Brandt): coefficient k3 in `r = theta + k1 theta^3 + k2 theta^5 + k3 theta^7 + k4 theta^9` (theta in radians).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Projection")]
    pub kb_k3: f32,
    /// Projection (Kannala-Brandt): coefficient k4 in `r = theta + k1 theta^3 + k2 theta^5 + k3 theta^7 + k4 theta^9` (theta in radians).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Projection")]
    pub kb_k4: f32,
    /// Projection (Panini): distance d (0: rectilinear, 1: classic Panini).
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Projection")]
    pub panini_distance: f32,
    /// Projection (Panini): vertical compression s (0 none, 1 classic).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Projection")]
    pub panini_compression: f32,
    /// Brown-Conrady: radial numerator coefficient k1 (unitless; negative k1 barrel, positive pincushion).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Distortion")]
    pub bc_k1: f32,
    /// Brown-Conrady: radial numerator coefficient k2 (unitless; negative k1 barrel, positive pincushion).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Distortion")]
    pub bc_k2: f32,
    /// Brown-Conrady: radial numerator coefficient k3 (unitless; negative k1 barrel, positive pincushion).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Distortion")]
    pub bc_k3: f32,
    /// Brown-Conrady: radial denominator coefficient k4 (unitless).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Distortion")]
    pub bc_k4: f32,
    /// Brown-Conrady: radial denominator coefficient k5 (unitless).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Distortion")]
    pub bc_k5: f32,
    /// Brown-Conrady: radial denominator coefficient k6 (unitless).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Distortion")]
    pub bc_k6: f32,
    /// Brown-Conrady: tangential coefficient p1 (unitless).
    #[property(min = -0.5, max = 0.5, step = 0.001, category = "Distortion")]
    pub bc_p1: f32,
    /// Brown-Conrady: tangential coefficient p2 (unitless).
    #[property(min = -0.5, max = 0.5, step = 0.001, category = "Distortion")]
    pub bc_p2: f32,
    /// Division model: coefficient l1 on r^2 (unitless; negative is barrel).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Distortion")]
    pub division_l1: f32,
    /// Division model: coefficient l2 on r^4 (unitless).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Distortion")]
    pub division_l2: f32,
    /// Anamorphic distortion: x coefficient on r^2 (unitless; barrel in one
    /// axis and pincushion in the other is the anamorphic mumps).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Distortion")]
    pub mumps_kx1: f32,
    /// Anamorphic distortion: x coefficient on r^4 (unitless).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Distortion")]
    pub mumps_kx2: f32,
    /// Anamorphic distortion: y coefficient on r^2 (unitless).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Distortion")]
    pub mumps_ky1: f32,
    /// Anamorphic distortion: y coefficient on r^4 (unitless).
    #[property(min = -2.0, max = 2.0, step = 0.001, category = "Distortion")]
    pub mumps_ky2: f32,
    /// Shift: optical centre offset x in image heights.
    #[property(min = -1.0, max = 1.0, step = 0.005, category = "Image Geometry")]
    pub shift_x: f32,
    /// Shift: optical centre offset y in image heights (y down).
    #[property(min = -1.0, max = 1.0, step = 0.005, category = "Image Geometry")]
    pub shift_y: f32,
    /// Keystone: horizontal perspective coefficient (about tan of the tilt).
    #[property(min = -1.0, max = 1.0, step = 0.005, category = "Image Geometry")]
    pub keystone_x: f32,
    /// Keystone: vertical perspective coefficient (about tan of the tilt).
    #[property(min = -1.0, max = 1.0, step = 0.005, category = "Image Geometry")]
    pub keystone_y: f32,
    /// Focus breathing: magnification change per dioptre of focus (negative:
    /// focusing closer narrows the view).
    #[property(min = -1.0, max = 1.0, step = 0.005, category = "Image Geometry")]
    pub breathing_per_dpt: f32,
    /// Focus breathing: focus in dioptres where the magnification is 1 (0: infinity).
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Image Geometry")]
    pub breathing_reference_dpt: f32,
    /// Prism: number of image copies (2..8).
    #[property(min = 2.0, max = 8.0, step = 1.0, category = "Prism")]
    pub prism_count: u32,
    /// Prism: copy spacing along the line, or ring radius, in image heights.
    #[property(min = 0.0, max = 1.0, step = 0.001, category = "Prism")]
    pub prism_offset: f32,
    /// Prism: direction of the line, or angle of the first ring copy, in degrees.
    #[property(min = -180.0, max = 180.0, step = 1.0, category = "Prism")]
    pub prism_angle_deg: f32,
    /// Prism: place the copies around a ring instead of along a line.
    #[property(category = "Prism")]
    pub prism_ring: bool,
    /// Prism: rotation of each copy over the one before, in degrees (kaleidoscope).
    #[property(min = -180.0, max = 180.0, step = 0.5, category = "Prism")]
    pub prism_spin_deg: f32,
    /// Prism: weight ratio between successive copies (0: all equal).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Prism")]
    pub prism_falloff: f32,
    /// Lateral CA: linear magnification change across the spectrum (unitless;
    /// positive puts red outward and blue inward).
    #[property(min = -0.1, max = 0.1, step = 0.0005, category = "Lateral CA")]
    pub lateral_ca_d1: f32,
    /// Lateral CA: quadratic magnification change across the spectrum (unitless).
    #[property(min = -0.1, max = 0.1, step = 0.0005, category = "Lateral CA")]
    pub lateral_ca_d2: f32,
    /// Flat and dome port: refractive index of water (0: 1.333).
    #[property(min = 0.0, max = 2.0, step = 0.001, category = "Underwater Port")]
    pub port_index: f32,
    /// Flat and dome port: index change across the spectrum (blue higher;
    /// water is about 0.006).
    #[property(min = -0.05, max = 0.05, step = 0.0005, category = "Underwater Port")]
    pub port_dispersion: f32,
    /// Dome port: dome radius in metres.
    #[property(min = 0.01, max = 1.0, step = 0.005, category = "Underwater Port")]
    pub dome_radius_m: f32,
    /// Dome port: entrance pupil offset from the dome centre in metres
    /// (positive toward the glass).
    #[property(min = -0.5, max = 0.5, step = 0.001, category = "Underwater Port")]
    pub dome_pupil_offset_m: f32,
    /// Dome port: scale of the virtual image's field curvature (0 off, 1 physical).
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Underwater Port")]
    pub dome_curvature: f32,
    /// Natural vignette: amount of cos^4 falloff (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Vignetting")]
    pub natural_vignette_amount: f32,
    /// Optical vignette: f-number at which the barrel radius holds (0: the
    /// current one).
    #[property(min = 0.0, max = 32.0, step = 0.1, category = "Vignetting")]
    pub vignette_reference_f_number: f32,
    /// Mechanical vignette: a rectangular matte box instead of a circular hood.
    #[property(category = "Vignetting")]
    pub hood_rectangular: bool,
    /// Mechanical vignette: hood radius or matte-box half width in image heights.
    #[property(min = 0.0, max = 3.0, step = 0.005, category = "Vignetting")]
    pub hood_size_x: f32,
    /// Mechanical vignette: matte-box half height in image heights.
    #[property(min = 0.0, max = 3.0, step = 0.005, category = "Vignetting")]
    pub hood_size_y: f32,
    /// Mechanical vignette: soft edge width in image heights.
    #[property(min = 0.0, max = 1.0, step = 0.005, category = "Vignetting")]
    pub hood_feather: f32,
    /// Mechanical vignette: matte-box rotation in degrees.
    #[property(min = -180.0, max = 180.0, step = 1.0, category = "Vignetting")]
    pub hood_rotation_deg: f32,
    /// Neutral density: stops of light removed.
    #[property(min = 0.0, max = 16.0, step = 0.1, category = "Filters")]
    pub nd_stops: f32,
    /// Graduated ND: stops of density on the dark side.
    #[property(min = 0.0, max = 16.0, step = 0.1, category = "Filters")]
    pub grad_nd_stops: f32,
    /// Graduated ND: direction pointing to the dark side in degrees (-90: top
    /// dark, 90: bottom dark).
    #[property(min = -180.0, max = 180.0, step = 1.0, category = "Filters")]
    pub grad_nd_angle_deg: f32,
    /// Graduated ND: position of the transition line in image heights.
    #[property(min = -1.0, max = 1.0, step = 0.005, category = "Filters")]
    pub grad_nd_offset: f32,
    /// Graduated ND: transition width in image heights (hard 0.01, soft 0.5).
    #[property(min = 0.0, max = 1.0, step = 0.005, category = "Filters")]
    pub grad_nd_softness: f32,
    /// Colour filter: linear red transmittance. Graduated ND: red transmittance
    /// at full density (1: neutral).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Filters")]
    pub tint_r: f32,
    /// Colour filter: linear green transmittance. Graduated ND: green tint.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Filters")]
    pub tint_g: f32,
    /// Colour filter: linear blue transmittance. Graduated ND: blue tint.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Filters")]
    pub tint_b: f32,
    /// Colour filter: how far the transmittance applies (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Filters")]
    pub color_filter_amount: f32,
    /// T-stop loss: amount of the (f-number / T-stop)^2 light loss (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Filters")]
    pub t_stop_loss_amount: f32,
    /// Spherical glow: share of the light scattered into the halo at the
    /// reference f-number (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Aberrations")]
    pub glow_amount: f32,
    /// Spherical glow: f-number at which the amount holds (0: 2.8).
    #[property(min = 0.0, max = 32.0, step = 0.1, category = "Aberrations")]
    pub glow_reference_f_number: f32,
    /// Spherical glow: falloff exponent k of 1/N^k when stopping down (0: 2).
    #[property(min = 0.0, max = 8.0, step = 0.1, category = "Aberrations")]
    pub glow_exponent: f32,
    /// Spherical glow: radius of the first lobe in image heights (0: 0.012).
    #[property(min = 0.0, max = 0.5, step = 0.001, category = "Aberrations")]
    pub glow_radius: f32,
    /// Spherical glow: weight of the wider lobes (0: 0.55).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Aberrations")]
    pub glow_tail: f32,
    /// Coma: share of the light in the tail at the frame corner (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Aberrations")]
    pub coma_amount: f32,
    /// Coma: tail length at the corner in image heights.
    #[property(min = 0.0, max = 0.5, step = 0.001, category = "Aberrations")]
    pub coma_length: f32,
    /// Coma: power of the field radius in the share (0: 1).
    #[property(min = 0.0, max = 8.0, step = 0.1, category = "Aberrations")]
    pub coma_exponent: f32,
    /// Coma: taper power of the tail's density (0: 2).
    #[property(min = 0.0, max = 8.0, step = 0.1, category = "Aberrations")]
    pub coma_shape: f32,
    /// Astigmatism: long-axis blur at the corner in image heights (positive
    /// elongates points along the radius, negative across it).
    #[property(min = -0.2, max = 0.2, step = 0.001, category = "Aberrations")]
    pub astigmatism_amount: f32,
    /// Astigmatism: power of the field radius (0: 2).
    #[property(min = 0.0, max = 8.0, step = 0.1, category = "Aberrations")]
    pub astigmatism_exponent: f32,
    /// Astigmatism: short axis over long axis (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Aberrations")]
    pub astigmatism_minor_ratio: f32,
    /// Corner softness: blur standard deviation at the corner in image heights.
    #[property(min = 0.0, max = 0.2, step = 0.0005, category = "Aberrations")]
    pub corner_softness_sigma: f32,
    /// Corner softness: power of the field radius (0: 2).
    #[property(min = 0.0, max = 8.0, step = 0.1, category = "Aberrations")]
    pub corner_softness_exponent: f32,
    /// Corner softness: field radius where the blur begins (0..1, 1 is the corner).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Aberrations")]
    pub corner_softness_start: f32,
    /// Diffusion, Pro-Mist, Black Pro-Mist, Glimmerglass, Fog: share of the
    /// light scattered into the halo (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Diffusion")]
    pub diffusion_strength: f32,
    /// Diffusion family: first lobe's standard deviation in image heights
    /// (0: the kind's default).
    #[property(min = 0.0, max = 0.5, step = 0.001, category = "Diffusion")]
    pub diffusion_radius: f32,
    /// Diffusion family: weight ratio of successive lobes (small stays near
    /// the highlight, large reaches far; 0: the kind's default).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Diffusion")]
    pub diffusion_tail: f32,
    /// Glimmerglass: contrast of the glints (0: 0.8).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Diffusion")]
    pub glint_contrast: f32,
    /// Star filter: number of spikes (2..16).
    #[property(min = 2.0, max = 16.0, step = 1.0, category = "Star Filter")]
    pub star_points: u32,
    /// Star filter: rotation of the first spike in degrees (y down).
    #[property(min = -180.0, max = 180.0, step = 1.0, category = "Star Filter")]
    pub star_rotation_deg: f32,
    /// Star filter: spike length in image heights.
    #[property(min = 0.0, max = 1.0, step = 0.005, category = "Star Filter")]
    pub star_length: f32,
    /// Star filter: share of the light in the spikes (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Star Filter")]
    pub star_intensity: f32,
    /// Star filter: fade exponent along a spike.
    #[property(min = 0.0, max = 8.0, step = 0.1, category = "Star Filter")]
    pub star_falloff: f32,
    /// Halation: share of the light scattered into the halo (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Halation")]
    pub halation_strength: f32,
    /// Halation: first lobe's standard deviation in image heights (0: 0.02).
    #[property(min = 0.0, max = 0.5, step = 0.001, category = "Halation")]
    pub halation_radius: f32,
    /// Halation: red scatter weight (all three 0: 1, 0.35, 0.08).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Halation")]
    pub halation_tint_r: f32,
    /// Halation: green scatter weight.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Halation")]
    pub halation_tint_g: f32,
    /// Halation: blue scatter weight.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Halation")]
    pub halation_tint_b: f32,
    /// Halation: weight of the second, wider lobe (0: 0.5).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Halation")]
    pub halation_tail: f32,
    /// Water drops: displacement at a unit normal in image heights.
    #[property(min = 0.0, max = 0.5, step = 0.001, category = "Contaminants")]
    pub drops_refraction: f32,
    /// Water drops: layer of the mask array holding the drop normals.
    #[property(min = 0.0, max = 63.0, step = 1.0, category = "Contaminants")]
    pub drops_normal_layer: u32,
    /// Water drops: standard deviation of the local defocus in image heights.
    #[property(min = 0.0, max = 0.2, step = 0.0005, category = "Contaminants")]
    pub drops_defocus: f32,
    /// Smear: share of the light spread along the direction (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Contaminants")]
    pub smear_strength: f32,
    /// Smear: standard deviation along the direction in image heights (0: 0.03).
    #[property(min = 0.0, max = 0.5, step = 0.001, category = "Contaminants")]
    pub smear_length: f32,
    /// Smear: direction in degrees (y down).
    #[property(min = -180.0, max = 180.0, step = 1.0, category = "Contaminants")]
    pub smear_angle_deg: f32,
    /// Dirt: share of the light scattered where the region covers (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Contaminants")]
    pub dirt_strength: f32,
    /// Dirt: first lobe's standard deviation in image heights (0: 0.02).
    #[property(min = 0.0, max = 0.5, step = 0.001, category = "Contaminants")]
    pub dirt_radius: f32,
    /// Dirt: light absorbed under full coverage (0..1).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Contaminants")]
    pub dirt_loss: f32,
    /// Dirt: weight ratio of successive lobes (0: 0.5).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Contaminants")]
    pub dirt_tail: f32,
    /// Prescription: the baked lens that drives the element's region.
    #[property(category = "Prescription")]
    pub prescription_lens: LensPrescriptionChoice,
    /// Prescription: scale of the baked in-focus point spread (core blur and
    /// coma tail): 1 is the bake, 0 leaves it out.
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Prescription")]
    pub psf_amount: f32,
    /// User element: runs the snippet's `user_ray_map`.
    #[property(category = "User Element")]
    pub user_map: bool,
    /// User element: runs the snippet's `user_transmission`.
    #[property(category = "User Element")]
    pub user_transmission: bool,
    /// User element: runs the snippet's `user_focus`.
    #[property(category = "User Element")]
    pub user_focus: bool,
    /// User element: `user_ray_map` depends on the wavelength, so the
    /// composite samples the spectrum.
    #[property(category = "User Element")]
    pub user_spectral: bool,
    /// User element: parameter 0, handed to the hooks as `params[0]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p0: f32,
    /// User element: parameter 1, handed to the hooks as `params[1]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p1: f32,
    /// User element: parameter 2, handed to the hooks as `params[2]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p2: f32,
    /// User element: parameter 3, handed to the hooks as `params[3]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p3: f32,
    /// User element: parameter 4, handed to the hooks as `params[4]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p4: f32,
    /// User element: parameter 5, handed to the hooks as `params[5]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p5: f32,
    /// User element: parameter 6, handed to the hooks as `params[6]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p6: f32,
    /// User element: parameter 7, handed to the hooks as `params[7]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p7: f32,
    /// User element: parameter 8, handed to the hooks as `params[8]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p8: f32,
    /// User element: parameter 9, handed to the hooks as `params[9]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p9: f32,
    /// User element: parameter 10, handed to the hooks as `params[10]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p10: f32,
    /// User element: parameter 11, handed to the hooks as `params[11]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p11: f32,
    /// User element: parameter 12, handed to the hooks as `params[12]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p12: f32,
    /// User element: parameter 13, handed to the hooks as `params[13]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p13: f32,
    /// User element: parameter 14, handed to the hooks as `params[14]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p14: f32,
    /// User element: parameter 15, handed to the hooks as `params[15]`.
    #[property(min = -1000.0, max = 1000.0, step = 0.01, category = "User Element")]
    pub user_p15: f32,
}

impl Default for LensElementProps {
    fn default() -> Self {
        Self {
            enabled: true,
            kind: LensElementKind::Diopter,
            auto_feather: true,
            feather: 0.02,
            shape: LensRegionShape::HalfPlane,
            angle_deg: 0.0,
            offset: 0.0,
            center_x: 0.0,
            center_y: 0.0,
            radius_x: 0.25,
            radius_y: 0.25,
            inner_radius: 0.2,
            outer_radius: 0.5,
            polygon_vertices: 3,
            p0_x: -0.3,
            p0_y: -0.3,
            p1_x: 0.3,
            p1_y: -0.3,
            p2_x: 0.0,
            p2_y: 0.3,
            p3_x: 0.0,
            p3_y: 0.0,
            mask_layer: 0,
            mask_channel: 3,
            mask_invert: false,
            mask_scale: 1.0,
            mask_offset_u: 0.0,
            mask_offset_v: 0.0,
            mask_tile: false,
            focus_m: 1.0,
            seam_darkening: 0.0,
            mount_distance_m: 0.0,
            extra_blur_px: 12.0,
            inherit_iris: true,
            blades: 0,
            blade_curvature: 0.0,
            iris_rotation_deg: 0.0,
            round_until_f_number: 0.0,
            squeeze: 1.0,
            cats_eye: 0.0,
            barrel_radius: 0.0,
            swirl: 0.0,
            spherical: 0.0,
            spherical_asymmetry: 0.0,
            onion_amplitude: 0.0,
            onion_rings: 0.0,
            apodization: 0.0,
            obstruction: 0.0,
            aperture_texture_layer: 0,
            aperture_texture_amount: 0.0,
            red_shift_dpt: 0.0,
            green_shift_dpt: 0.0,
            blue_shift_dpt: 0.0,
            projection_model: LensProjectionModel::Rectilinear,
            kb_k1: 0.0,
            kb_k2: 0.0,
            kb_k3: 0.0,
            kb_k4: 0.0,
            panini_distance: 1.0,
            panini_compression: 1.0,
            bc_k1: 0.0,
            bc_k2: 0.0,
            bc_k3: 0.0,
            bc_k4: 0.0,
            bc_k5: 0.0,
            bc_k6: 0.0,
            bc_p1: 0.0,
            bc_p2: 0.0,
            division_l1: 0.0,
            division_l2: 0.0,
            mumps_kx1: 0.0,
            mumps_kx2: 0.0,
            mumps_ky1: 0.0,
            mumps_ky2: 0.0,
            shift_x: 0.0,
            shift_y: 0.0,
            keystone_x: 0.0,
            keystone_y: 0.0,
            breathing_per_dpt: 0.0,
            breathing_reference_dpt: 0.0,
            prism_count: 3,
            prism_offset: 0.02,
            prism_angle_deg: 0.0,
            prism_ring: false,
            prism_spin_deg: 0.0,
            prism_falloff: 0.0,
            lateral_ca_d1: 0.0,
            lateral_ca_d2: 0.0,
            port_index: 1.333,
            port_dispersion: 0.006,
            dome_radius_m: 0.1,
            dome_pupil_offset_m: 0.0,
            dome_curvature: 1.0,
            natural_vignette_amount: 1.0,
            vignette_reference_f_number: 0.0,
            hood_rectangular: false,
            hood_size_x: 0.6,
            hood_size_y: 0.4,
            hood_feather: 0.1,
            hood_rotation_deg: 0.0,
            nd_stops: 1.0,
            grad_nd_stops: 2.0,
            grad_nd_angle_deg: -90.0,
            grad_nd_offset: 0.0,
            grad_nd_softness: 0.25,
            tint_r: 1.0,
            tint_g: 1.0,
            tint_b: 1.0,
            color_filter_amount: 1.0,
            t_stop_loss_amount: 1.0,
            glow_amount: 0.3,
            glow_reference_f_number: 0.0,
            glow_exponent: 0.0,
            glow_radius: 0.0,
            glow_tail: 0.0,
            coma_amount: 0.3,
            coma_length: 0.05,
            coma_exponent: 0.0,
            coma_shape: 0.0,
            astigmatism_amount: 0.01,
            astigmatism_exponent: 0.0,
            astigmatism_minor_ratio: 0.0,
            corner_softness_sigma: 0.01,
            corner_softness_exponent: 0.0,
            corner_softness_start: 0.3,
            diffusion_strength: 0.25,
            diffusion_radius: 0.0,
            diffusion_tail: 0.0,
            glint_contrast: 0.0,
            star_points: 4,
            star_rotation_deg: 0.0,
            star_length: 0.2,
            star_intensity: 0.2,
            star_falloff: 1.0,
            halation_strength: 0.4,
            halation_radius: 0.0,
            halation_tint_r: 0.0,
            halation_tint_g: 0.0,
            halation_tint_b: 0.0,
            halation_tail: 0.0,
            drops_refraction: 0.02,
            drops_normal_layer: 0,
            drops_defocus: 0.005,
            smear_strength: 0.3,
            smear_length: 0.0,
            smear_angle_deg: 0.0,
            dirt_strength: 0.3,
            dirt_radius: 0.0,
            dirt_loss: 0.1,
            dirt_tail: 0.0,
            prescription_lens: LensPrescriptionChoice::DoubleGauss50F2,
            psf_amount: 1.0,
            user_map: false,
            user_transmission: false,
            user_focus: false,
            user_spectral: false,
            user_p0: 0.0,
            user_p1: 0.0,
            user_p2: 0.0,
            user_p3: 0.0,
            user_p4: 0.0,
            user_p5: 0.0,
            user_p6: 0.0,
            user_p7: 0.0,
            user_p8: 0.0,
            user_p9: 0.0,
            user_p10: 0.0,
            user_p11: 0.0,
            user_p12: 0.0,
            user_p13: 0.0,
            user_p14: 0.0,
            user_p15: 0.0,
        }
    }
}

impl LensElementProps {
    /// The element a preset describes.
    pub fn from_preset(element: &PresetElement) -> Self {
        let mut props = Self {
            kind: element.kind,
            focus_m: if element.kind == LensElementKind::Diopter {
                element.amount
            } else {
                0.0
            },
            extra_blur_px: if element.kind == LensElementKind::SweetSpot {
                element.amount
            } else {
                0.0
            },
            seam_darkening: element.seam_darkening,
            auto_feather: element.feather.is_none(),
            feather: element.feather.unwrap_or(0.02),
            ..Self::default()
        };
        match element.region {
            PresetRegion::FullFrame => props.shape = LensRegionShape::FullFrame,
            PresetRegion::HalfPlane { angle_deg, offset } => {
                props.shape = LensRegionShape::HalfPlane;
                props.angle_deg = angle_deg;
                props.offset = offset;
            }
            PresetRegion::Ellipse {
                center,
                radii,
                rotation_deg,
            } => {
                props.shape = LensRegionShape::Ellipse;
                (props.center_x, props.center_y) = (center[0], center[1]);
                (props.radius_x, props.radius_y) = (radii[0], radii[1]);
                props.angle_deg = rotation_deg;
            }
            PresetRegion::RadialBand {
                inner,
                outer,
                center,
            } => {
                props.shape = LensRegionShape::RadialBand;
                (props.inner_radius, props.outer_radius) = (inner, outer);
                (props.center_x, props.center_y) = (center[0], center[1]);
            }
            PresetRegion::Mask { layer, channel } => {
                props.shape = LensRegionShape::Mask;
                (props.mask_layer, props.mask_channel) = (layer, channel);
            }
        }
        if let Some(tune) = element.tune {
            tune(&mut props);
        }
        props
    }

    /// The 16 parameters of a user element, in order.
    fn user_params(&self) -> [f32; 16] {
        [
            self.user_p0,
            self.user_p1,
            self.user_p2,
            self.user_p3,
            self.user_p4,
            self.user_p5,
            self.user_p6,
            self.user_p7,
            self.user_p8,
            self.user_p9,
            self.user_p10,
            self.user_p11,
            self.user_p12,
            self.user_p13,
            self.user_p14,
            self.user_p15,
        ]
    }

    fn region(&self) -> ([[f32; 2]; 4], usize) {
        (
            [
                [self.p0_x, self.p0_y],
                [self.p1_x, self.p1_y],
                [self.p2_x, self.p2_y],
                [self.p3_x, self.p3_y],
            ],
            self.polygon_vertices.clamp(3, 4) as usize,
        )
    }

    /// The pass row of this element. A disabled element is an inert row.
    pub fn to_row(&self) -> LensElementComponent {
        if !self.enabled {
            return bytemuck::Zeroable::zeroed();
        }
        let (vertices, count) = self.region();
        let region = match self.shape {
            LensRegionShape::FullFrame => LensRegion::FullFrame,
            LensRegionShape::HalfPlane => LensRegion::HalfPlane {
                angle: self.angle_deg.to_radians(),
                offset: self.offset,
            },
            LensRegionShape::Ellipse => LensRegion::Ellipse {
                center: [self.center_x, self.center_y],
                radii: [self.radius_x, self.radius_y],
                rotation: self.angle_deg.to_radians(),
            },
            LensRegionShape::Polygon => LensRegion::Polygon(&vertices[..count]),
            LensRegionShape::RadialBand => LensRegion::RadialBand {
                r0: self.inner_radius,
                r1: self.outer_radius,
                center: [self.center_x, self.center_y],
            },
            LensRegionShape::Mask => LensRegion::MaskChannel {
                layer: self.mask_layer,
                channel: self.mask_channel.min(3),
                invert: self.mask_invert,
            },
        };
        let mut row = match self.kind {
            LensElementKind::Diopter => {
                let mut row = LensElementComponent::diopter(self.focus_m, region)
                    .with_seam_darkening(self.seam_darkening);
                if self.mount_distance_m > 0.0 {
                    row = row.with_mount_distance(self.mount_distance_m);
                }
                row
            }
            LensElementKind::SweetSpot => {
                LensElementComponent::sweet_spot(self.extra_blur_px, region)
            }
            LensElementKind::Pupil => {
                let mut row = LensElementComponent::pupil(region);
                row = row.with_aperture_texture(
                    self.aperture_texture_layer,
                    self.aperture_texture_amount,
                );
                if !self.inherit_iris {
                    row = row.with_iris(
                        self.blades,
                        self.blade_curvature,
                        self.iris_rotation_deg.to_radians(),
                    );
                }
                row.with_f_number_rounding(self.round_until_f_number)
                    .with_anamorphic(self.squeeze.max(1.0))
                    .with_cats_eye(self.cats_eye, self.barrel_radius)
                    .with_swirl(self.swirl)
                    .with_spherical_aberration(self.spherical, self.spherical_asymmetry)
                    .with_onion_rings(self.onion_amplitude, self.onion_rings)
                    .with_apodization(self.apodization)
                    .with_central_obstruction(self.obstruction)
            }
            LensElementKind::LongitudinalCa => LensElementComponent::longitudinal_ca(
                self.red_shift_dpt,
                self.green_shift_dpt,
                self.blue_shift_dpt,
                region,
            ),
            LensElementKind::Projection => {
                let mut row =
                    LensElementComponent::projection(self.projection_model.to_pass(), region);
                match self.projection_model {
                    LensProjectionModel::KannalaBrandt => {
                        row = row
                            .with_kannala_brandt([self.kb_k1, self.kb_k2, self.kb_k3, self.kb_k4]);
                    }
                    LensProjectionModel::Panini => {
                        row = row.with_panini(self.panini_distance, self.panini_compression);
                    }
                    _ => {}
                }
                row
            }
            LensElementKind::BrownConrady => LensElementComponent::brown_conrady(
                [
                    self.bc_k1, self.bc_k2, self.bc_k3, self.bc_k4, self.bc_k5, self.bc_k6,
                ],
                [self.bc_p1, self.bc_p2],
                region,
            ),
            LensElementKind::Division => {
                LensElementComponent::division(self.division_l1, self.division_l2, region)
            }
            LensElementKind::AnamorphicDistortion => LensElementComponent::anamorphic_distortion(
                [self.mumps_kx1, self.mumps_kx2],
                [self.mumps_ky1, self.mumps_ky2],
                region,
            ),
            LensElementKind::AnamorphicSqueeze => {
                LensElementComponent::anamorphic_squeeze(self.squeeze.max(0.1), region)
            }
            LensElementKind::Shift => {
                LensElementComponent::shift(self.shift_x, self.shift_y, region)
            }
            LensElementKind::Keystone => {
                LensElementComponent::keystone(self.keystone_x, self.keystone_y, region)
            }
            LensElementKind::FocusBreathing => {
                let mut row = LensElementComponent::focus_breathing(self.breathing_per_dpt, region);
                row.params[helio_pass_lens::breathing::REFERENCE_DIOPTRES] =
                    self.breathing_reference_dpt;
                row
            }
            LensElementKind::Prism => {
                let mut row = LensElementComponent::prism(
                    self.prism_count.clamp(2, 8),
                    self.prism_offset,
                    self.prism_angle_deg.to_radians(),
                    region,
                )
                .with_prism_spin(self.prism_spin_deg.to_radians())
                .with_prism_falloff(self.prism_falloff);
                if self.prism_ring {
                    row = row.with_prism_ring();
                }
                row
            }
            LensElementKind::LateralCa => {
                LensElementComponent::lateral_ca(self.lateral_ca_d1, self.lateral_ca_d2, region)
            }
            LensElementKind::FlatPort => {
                LensElementComponent::flat_port(self.port_index, self.port_dispersion, region)
            }
            LensElementKind::DomePort => {
                use helio_pass_lens::dome_port;
                let mut row = LensElementComponent::dome_port(
                    self.dome_radius_m,
                    self.dome_pupil_offset_m,
                    region,
                );
                row.params[dome_port::INDEX] = self.port_index;
                row.params[dome_port::DISPERSION] = self.port_dispersion;
                row.params[dome_port::CURVATURE] = self.dome_curvature;
                row
            }
            LensElementKind::NaturalVignette => {
                LensElementComponent::natural_vignette(self.natural_vignette_amount, region)
            }
            LensElementKind::OpticalVignette => LensElementComponent::optical_vignette(
                self.cats_eye,
                self.barrel_radius,
                self.vignette_reference_f_number,
                region,
            ),
            LensElementKind::MechanicalVignette => {
                if self.hood_rectangular {
                    LensElementComponent::matte_box(
                        [self.hood_size_x, self.hood_size_y],
                        self.hood_rotation_deg.to_radians(),
                        self.hood_feather,
                        region,
                    )
                } else {
                    LensElementComponent::circular_hood(self.hood_size_x, self.hood_feather, region)
                }
            }
            LensElementKind::NdFilter => LensElementComponent::nd_filter(self.nd_stops, region),
            LensElementKind::GraduatedNd => LensElementComponent::graduated_nd(
                self.grad_nd_stops,
                self.grad_nd_angle_deg.to_radians(),
                self.grad_nd_offset,
                self.grad_nd_softness,
                [self.tint_r, self.tint_g, self.tint_b],
                region,
            ),
            LensElementKind::ColorFilter => LensElementComponent::color_filter(
                [self.tint_r, self.tint_g, self.tint_b],
                self.color_filter_amount,
                region,
            ),
            LensElementKind::TStopLoss => {
                LensElementComponent::t_stop_loss(self.t_stop_loss_amount, region)
            }
            LensElementKind::SphericalGlow => {
                use helio_pass_lens::glow;
                let mut row = LensElementComponent::spherical_glow(
                    self.glow_amount,
                    self.glow_reference_f_number,
                    self.glow_exponent,
                    self.glow_radius,
                    region,
                );
                row.params[glow::TAIL] = self.glow_tail;
                row
            }
            LensElementKind::Coma => {
                use helio_pass_lens::coma;
                let mut row =
                    LensElementComponent::coma(self.coma_amount, self.coma_length, region);
                row.params[coma::EXPONENT] = self.coma_exponent;
                row.params[coma::SHAPE] = self.coma_shape;
                row
            }
            LensElementKind::Astigmatism => {
                use helio_pass_lens::astigmatism;
                let mut row = LensElementComponent::astigmatism(
                    self.astigmatism_amount,
                    self.astigmatism_minor_ratio,
                    region,
                );
                row.params[astigmatism::EXPONENT] = self.astigmatism_exponent;
                row
            }
            LensElementKind::CornerSoftness => {
                use helio_pass_lens::corner_softness;
                let mut row = LensElementComponent::corner_softness(
                    self.corner_softness_sigma,
                    self.corner_softness_start,
                    region,
                );
                row.params[corner_softness::EXPONENT] = self.corner_softness_exponent;
                row
            }
            LensElementKind::Diffusion
            | LensElementKind::ProMist
            | LensElementKind::BlackProMist
            | LensElementKind::Glimmerglass
            | LensElementKind::Fog => {
                let (strength, radius) = (self.diffusion_strength, self.diffusion_radius);
                let mut row = match self.kind {
                    LensElementKind::ProMist => LensElementComponent::pro_mist(strength, region),
                    LensElementKind::BlackProMist => {
                        LensElementComponent::black_pro_mist(strength, region)
                    }
                    LensElementKind::Glimmerglass => {
                        LensElementComponent::glimmerglass(strength, region)
                    }
                    LensElementKind::Fog => LensElementComponent::fog(strength, region),
                    _ => LensElementComponent::diffusion(strength, radius, region),
                };
                row.params[helio_pass_lens::diffusion::RADIUS] = radius;
                row = row.with_diffusion_tail(self.diffusion_tail);
                if self.kind == LensElementKind::Glimmerglass {
                    row = row.with_glint(self.glint_contrast);
                }
                row
            }
            LensElementKind::StarFilter => {
                let mut row = LensElementComponent::star_filter(
                    self.star_points,
                    self.star_rotation_deg.to_radians(),
                    self.star_length,
                    self.star_intensity,
                    region,
                );
                row.params[helio_pass_lens::star::FALLOFF] = self.star_falloff;
                row
            }
            LensElementKind::Halation => {
                use helio_pass_lens::halation;
                let mut row = LensElementComponent::halation(
                    self.halation_strength,
                    self.halation_radius,
                    region,
                )
                .with_halation_tint([
                    self.halation_tint_r,
                    self.halation_tint_g,
                    self.halation_tint_b,
                ]);
                row.params[halation::TAIL] = self.halation_tail;
                row
            }
            LensElementKind::WaterDrops => LensElementComponent::water_drops(
                self.drops_refraction,
                self.drops_normal_layer,
                self.drops_defocus,
                region,
            ),
            LensElementKind::Smear => LensElementComponent::smear(
                self.smear_strength,
                self.smear_length,
                self.smear_angle_deg.to_radians(),
                region,
            ),
            LensElementKind::Dirt => {
                use helio_pass_lens::dirt;
                let mut row = LensElementComponent::dirt(
                    self.dirt_strength,
                    self.dirt_radius,
                    self.dirt_loss,
                    region,
                );
                row.params[dirt::TAIL] = self.dirt_tail;
                row
            }
            LensElementKind::Prescription => {
                LensElementComponent::prescription(self.prescription_lens.layer(), region)
                    .with_psf_amount(self.psf_amount)
            }
            LensElementKind::User => {
                let hooks = [
                    (self.user_map, user::MAP),
                    (self.user_transmission, user::TRANSMISSION),
                    (self.user_focus, user::FOCUS),
                    (self.user_spectral, user::SPECTRAL),
                ]
                .into_iter()
                .filter(|(on, _)| *on)
                .fold(0, |hooks, (_, bit)| hooks | bit);
                LensElementComponent::user(hooks, self.user_params(), region)
            }
        };
        if self.shape == LensRegionShape::Mask {
            row = row.with_mask_transform(
                self.mask_scale,
                [self.mask_offset_u, self.mask_offset_v],
                self.mask_tile,
            );
        } else if !self.auto_feather {
            row = row.with_feather(self.feather.max(0.0));
        }
        debug_assert!(row.kind != element_kind::NONE);
        row
    }
}

fn serialize_lens_element_json(value: &LensElementProps) -> ReflectResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_lens_element_json(value: serde_json::Value) -> ReflectResult<LensElementProps> {
    serde_json::from_value(value).map_err(|e| ReflectError::DeserializationFailed(e.to_string()))
}

#[pulsar_type(
    serialize_json_with = serialize_lens_element_json,
    deserialize_json_with = deserialize_lens_element_json
)]
pub type RegisteredLensElementProps = LensElementProps;

/// The kind fields [`LensElementProps::to_row`] reads for `kind` (before
/// the per-field conditions in [`LensElementProps::reads`]).
fn kind_fields(kind: LensElementKind) -> &'static [&'static str] {
    use LensElementKind as K;
    match kind {
        K::Diopter => &["focus_m", "seam_darkening", "mount_distance_m"],
        K::SweetSpot => &["extra_blur_px"],
        K::Pupil => &[
            "inherit_iris",
            "blades",
            "blade_curvature",
            "iris_rotation_deg",
            "round_until_f_number",
            "squeeze",
            "cats_eye",
            "barrel_radius",
            "swirl",
            "spherical",
            "spherical_asymmetry",
            "onion_amplitude",
            "onion_rings",
            "apodization",
            "obstruction",
            "aperture_texture_layer",
            "aperture_texture_amount",
        ],
        K::LongitudinalCa => &["red_shift_dpt", "green_shift_dpt", "blue_shift_dpt"],
        K::Projection => &[
            "projection_model",
            "kb_k1",
            "kb_k2",
            "kb_k3",
            "kb_k4",
            "panini_distance",
            "panini_compression",
        ],
        K::BrownConrady => &[
            "bc_k1", "bc_k2", "bc_k3", "bc_k4", "bc_k5", "bc_k6", "bc_p1", "bc_p2",
        ],
        K::Division => &["division_l1", "division_l2"],
        K::AnamorphicDistortion => &["mumps_kx1", "mumps_kx2", "mumps_ky1", "mumps_ky2"],
        K::AnamorphicSqueeze => &["squeeze"],
        K::Shift => &["shift_x", "shift_y"],
        K::Keystone => &["keystone_x", "keystone_y"],
        K::FocusBreathing => &["breathing_per_dpt", "breathing_reference_dpt"],
        K::Prism => &[
            "prism_count",
            "prism_offset",
            "prism_angle_deg",
            "prism_ring",
            "prism_spin_deg",
            "prism_falloff",
        ],
        K::LateralCa => &["lateral_ca_d1", "lateral_ca_d2"],
        K::FlatPort => &["port_index", "port_dispersion"],
        K::DomePort => &[
            "dome_radius_m",
            "dome_pupil_offset_m",
            "port_index",
            "port_dispersion",
            "dome_curvature",
        ],
        K::NaturalVignette => &["natural_vignette_amount"],
        K::OpticalVignette => &["cats_eye", "barrel_radius", "vignette_reference_f_number"],
        K::MechanicalVignette => &[
            "hood_rectangular",
            "hood_size_x",
            "hood_size_y",
            "hood_feather",
            "hood_rotation_deg",
        ],
        K::NdFilter => &["nd_stops"],
        K::GraduatedNd => &[
            "grad_nd_stops",
            "grad_nd_angle_deg",
            "grad_nd_offset",
            "grad_nd_softness",
            "tint_r",
            "tint_g",
            "tint_b",
        ],
        K::ColorFilter => &["tint_r", "tint_g", "tint_b", "color_filter_amount"],
        K::TStopLoss => &["t_stop_loss_amount"],
        K::SphericalGlow => &[
            "glow_amount",
            "glow_reference_f_number",
            "glow_exponent",
            "glow_radius",
            "glow_tail",
        ],
        K::Coma => &["coma_amount", "coma_length", "coma_exponent", "coma_shape"],
        K::Astigmatism => &[
            "astigmatism_amount",
            "astigmatism_exponent",
            "astigmatism_minor_ratio",
        ],
        K::CornerSoftness => &[
            "corner_softness_sigma",
            "corner_softness_exponent",
            "corner_softness_start",
        ],
        K::Diffusion | K::ProMist | K::BlackProMist | K::Fog => {
            &["diffusion_strength", "diffusion_radius", "diffusion_tail"]
        }
        K::Glimmerglass => &[
            "diffusion_strength",
            "diffusion_radius",
            "diffusion_tail",
            "glint_contrast",
        ],
        K::StarFilter => &[
            "star_points",
            "star_rotation_deg",
            "star_length",
            "star_intensity",
            "star_falloff",
        ],
        K::Halation => &[
            "halation_strength",
            "halation_radius",
            "halation_tint_r",
            "halation_tint_g",
            "halation_tint_b",
            "halation_tail",
        ],
        K::WaterDrops => &["drops_refraction", "drops_normal_layer", "drops_defocus"],
        K::Smear => &["smear_strength", "smear_length", "smear_angle_deg"],
        K::Dirt => &["dirt_strength", "dirt_radius", "dirt_loss", "dirt_tail"],
        K::Prescription => &["prescription_lens", "psf_amount"],
        K::User => &[
            "user_map",
            "user_transmission",
            "user_focus",
            "user_spectral",
            "user_p0",
            "user_p1",
            "user_p2",
            "user_p3",
            "user_p4",
            "user_p5",
            "user_p6",
            "user_p7",
            "user_p8",
            "user_p9",
            "user_p10",
            "user_p11",
            "user_p12",
            "user_p13",
            "user_p14",
            "user_p15",
        ],
    }
}

impl LensElementProps {
    /// Whether [`Self::to_row`] reads the field `name` for this element's
    /// kind, region shape and switches; the inspector shows only these.
    pub fn reads(&self, name: &str) -> bool {
        use LensRegionShape as S;
        let shape = self.shape;
        match name {
            "enabled" | "kind" | "shape" => true,
            "auto_feather" => shape != S::Mask,
            "feather" => shape != S::Mask && !self.auto_feather,
            "angle_deg" => matches!(shape, S::HalfPlane | S::Ellipse),
            "offset" => shape == S::HalfPlane,
            "center_x" | "center_y" => matches!(shape, S::Ellipse | S::RadialBand),
            "radius_x" | "radius_y" => shape == S::Ellipse,
            "inner_radius" | "outer_radius" => shape == S::RadialBand,
            "polygon_vertices" | "p0_x" | "p0_y" | "p1_x" | "p1_y" | "p2_x" | "p2_y" => {
                shape == S::Polygon
            }
            "p3_x" | "p3_y" => shape == S::Polygon && self.polygon_vertices >= 4,
            _ if name.starts_with("mask_") => shape == S::Mask,
            _ if !kind_fields(self.kind).contains(&name) => false,
            "blades" | "blade_curvature" | "iris_rotation_deg" => !self.inherit_iris,
            _ if name.starts_with("kb_") => {
                self.projection_model == LensProjectionModel::KannalaBrandt
            }
            _ if name.starts_with("panini_") => {
                self.projection_model == LensProjectionModel::Panini
            }
            "hood_size_y" | "hood_rotation_deg" => self.hood_rectangular,
            _ => true,
        }
    }
}

impl super::reflected_list::ReflectedListItem for LensElementProps {
    const CLASS_NAME: &'static str = "LensElementProps";
    const NOUN: &'static str = "element";
    const MAX_ITEMS: usize = LENS_ELEMENT_SLOTS;

    fn title(&self, index: usize) -> String {
        format!(
            "{}. {:?} ({:?}){}",
            index + 1,
            self.kind,
            self.shape,
            if self.enabled { "" } else { " (off)" }
        )
    }

    fn shows(&self, prop: &pulsar_reflection::PropertyMetadata) -> bool {
        self.reads(prop.name)
    }
}

crate::register_reflected_list_editor!(LensElementProps);

lens_enum! {
    /// A named lens setup. `Custom` uses the fields below it; any other
    /// preset replaces the optics, tilt, curvature, iris and element stack
    /// (and its focus distance, where it sets one) with its own. Bake a
    /// preset into the fields with the `bake_preset` method to adjust it.
    LensPreset {
        #[default]
        Custom,
        Standard50mmF2_8,
        Portrait85mmF1_4,
        SplitDiopter2Way,
        SplitDiopter3Way,
        TiltShiftMiniature,
        LensbabySweetSpot,
        Anamorphic2x,
        VintageHeliosSwirl,
        Fisheye8mm,
        UnderwaterFlatPort,
        ProMistQuarter,
        BlackProMistEighth,
        RainyLens,
        StarFilter6pt,
        FilmHalation,
        GradNd3Stop,
        DoubleGauss50mmF2Traced,
    }
}

impl LensPreset {
    /// The preset's data; `None` for `Custom`.
    pub fn data(self) -> Option<&'static LensPresetData> {
        match self {
            Self::Custom => None,
            other => LENS_PRESETS.iter().find(|p| p.preset == other),
        }
    }
}

/// Where a preset element acts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PresetRegion {
    FullFrame,
    HalfPlane {
        angle_deg: f32,
        offset: f32,
    },
    Ellipse {
        center: [f32; 2],
        radii: [f32; 2],
        rotation_deg: f32,
    },
    RadialBand {
        inner: f32,
        outer: f32,
        center: [f32; 2],
    },
    /// One channel (0 red .. 3 alpha) of a layer of the mask array.
    Mask {
        layer: u32,
        channel: u32,
    },
}

/// One element of a preset: `amount` is the sharp distance in metres for a
/// diopter and pixels of extra defocus for a sweet spot; `tune` sets any
/// other field of the element (every kind added since is configured there).
#[derive(Clone, Copy, Debug)]
pub struct PresetElement {
    pub kind: LensElementKind,
    pub region: PresetRegion,
    pub amount: f32,
    /// `None`: derived from the aperture.
    pub feather: Option<f32>,
    pub seam_darkening: f32,
    pub tune: Option<fn(&mut LensElementProps)>,
}

/// A named lens setup as data.
#[derive(Clone, Copy, Debug)]
pub struct LensPresetData {
    pub preset: LensPreset,
    pub name: &'static str,
    pub focal_length_mm: f32,
    pub f_number: f32,
    pub aperture_blades: u32,
    pub tilt_deg: f32,
    pub tilt_axis_deg: f32,
    pub field_curvature_dpt: f32,
    /// `None` keeps the authored focus distance.
    pub focus_distance_m: Option<f32>,
    pub elements: &'static [PresetElement],
}

const fn diopter(focus_m: f32, region: PresetRegion) -> PresetElement {
    PresetElement {
        kind: LensElementKind::Diopter,
        region,
        amount: focus_m,
        feather: None,
        seam_darkening: 0.15,
        tune: None,
    }
}

/// A preset element of any kind over the whole frame (or `region`), with its
/// fields set by `tune`.
const fn tuned(
    kind: LensElementKind,
    region: PresetRegion,
    tune: fn(&mut LensElementProps),
) -> PresetElement {
    PresetElement {
        kind,
        region,
        amount: 0.0,
        feather: None,
        seam_darkening: 0.0,
        tune: Some(tune),
    }
}

const FULL: PresetRegion = PresetRegion::FullFrame;

/// The named presets. A split diopter names the distance its part of the
/// frame is sharp at; the pass gives it the power `D = 1/s' - 1/s` that
/// takes the lens's focus `s` there.
pub const LENS_PRESETS: &[LensPresetData] = &[
    LensPresetData {
        preset: LensPreset::Standard50mmF2_8,
        name: "Standard 50mm f/2.8",
        focal_length_mm: 50.0,
        f_number: 2.8,
        aperture_blades: 6,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[],
    },
    LensPresetData {
        preset: LensPreset::Portrait85mmF1_4,
        name: "Portrait 85mm f/1.4",
        focal_length_mm: 85.0,
        f_number: 1.4,
        aperture_blades: 9,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[],
    },
    // Right half of the frame focuses nearer; the seam is the filter's edge.
    LensPresetData {
        preset: LensPreset::SplitDiopter2Way,
        name: "2-way split diopter",
        focal_length_mm: 50.0,
        f_number: 5.6,
        aperture_blades: 6,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: Some(4.0),
        elements: &[diopter(
            0.6,
            PresetRegion::HalfPlane {
                angle_deg: 0.0,
                offset: 0.0,
            },
        )],
    },
    // Left third nearest, centre at the base focus, right third in between.
    LensPresetData {
        preset: LensPreset::SplitDiopter3Way,
        name: "3-way split diopter",
        focal_length_mm: 50.0,
        f_number: 5.6,
        aperture_blades: 6,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: Some(6.0),
        elements: &[
            diopter(
                0.5,
                PresetRegion::HalfPlane {
                    angle_deg: 180.0,
                    offset: 0.3,
                },
            ),
            diopter(
                1.1,
                PresetRegion::HalfPlane {
                    angle_deg: 0.0,
                    offset: 0.3,
                },
            ),
        ],
    },
    // A wedge-shaped focal plane: a sharp band across the frame, the rest
    // falling away, which reads as a miniature.
    LensPresetData {
        preset: LensPreset::TiltShiftMiniature,
        name: "Tilt-shift miniature",
        focal_length_mm: 65.0,
        f_number: 2.0,
        aperture_blades: 8,
        tilt_deg: 6.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: Some(5.0),
        elements: &[],
    },
    // Sharp in a central ellipse, increasingly soft outside it.
    LensPresetData {
        preset: LensPreset::LensbabySweetSpot,
        name: "Lensbaby sweet spot",
        focal_length_mm: 50.0,
        f_number: 2.0,
        aperture_blades: 6,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[PresetElement {
            kind: LensElementKind::SweetSpot,
            region: PresetRegion::Ellipse {
                center: [0.0, 0.0],
                radii: [0.22, 0.22],
                rotation_deg: 0.0,
            },
            amount: 14.0,
            feather: Some(0.14),
            seam_darkening: 0.0,
            tune: None,
        }],
    },
    // 2x anamorphic: bokeh twice as tall as wide, mumps (barrel across,
    // pincushion down) and the squeeze's doubled horizontal view.
    LensPresetData {
        preset: LensPreset::Anamorphic2x,
        name: "Anamorphic 2x (oval bokeh, mumps, squeeze)",
        focal_length_mm: 50.0,
        f_number: 2.8,
        aperture_blades: 0,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[
            tuned(LensElementKind::Pupil, FULL, |p| {
                p.inherit_iris = true;
                p.squeeze = 2.0;
                p.cats_eye = 0.35;
                p.barrel_radius = 1.0;
            }),
            tuned(LensElementKind::AnamorphicDistortion, FULL, |p| {
                p.mumps_kx1 = -0.04;
                p.mumps_ky1 = 0.03;
            }),
            tuned(LensElementKind::AnamorphicSqueeze, FULL, |p| {
                p.squeeze = 2.0
            }),
        ],
    },
    // A swirly, soft-cornered vintage portrait lens.
    LensPresetData {
        preset: LensPreset::VintageHeliosSwirl,
        name: "Vintage Helios swirl",
        focal_length_mm: 58.0,
        f_number: 2.0,
        aperture_blades: 8,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[
            tuned(LensElementKind::Pupil, FULL, |p| {
                p.swirl = 0.8;
                p.cats_eye = 0.6;
                p.barrel_radius = 1.0;
                p.spherical = 0.25;
            }),
            tuned(LensElementKind::OpticalVignette, FULL, |p| {
                p.cats_eye = 0.6;
                p.barrel_radius = 1.0;
            }),
            tuned(LensElementKind::CornerSoftness, FULL, |p| {
                p.corner_softness_sigma = 0.006;
                p.corner_softness_start = 0.4;
            }),
            tuned(LensElementKind::SphericalGlow, FULL, |p| {
                p.glow_amount = 0.15
            }),
        ],
    },
    LensPresetData {
        preset: LensPreset::Fisheye8mm,
        name: "8mm fisheye",
        focal_length_mm: 8.0,
        f_number: 5.6,
        aperture_blades: 6,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[
            tuned(LensElementKind::Projection, FULL, |p| {
                p.projection_model = LensProjectionModel::Equidistant
            }),
            tuned(LensElementKind::NaturalVignette, FULL, |p| {
                p.natural_vignette_amount = 0.6
            }),
            tuned(LensElementKind::LateralCa, FULL, |p| {
                p.lateral_ca_d1 = 0.004
            }),
        ],
    },
    // Behind a flat port the view narrows by the water's index and colours
    // split; water tints the light blue-green and scatters a little.
    LensPresetData {
        preset: LensPreset::UnderwaterFlatPort,
        name: "Underwater flat port",
        focal_length_mm: 24.0,
        f_number: 4.0,
        aperture_blades: 7,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[
            tuned(LensElementKind::FlatPort, FULL, |p| {
                p.port_index = 1.333;
                p.port_dispersion = 0.006;
            }),
            tuned(LensElementKind::ColorFilter, FULL, |p| {
                (p.tint_r, p.tint_g, p.tint_b) = (0.45, 0.85, 1.0);
                p.color_filter_amount = 0.5;
            }),
            tuned(LensElementKind::Diffusion, FULL, |p| {
                p.diffusion_strength = 0.1
            }),
        ],
    },
    LensPresetData {
        preset: LensPreset::ProMistQuarter,
        name: "Pro-Mist 1/4",
        focal_length_mm: 50.0,
        f_number: 2.8,
        aperture_blades: 6,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[tuned(LensElementKind::ProMist, FULL, |p| {
            p.diffusion_strength = 0.25
        })],
    },
    LensPresetData {
        preset: LensPreset::BlackProMistEighth,
        name: "Black Pro-Mist 1/8",
        focal_length_mm: 50.0,
        f_number: 2.8,
        aperture_blades: 6,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[tuned(LensElementKind::BlackProMist, FULL, |p| {
            p.diffusion_strength = 0.125
        })],
    },
    // Drops on the front glass, their footprint in the alpha of mask layer 0
    // (and their normals in its red and green), under a faint diffusion.
    LensPresetData {
        preset: LensPreset::RainyLens,
        name: "Rainy lens",
        focal_length_mm: 35.0,
        f_number: 2.8,
        aperture_blades: 6,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[
            tuned(
                LensElementKind::WaterDrops,
                PresetRegion::Mask {
                    layer: 0,
                    channel: 3,
                },
                |p| {
                    p.drops_refraction = 0.03;
                    p.drops_normal_layer = 0;
                    p.drops_defocus = 0.004;
                },
            ),
            tuned(LensElementKind::Diffusion, FULL, |p| {
                p.diffusion_strength = 0.08
            }),
        ],
    },
    LensPresetData {
        preset: LensPreset::StarFilter6pt,
        name: "Star filter 6pt",
        focal_length_mm: 50.0,
        f_number: 4.0,
        aperture_blades: 6,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[tuned(LensElementKind::StarFilter, FULL, |p| {
            p.star_points = 6;
            p.star_length = 0.2;
            p.star_intensity = 0.2;
        })],
    },
    LensPresetData {
        preset: LensPreset::FilmHalation,
        name: "Film halation",
        focal_length_mm: 50.0,
        f_number: 2.8,
        aperture_blades: 6,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[tuned(LensElementKind::Halation, FULL, |p| {
            p.halation_strength = 0.4;
            p.halation_radius = 0.025;
        })],
    },
    // Three stops of density over the top of the frame, softly blended.
    LensPresetData {
        preset: LensPreset::GradNd3Stop,
        name: "Grad ND 3-stop",
        focal_length_mm: 24.0,
        f_number: 8.0,
        aperture_blades: 6,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: Some(100.0),
        elements: &[tuned(LensElementKind::GraduatedNd, FULL, |p| {
            p.grad_nd_stops = 3.0;
            p.grad_nd_angle_deg = -90.0;
            p.grad_nd_offset = 0.05;
            p.grad_nd_softness = 0.3;
        })],
    },
    // The bundled double Gauss over the whole frame: its barrel distortion,
    // lateral colour, vignetting at the stop, cat's-eye bokeh, field curvature
    // and point spread are traced, and its focal length is the lens's.
    LensPresetData {
        preset: LensPreset::DoubleGauss50mmF2Traced,
        name: "Double Gauss 50mm f/2 (traced)",
        focal_length_mm: 50.0,
        f_number: 2.0,
        aperture_blades: 9,
        tilt_deg: 0.0,
        tilt_axis_deg: 0.0,
        field_curvature_dpt: 0.0,
        focus_distance_m: None,
        elements: &[tuned(LensElementKind::Prescription, FULL, |p| {
            p.prescription_lens = LensPrescriptionChoice::DoubleGauss50F2;
            p.psf_amount = 1.0;
        })],
    },
];

/// A camera's lens (authored).
#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Camera", category_color = "#3AA0FF")]
#[category("Optics", category_color = "#E0B040")]
#[category("Focus", category_color = "#50C878")]
#[category("Focal Surface", category_color = "#C080E0")]
#[category("Elements", category_color = "#E07050")]
#[serde(default)]
pub struct CameraLensComponent {
    #[property(category = "Camera")]
    pub enabled: bool,
    /// Render view this lens belongs to (the editor viewport is view 0);
    /// independent of any editor camera type.
    #[property(category = "Camera")]
    pub view_id: u32,
    /// A named setup, or Custom to use the fields below.
    #[property(category = "Camera")]
    pub preset: LensPreset,
    #[property(min = 4.0, max = 1200.0, step = 1.0, category = "Optics")]
    pub focal_length_mm: f32,
    #[property(min = 0.7, max = 64.0, step = 0.1, category = "Optics")]
    pub f_number: f32,
    /// Transmission stop, carried for exposure (0: the f-number).
    #[property(min = 0.0, max = 64.0, step = 0.1, category = "Optics")]
    pub t_stop: f32,
    #[property(min = 1.0, max = 120.0, step = 0.1, category = "Optics")]
    pub sensor_width_mm: f32,
    /// 0 derives the height at 3:2.
    #[property(min = 0.0, max = 120.0, step = 0.1, category = "Optics")]
    pub sensor_height_mm: f32,
    /// Iris blades (0: circular); shape the flare pupil.
    #[property(min = 0.0, max = 16.0, step = 1.0, category = "Optics")]
    pub aperture_blades: u32,
    #[property(min = -180.0, max = 180.0, step = 1.0, category = "Optics")]
    pub aperture_rotation_deg: f32,
    #[property(min = 0.0, max = 64.0, step = 0.5, category = "Optics")]
    pub max_coc_px: f32,
    #[property(category = "Optics")]
    pub quality: LensGatherQuality,
    #[property(category = "Focus")]
    pub focus_mode: LensFocusMode,
    /// Manual focus distance in metres.
    #[property(min = 0.05, max = 100000.0, step = 0.05, category = "Focus")]
    pub focus_distance_m: f32,
    /// Autofocus point in uv (0..1, y down) and its sample radius.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Focus")]
    pub af_u: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Focus")]
    pub af_v: f32,
    #[property(min = 0.0, max = 0.5, step = 0.005, category = "Focus")]
    pub af_radius: f32,
    /// Autofocus ease rate in 1/s (0: instant).
    #[property(min = 0.0, max = 60.0, step = 0.1, category = "Focus")]
    pub af_speed: f32,
    /// Scheimpflug tilt of the focal plane in degrees, about an axis in
    /// the image plane at `tilt_axis_deg` from +x.
    #[property(min = -30.0, max = 30.0, step = 0.1, category = "Focal Surface")]
    pub tilt_deg: f32,
    #[property(min = -180.0, max = 180.0, step = 1.0, category = "Focal Surface")]
    pub tilt_axis_deg: f32,
    /// Petzval field curvature in dioptres at the frame corner.
    #[property(min = -4.0, max = 4.0, step = 0.01, category = "Focal Surface")]
    pub field_curvature_dpt: f32,
    /// The ordered element stack (at most [`LENS_ELEMENT_SLOTS`]); ignored
    /// by a preset other than Custom.
    #[property(category = "Elements")]
    pub elements: Vec<LensElementProps>,
}

impl Default for CameraLensComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            view_id: 0,
            preset: LensPreset::Custom,
            focal_length_mm: 50.0,
            f_number: 2.8,
            t_stop: 0.0,
            sensor_width_mm: 36.0,
            sensor_height_mm: 24.0,
            aperture_blades: 6,
            aperture_rotation_deg: 0.0,
            max_coc_px: 0.0,
            quality: LensGatherQuality::Economical,
            focus_mode: LensFocusMode::Manual,
            focus_distance_m: 3.0,
            af_u: 0.5,
            af_v: 0.5,
            af_radius: 0.0,
            af_speed: 0.0,
            tilt_deg: 0.0,
            tilt_axis_deg: 0.0,
            field_curvature_dpt: 0.0,
            elements: Vec::new(),
        }
    }
}

impl CameraLensComponent {
    /// This lens set to `preset`'s values in its own fields, as `Custom`,
    /// ready to adjust. Returns false for `Custom`.
    pub fn bake_preset(&mut self, preset: LensPreset) -> bool {
        let Some(data) = preset.data() else {
            return false;
        };
        self.focal_length_mm = data.focal_length_mm;
        self.f_number = data.f_number;
        self.aperture_blades = data.aperture_blades;
        self.tilt_deg = data.tilt_deg;
        self.tilt_axis_deg = data.tilt_axis_deg;
        self.field_curvature_dpt = data.field_curvature_dpt;
        if let Some(distance) = data.focus_distance_m {
            self.focus_distance_m = distance;
        }
        self.elements = data
            .elements
            .iter()
            .map(LensElementProps::from_preset)
            .collect();
        self.preset = LensPreset::Custom;
        true
    }

    /// The lens with its preset applied: the authored fields, with the
    /// preset's values in place of the optics and the element stack.
    pub fn resolved(&self) -> Self {
        let mut lens = self.clone();
        if lens.bake_preset(self.preset) {
            lens.preset = self.preset;
        }
        lens
    }

    /// The pass rows: the lens, and its element rows from `element_first`
    /// (inert past the stack; at most [`LENS_ELEMENT_SLOTS`] elements).
    pub fn to_rows(
        &self,
        element_first: u32,
    ) -> (LensComponent, [LensElementComponent; LENS_ELEMENT_SLOTS]) {
        let lens = self.resolved();
        let mut elements = [bytemuck::Zeroable::zeroed(); LENS_ELEMENT_SLOTS];
        let mut row = LensComponent::new(lens.view_id);
        row.enabled = u32::from(lens.enabled);
        if !lens.enabled {
            return (row, elements);
        }
        let count = lens.elements.len().min(LENS_ELEMENT_SLOTS);
        for (slot, element) in elements.iter_mut().zip(&lens.elements[..count]) {
            *slot = element.to_row();
        }
        row.focus_mode = match lens.focus_mode {
            LensFocusMode::Manual => FOCUS_MANUAL,
            LensFocusMode::AutoPoint => FOCUS_AUTO_POINT,
        };
        row.quality = match lens.quality {
            LensGatherQuality::Economical => 0,
            LensGatherQuality::High => 1,
            LensGatherQuality::Ultra => 2,
        };
        row.focal_length_mm = lens.focal_length_mm.max(1.0);
        row.f_number = lens.f_number.max(0.5);
        row.t_stop = if lens.t_stop > 0.0 {
            lens.t_stop
        } else {
            row.f_number
        };
        row.sensor_width_mm = lens.sensor_width_mm.max(1.0);
        row.sensor_height_mm = lens.sensor_height_mm.max(0.0);
        row.focus_distance_m = lens.focus_distance_m.max(0.01);
        row.af_u = lens.af_u;
        row.af_v = lens.af_v;
        row.af_radius = lens.af_radius;
        row.af_speed = lens.af_speed;
        row.max_coc_px = lens.max_coc_px;
        row.element_first = element_first;
        row.element_count = count as u32;
        row.tilt_angle = lens.tilt_deg.to_radians();
        row.tilt_axis = lens.tilt_axis_deg.to_radians();
        row.field_curvature = lens.field_curvature_dpt;
        row.aperture_blades = lens.aperture_blades;
        row.aperture_rotation = lens.aperture_rotation_deg.to_radians();
        (row, elements)
    }
}

#[register_world_component]
impl CameraLensComponent {}

/// The lens's script methods.
fn camera_lens_methods() -> Vec<pulsar_reflection::MethodMetadata> {
    use pulsar_reflection::{
        EngineClass, MethodFlags, MethodMetadata, MethodParameter, MethodReturnType,
        RUNTIME_TYPE_REGISTRY,
    };
    let (Some(u32_info), Some(bool_info)) = (
        RUNTIME_TYPE_REGISTRY.get::<u32>(),
        RUNTIME_TYPE_REGISTRY.get::<bool>(),
    ) else {
        return Vec::new();
    };
    vec![MethodMetadata {
        name: "bake_preset",
        display_name: "Bake Preset".into(),
        category: Some("Camera"),
        params: vec![MethodParameter {
            name: "preset_index",
            type_info: u32_info,
        }],
        return_type: Some(MethodReturnType {
            type_info: bool_info,
        }),
        flags: MethodFlags::NONE,
        caller: Box::new(
            |instance: &mut dyn EngineClass, args: Vec<Box<dyn std::any::Any>>| {
                let index = *args.first()?.downcast_ref::<u32>()? as usize;
                let preset = *LensPreset::ALL.get(index)?;
                let lens = instance
                    .as_any_mut()
                    .downcast_mut::<CameraLensComponent>()?;
                Some(Box::new(lens.bake_preset(preset)))
            },
        ),
    }]
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::ComponentMethodRegistration {
        class_name: CAMERA_LENS_CLASS_NAME,
        methods: camera_lens_methods,
    }
}

/// Where each lens's element block lives in `lens_elements`: blocks of
/// [`LENS_ELEMENT_SLOTS`] rows from a free list, per GPU store. It also holds
/// what the lens's overscan was derived for, see [`camera_lens_view_overscan`].
#[derive(Default)]
struct ElementBlocks {
    next: u32,
    free: Vec<u32>,
    by_lens_row: HashMap<u32, u32>,
    /// By lens row: the image aspect the row's overscan was last derived for,
    /// and that overscan.
    overscan: HashMap<u32, (f32, f32)>,
}

impl ElementBlocks {
    fn block_of(&mut self, lens_row: u32) -> u32 {
        if let Some(&block) = self.by_lens_row.get(&lens_row) {
            return block;
        }
        let block = self.free.pop().unwrap_or_else(|| {
            let block = self.next;
            self.next += 1;
            block
        });
        self.by_lens_row.insert(lens_row, block);
        block
    }

    fn release(&mut self, lens_row: u32) -> Option<u32> {
        self.overscan.remove(&lens_row);
        let block = self.by_lens_row.remove(&lens_row)?;
        self.free.push(block);
        Some(block)
    }
}

static BLOCKS: Mutex<Option<HashMap<usize, ElementBlocks>>> = Mutex::new(None);

fn with_blocks<T>(mirror: &GpuMirrorHandle, f: impl FnOnce(&mut ElementBlocks) -> T) -> T {
    let key = mirror.store() as *const SceneGpuStore as usize;
    let mut blocks = BLOCKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(blocks
        .get_or_insert_with(HashMap::new)
        .entry(key)
        .or_default())
}

fn camera_lens_dispatch(mirror: &GpuMirrorHandle, row: u32, data: *const (), is_new_insert: bool) {
    // SAFETY: SceneDB reaches this only through `CameraLensComponent`'s own
    // `ComponentId`, with a pointer to a live value.
    let authored = unsafe { &*(data as *const CameraLensComponent) };
    let (block, aspect) = with_blocks(mirror, |blocks| {
        let aspect = blocks
            .overscan
            .get(&row)
            .map_or(DEFAULT_ASPECT, |(aspect, _)| *aspect);
        (blocks.block_of(row), aspect)
    });
    let first = block * LENS_ELEMENT_SLOTS as u32;
    let (mut lens, elements) = authored.to_rows(first);
    // The camera widens its view by this much; the row tells the pass.
    lens.overscan = overscan_of(&lens, &elements, aspect);
    with_blocks(mirror, |blocks| {
        blocks.overscan.insert(row, (aspect, lens.overscan))
    });
    pulsar_scenedb::gpu::write_derived_row(
        mirror,
        row,
        &CameraLensSourceRow::of(&lens),
        is_new_insert,
    );
    for (slot, element) in elements.iter().enumerate() {
        pulsar_scenedb::gpu::write_derived_row(mirror, first + slot as u32, element, is_new_insert);
    }
}

/// The image aspect a lens row is derived for until the host says another
/// ([`camera_lens_view_overscan`]).
const DEFAULT_ASPECT: f32 = 16.0 / 9.0;

/// The overscan a camera applies for the lens `row` with its `elements` on an
/// image of the given width over height. The bundled prescriptions' ray maps
/// are traced on first use (a lens with a `Prescription` element waits for
/// that once, in the background of the first frames if the renderer started
/// them, else here).
fn overscan_of(row: &LensComponent, elements: &[LensElementComponent], aspect: f32) -> f32 {
    let count = (row.element_count as usize).min(elements.len());
    let elements = &elements[..count];
    let rays = if elements
        .iter()
        .any(|e| e.kind == element_kind::PRESCRIPTION)
    {
        bundled_rays()
    } else {
        &[]
    };
    applied_overscan(row, elements, aspect, rays)
}

/// The overscan the camera of render view `view_id` applies, for an image of
/// the given width over height: the factor to widen the projection's tangent
/// extent by (`helio::Camera::with_overscan`) so the view's lens, which maps
/// beyond the frame (a fisheye, a barrel distortion), reads real pixels
/// instead of the clamped edge. 1 for a view without an enabled lens, with a
/// lens that stays inside the frame, or on a world with no GPU mirror.
///
/// It also gives the lens pass the same factor, in the lens's GPU row, so the
/// pass crops the wider image back to the frame: call it once per frame,
/// before the world's GPU mirror is flushed and the frame rendered, and use
/// the result for that frame's camera. The overscan depends on the aspect
/// (the lens's field of view follows the sensor width across the image), so
/// the row is rewritten when the aspect changes. Renderers that share a view
/// id and a world but not an aspect (an editor viewport and a play-in-editor
/// window) take turns: each must call it right before its own frame.
pub fn camera_lens_view_overscan(world: &pulsar_scenedb::World, view_id: u32, aspect: f32) -> f32 {
    let Some(mirror) = world.gpu_mirror() else {
        return 1.0;
    };
    // The pass reads the lowest enabled row of the view.
    let Some((instance, _, lens)) =
        pulsar_scene_model::attachments::enabled_components::<CameraLensComponent>(world)
            .filter(|(_, _, lens)| lens.enabled && lens.view_id == view_id)
            .min_by_key(|(instance, _, _)| instance.index())
    else {
        return 1.0;
    };
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        DEFAULT_ASPECT
    };
    let row = instance.index();
    let cached = |blocks: &mut ElementBlocks| blocks.overscan.get(&row).copied();
    if let Some((cached_aspect, overscan)) = with_blocks(mirror, cached) {
        if cached_aspect == aspect {
            return overscan;
        }
    }
    with_blocks(mirror, |blocks| {
        let overscan = blocks
            .overscan
            .get(&row)
            .map_or(1.0, |(_, overscan)| *overscan);
        blocks.overscan.insert(row, (aspect, overscan));
    });
    camera_lens_dispatch(
        mirror,
        row,
        lens as *const CameraLensComponent as *const (),
        false,
    );
    with_blocks(mirror, cached).map_or(1.0, |(_, overscan)| overscan)
}

fn camera_lens_clear(mirror: &GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<CameraLensSourceRow>(mirror, row);
    // A clear can arrive after its store entry was already removed (for
    // example during world teardown). Do not create an empty permanent entry
    // just to release a row, and drop the per-store bookkeeping once the last
    // lens has gone away.
    let key = mirror.store() as *const SceneGpuStore as usize;
    let block = {
        let mut stores = BLOCKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(by_store) = stores.as_mut() else {
            return;
        };
        let block = by_store.get_mut(&key).and_then(|blocks| blocks.release(row));
        if by_store
            .get(&key)
            .is_some_and(|blocks| blocks.by_lens_row.is_empty())
        {
            by_store.remove(&key);
        }
        block
    };
    if let Some(block) = block {
        let first = block * LENS_ELEMENT_SLOTS as u32;
        for slot in 0..LENS_ELEMENT_SLOTS as u32 {
            pulsar_scenedb::gpu::clear_derived_row::<LensElementComponent>(mirror, first + slot);
        }
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<CameraLensComponent>,
        dispatch: camera_lens_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<CameraLensComponent>,
        clear: camera_lens_clear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helio_pass_lens::region_kind;
    use pulsar_reflection::EngineClass;

    #[test]
    fn the_default_lens_is_the_passes_neutral_lens() {
        let (row, elements) = CameraLensComponent::default().to_rows(0);
        let mut expected = LensComponent::new(0);
        expected.t_stop = expected.f_number;
        assert_eq!(row, expected);
        assert!(elements.iter().all(|e| e.kind == element_kind::NONE));
    }

    #[test]
    fn a_disabled_lens_is_an_inert_row_with_no_elements() {
        let mut lens = CameraLensComponent::default();
        lens.preset = LensPreset::SplitDiopter2Way;
        lens.enabled = false;
        let (row, elements) = lens.to_rows(8);
        assert_eq!(row.enabled, 0);
        assert_eq!(row.element_count, 0);
        assert!(elements.iter().all(|e| e.kind == element_kind::NONE));
    }

    #[test]
    fn fields_reach_the_row_in_the_passes_units() {
        let mut lens = CameraLensComponent::default();
        lens.view_id = 3;
        lens.focal_length_mm = 85.0;
        lens.f_number = 1.4;
        lens.tilt_deg = 90.0;
        lens.tilt_axis_deg = 180.0;
        lens.aperture_rotation_deg = 45.0;
        lens.focus_mode = LensFocusMode::AutoPoint;
        lens.quality = LensGatherQuality::High;
        let (row, _) = lens.to_rows(16);
        assert_eq!(row.view_id, 3);
        assert_eq!(
            (row.focal_length_mm, row.f_number, row.t_stop),
            (85.0, 1.4, 1.4)
        );
        assert!((row.tilt_angle - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((row.tilt_axis - std::f32::consts::PI).abs() < 1e-6);
        assert!((row.aperture_rotation - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert_eq!((row.focus_mode, row.quality), (FOCUS_AUTO_POINT, 1));
        assert_eq!(row.element_first, 16);
    }

    #[test]
    fn element_rows_follow_the_stack_and_skip_disabled_ones() {
        let mut lens = CameraLensComponent::default();
        let mut off = LensElementProps::default();
        off.enabled = false;
        lens.elements = vec![
            LensElementProps::default(),
            off,
            LensElementProps {
                kind: LensElementKind::SweetSpot,
                shape: LensRegionShape::Ellipse,
                auto_feather: false,
                feather: 0.1,
                ..LensElementProps::default()
            },
        ];
        let (row, elements) = lens.to_rows(0);
        assert_eq!(row.element_count, 3);
        assert_eq!(elements[0].kind, element_kind::DIOPTER);
        assert_eq!(elements[0].region_kind, region_kind::HALF_PLANE);
        assert!(elements[0].feather < 0.0, "derived from the aperture");
        assert_eq!(elements[1].kind, element_kind::NONE, "disabled: inert");
        assert_eq!(elements[2].kind, element_kind::SWEET_SPOT);
        assert_eq!(elements[2].feather, 0.1);
        assert_eq!(elements[3].kind, element_kind::NONE);
    }

    #[test]
    fn pupil_and_chromatic_elements_carry_their_parameters() {
        use helio_pass_lens::pupil;
        let mut lens = CameraLensComponent::default();
        lens.elements = vec![
            LensElementProps {
                kind: LensElementKind::Pupil,
                shape: LensRegionShape::FullFrame,
                inherit_iris: false,
                blades: 9,
                blade_curvature: 0.5,
                squeeze: 1.8,
                cats_eye: 0.7,
                spherical: 0.4,
                obstruction: 0.3,
                ..LensElementProps::default()
            },
            LensElementProps {
                kind: LensElementKind::LongitudinalCa,
                shape: LensRegionShape::FullFrame,
                red_shift_dpt: -0.2,
                blue_shift_dpt: -0.1,
                ..LensElementProps::default()
            },
            LensElementProps {
                kind: LensElementKind::Pupil,
                shape: LensRegionShape::FullFrame,
                ..LensElementProps::default()
            },
        ];
        let (row, elements) = lens.to_rows(0);
        assert_eq!(row.element_count, 3);
        assert_eq!(elements[0].kind, element_kind::PUPIL);
        let p = &elements[0].params;
        assert_eq!((p[pupil::BLADES], p[pupil::CURVATURE]), (9.0, 0.5));
        assert_eq!((p[pupil::SQUEEZE], p[pupil::CATS_EYE]), (1.8, 0.7));
        assert_eq!((p[pupil::SPHERICAL], p[pupil::OBSTRUCTION]), (0.4, 0.3));
        assert_eq!(elements[1].kind, element_kind::LONGITUDINAL_CA);
        assert_eq!(&elements[1].params[..3], &[-0.2, 0.0, -0.1]);
        assert!(
            elements[2].params[pupil::BLADES] < 0.0,
            "inherits the lens iris"
        );
    }

    #[test]
    fn a_long_stack_is_clamped_to_the_block() {
        let mut lens = CameraLensComponent::default();
        lens.elements = vec![LensElementProps::default(); LENS_ELEMENT_SLOTS + 5];
        let (row, _) = lens.to_rows(0);
        assert_eq!(row.element_count as usize, LENS_ELEMENT_SLOTS);
    }

    #[test]
    fn polygon_regions_carry_their_vertices() {
        let mut lens = CameraLensComponent::default();
        lens.elements = vec![LensElementProps {
            shape: LensRegionShape::Polygon,
            polygon_vertices: 4,
            p3_x: 0.25,
            p3_y: 0.5,
            ..LensElementProps::default()
        }];
        let (_, elements) = lens.to_rows(0);
        assert_eq!(elements[0].region_kind, region_kind::POLYGON);
        assert_eq!(elements[0].region_count, 4);
        assert_eq!(&elements[0].region[6..8], &[0.25, 0.5]);
    }

    #[test]
    fn every_preset_has_data_and_resolves_to_valid_rows() {
        for preset in LensPreset::ALL {
            if *preset == LensPreset::Custom {
                assert!(preset.data().is_none());
                continue;
            }
            let data = preset
                .data()
                .unwrap_or_else(|| panic!("{preset:?} has no data"));
            assert!(!data.name.is_empty());
            assert!(data.elements.len() <= LENS_ELEMENT_SLOTS, "{}", data.name);
            let mut lens = CameraLensComponent::default();
            lens.preset = *preset;
            let (row, elements) = lens.to_rows(0);
            assert_eq!(row.focal_length_mm, data.focal_length_mm, "{}", data.name);
            assert_eq!(row.f_number, data.f_number, "{}", data.name);
            assert_eq!(
                row.element_count as usize,
                data.elements.len(),
                "{}",
                data.name
            );
            for element in &elements[..data.elements.len()] {
                assert_ne!(element.kind, element_kind::NONE, "{}", data.name);
                assert!(
                    element
                        .params
                        .iter()
                        .chain(&element.region)
                        .all(|v| v.is_finite()),
                    "{}",
                    data.name
                );
            }
        }
        let names: Vec<_> = LENS_PRESETS.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "Standard 50mm f/2.8",
                "Portrait 85mm f/1.4",
                "2-way split diopter",
                "3-way split diopter",
                "Tilt-shift miniature",
                "Lensbaby sweet spot",
                "Anamorphic 2x (oval bokeh, mumps, squeeze)",
                "Vintage Helios swirl",
                "8mm fisheye",
                "Underwater flat port",
                "Pro-Mist 1/4",
                "Black Pro-Mist 1/8",
                "Rainy lens",
                "Star filter 6pt",
                "Film halation",
                "Grad ND 3-stop",
                "Double Gauss 50mm f/2 (traced)",
            ]
        );
    }

    #[test]
    fn split_diopters_name_the_distance_they_bring_into_focus() {
        let two = LensPreset::SplitDiopter2Way.data().unwrap();
        assert_eq!(two.elements.len(), 1);
        let near = two.elements[0].amount;
        assert!(
            near < two.focus_distance_m.unwrap(),
            "the diopter half is the near one"
        );
        let lens = CameraLensComponent {
            preset: LensPreset::SplitDiopter2Way,
            ..Default::default()
        };
        assert_eq!(
            lens.to_rows(0).1[0].params[0],
            near,
            "the row carries the distance"
        );

        let three = LensPreset::SplitDiopter3Way.data().unwrap();
        assert_eq!(three.elements.len(), 2);
        let base = three.focus_distance_m.unwrap();
        let (left, right) = (three.elements[0].amount, three.elements[1].amount);
        assert!(left < right && right < base, "three distinct focus zones");
        // Left covers x < -0.3, right x > 0.3 (half planes at 180 and 0 deg).
        let angles: Vec<f32> = three
            .elements
            .iter()
            .map(|e| match e.region {
                PresetRegion::HalfPlane { angle_deg, .. } => angle_deg,
                _ => panic!("half planes"),
            })
            .collect();
        assert_eq!(angles, [180.0, 0.0]);
    }

    #[test]
    fn the_tilt_shift_and_sweet_spot_presets_use_their_features() {
        let (tilt, _) = {
            let mut lens = CameraLensComponent::default();
            lens.preset = LensPreset::TiltShiftMiniature;
            lens.to_rows(0)
        };
        assert!(tilt.tilt_angle > 0.0 && tilt.element_count == 0);
        let (spot, elements) = {
            let mut lens = CameraLensComponent::default();
            lens.preset = LensPreset::LensbabySweetSpot;
            lens.to_rows(0)
        };
        assert_eq!(spot.element_count, 1);
        assert_eq!(elements[0].kind, element_kind::SWEET_SPOT);
        assert_eq!(elements[0].region_kind, region_kind::ELLIPSE);
        assert_eq!(elements[0].params[0], 14.0);
        assert_eq!(elements[0].feather, 0.14);
    }

    #[test]
    fn a_preset_keeps_the_authored_focus_unless_it_sets_one() {
        let mut lens = CameraLensComponent::default();
        lens.focus_distance_m = 7.0;
        lens.preset = LensPreset::Portrait85mmF1_4;
        assert_eq!(lens.to_rows(0).0.focus_distance_m, 7.0);
        lens.preset = LensPreset::SplitDiopter2Way;
        assert_eq!(lens.to_rows(0).0.focus_distance_m, 4.0);
    }

    #[test]
    fn baking_a_preset_copies_it_into_the_fields_as_custom() {
        let mut lens = CameraLensComponent::default();
        assert!(!lens.bake_preset(LensPreset::Custom));
        assert!(lens.bake_preset(LensPreset::SplitDiopter3Way));
        assert_eq!(lens.preset, LensPreset::Custom);
        assert_eq!(lens.elements.len(), 2);
        assert_eq!(lens.focus_distance_m, 6.0);
        // The baked lens makes the same rows as the preset did.
        let mut preset = CameraLensComponent::default();
        preset.preset = LensPreset::SplitDiopter3Way;
        assert_eq!(lens.to_rows(0), preset.to_rows(0));
    }

    #[test]
    fn bake_preset_is_a_method_and_the_fields_are_reflected() {
        let methods = <CameraLensComponent as EngineClass>::get_methods();
        let bake = methods
            .iter()
            .find(|m| m.name == "bake_preset")
            .expect("bake_preset is registered");
        let mut lens = CameraLensComponent::default();
        let index = LensPreset::LensbabySweetSpot as u32;
        let done = (bake.caller)(&mut lens, vec![Box::new(index)]).unwrap();
        assert!(*done.downcast_ref::<bool>().unwrap());
        assert_eq!(lens.elements.len(), 1);

        let names: Vec<_> = lens.get_properties().iter().map(|p| p.name).collect();
        for field in [
            "focal_length_mm",
            "f_number",
            "sensor_width_mm",
            "aperture_blades",
            "elements",
            "preset",
        ] {
            assert!(names.contains(&field), "{field} is reflected");
        }
    }

    #[test]
    fn the_authored_value_round_trips_through_json_with_names_or_indices() {
        let mut lens = CameraLensComponent::default();
        lens.preset = LensPreset::SplitDiopter2Way;
        lens.elements = vec![LensElementProps {
            kind: LensElementKind::SweetSpot,
            shape: LensRegionShape::RadialBand,
            ..LensElementProps::default()
        }];
        let json = serde_json::to_value(&lens).unwrap();
        let back: CameraLensComponent = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back.to_rows(0), lens.to_rows(0));
        let by_index: LensElementKind = serde_json::from_value(serde_json::json!(1)).unwrap();
        assert_eq!(by_index, LensElementKind::SweetSpot);
        // A level saved with fewer fields still loads.
        let sparse: CameraLensComponent =
            serde_json::from_value(serde_json::json!({ "f_number": 4.0 })).unwrap();
        assert_eq!(sparse.f_number, 4.0);
        assert_eq!(sparse.focal_length_mm, 50.0);
    }

    /// The row of one element of `kind` with its fields set by `set`, over
    /// the whole frame.
    fn row_of(
        kind: LensElementKind,
        set: impl FnOnce(&mut LensElementProps),
    ) -> LensElementComponent {
        let mut props = LensElementProps {
            kind,
            shape: LensRegionShape::FullFrame,
            ..Default::default()
        };
        set(&mut props);
        let mut lens = CameraLensComponent::default();
        lens.elements = vec![props];
        let (row, elements) = lens.to_rows(0);
        assert_eq!(row.element_count, 1);
        elements[0]
    }

    #[test]
    fn every_element_kind_maps_to_its_pass_kind_id_in_order() {
        use element_kind as k;
        let expected = [
            k::DIOPTER,
            k::SWEET_SPOT,
            k::PUPIL,
            k::LONGITUDINAL_CA,
            k::PROJECTION,
            k::BROWN_CONRADY,
            k::DIVISION,
            k::ANAMORPHIC_DISTORTION,
            k::ANAMORPHIC_SQUEEZE,
            k::SHIFT,
            k::KEYSTONE,
            k::FOCUS_BREATHING,
            k::PRISM,
            k::LATERAL_CA,
            k::FLAT_PORT,
            k::DOME_PORT,
            k::NATURAL_VIGNETTE,
            k::OPTICAL_VIGNETTE,
            k::MECHANICAL_VIGNETTE,
            k::ND_FILTER,
            k::GRADUATED_ND,
            k::COLOR_FILTER,
            k::T_STOP_LOSS,
            k::SPHERICAL_GLOW,
            k::COMA,
            k::ASTIGMATISM,
            k::CORNER_SOFTNESS,
            k::DIFFUSION,
            k::PRO_MIST,
            k::BLACK_PRO_MIST,
            k::GLIMMERGLASS,
            k::STAR_FILTER,
            k::HALATION,
            k::WATER_DROPS,
            k::SMEAR,
            k::FOG,
            k::DIRT,
            k::PRESCRIPTION,
            k::USER,
        ];
        assert_eq!(LensElementKind::ALL.len(), expected.len());
        for (kind, id) in LensElementKind::ALL.iter().zip(expected) {
            let row = row_of(*kind, |_| {});
            assert_eq!(row.kind, id, "{kind:?}");
            assert_eq!(row.region_kind, region_kind::FULL_FRAME, "{kind:?}");
        }
    }

    #[test]
    fn ray_map_kinds_carry_their_fields_to_the_right_slots() {
        use helio_pass_lens::{
            anamorphic_distortion as ad, brown_conrady as bc, division as dv, dome_port as dp,
            keystone as ks, lateral_ca as lca, prism, projection,
        };
        let r = row_of(LensElementKind::Projection, |p| {
            p.projection_model = LensProjectionModel::Stereographic
        });
        assert_eq!(
            r.params[projection::MODEL],
            ProjectionModel::Stereographic as u32 as f32
        );
        let r = row_of(LensElementKind::Projection, |p| {
            p.projection_model = LensProjectionModel::KannalaBrandt;
            (p.kb_k1, p.kb_k2, p.kb_k3, p.kb_k4) = (0.1, 0.2, 0.3, 0.4);
        });
        assert_eq!(
            r.params[projection::MODEL],
            ProjectionModel::KannalaBrandt as u32 as f32
        );
        assert_eq!(
            &r.params[projection::K1..projection::K1 + 4],
            &[0.1, 0.2, 0.3, 0.4]
        );
        let r = row_of(LensElementKind::Projection, |p| {
            p.projection_model = LensProjectionModel::Panini;
            (p.panini_distance, p.panini_compression) = (0.7, 0.4);
        });
        assert_eq!(
            (
                r.params[projection::PANINI_D],
                r.params[projection::PANINI_S]
            ),
            (0.7, 0.4)
        );
        for (i, model) in LensProjectionModel::ALL.iter().enumerate() {
            let r = row_of(LensElementKind::Projection, |p| p.projection_model = *model);
            assert_eq!(r.params[projection::MODEL], i as f32, "{model:?}");
        }

        let r = row_of(LensElementKind::BrownConrady, |p| {
            (p.bc_k1, p.bc_k2, p.bc_k3) = (0.01, 0.02, 0.03);
            (p.bc_k4, p.bc_k5, p.bc_k6) = (0.04, 0.05, 0.06);
            (p.bc_p1, p.bc_p2) = (0.07, 0.08);
        });
        assert_eq!(
            &r.params[bc::K1..=bc::P2],
            &[0.01, 0.02, 0.03, 0.04, 0.05, 0.06, 0.07, 0.08]
        );
        let r = row_of(LensElementKind::Division, |p| {
            (p.division_l1, p.division_l2) = (-0.2, 0.1)
        });
        assert_eq!((r.params[dv::L1], r.params[dv::L2]), (-0.2, 0.1));
        let r = row_of(LensElementKind::AnamorphicDistortion, |p| {
            (p.mumps_kx1, p.mumps_kx2, p.mumps_ky1, p.mumps_ky2) = (0.1, 0.2, 0.3, 0.4);
        });
        assert_eq!(
            (
                r.params[ad::KX1],
                r.params[ad::KX2],
                r.params[ad::KY1],
                r.params[ad::KY2]
            ),
            (0.1, 0.2, 0.3, 0.4)
        );
        let r = row_of(LensElementKind::AnamorphicSqueeze, |p| p.squeeze = 1.8);
        assert_eq!(r.params[0], 1.8);
        let r = row_of(LensElementKind::Shift, |p| {
            (p.shift_x, p.shift_y) = (0.1, -0.2)
        });
        assert_eq!(&r.params[..2], &[0.1, -0.2]);
        let r = row_of(LensElementKind::Keystone, |p| {
            (p.keystone_x, p.keystone_y) = (0.05, -0.1)
        });
        assert_eq!((r.params[ks::KX], r.params[ks::KY]), (0.05, -0.1));
        let r = row_of(LensElementKind::FocusBreathing, |p| {
            (p.breathing_per_dpt, p.breathing_reference_dpt) = (-0.04, 0.2);
        });
        assert_eq!((r.params[0], r.params[1]), (-0.04, 0.2));

        let r = row_of(LensElementKind::Prism, |p| {
            p.prism_count = 5;
            p.prism_offset = 0.03;
            p.prism_angle_deg = 90.0;
            p.prism_ring = true;
            p.prism_spin_deg = 180.0;
            p.prism_falloff = 0.5;
        });
        assert_eq!(
            (r.params[prism::COUNT], r.params[prism::OFFSET]),
            (5.0, 0.03)
        );
        assert!((r.params[prism::ANGLE] - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(r.params[prism::RING], 1.0);
        assert!((r.params[prism::SPIN] - std::f32::consts::PI).abs() < 1e-6);
        assert_eq!(r.params[prism::FALLOFF], 0.5);
        let r = row_of(LensElementKind::Prism, |p| p.prism_count = 99);
        assert_eq!(r.params[prism::COUNT], 8.0, "clamped to the pass's 2..=8");
        assert_eq!(r.params[prism::RING], 0.0);

        let r = row_of(LensElementKind::LateralCa, |p| {
            (p.lateral_ca_d1, p.lateral_ca_d2) = (0.01, -0.02)
        });
        assert_eq!((r.params[lca::D1], r.params[lca::D2]), (0.01, -0.02));
        let r = row_of(LensElementKind::FlatPort, |p| {
            (p.port_index, p.port_dispersion) = (1.5, 0.01)
        });
        assert_eq!((r.params[0], r.params[1]), (1.5, 0.01));
        let r = row_of(LensElementKind::DomePort, |p| {
            p.dome_radius_m = 0.2;
            p.dome_pupil_offset_m = 0.03;
            p.port_index = 1.34;
            p.port_dispersion = 0.007;
            p.dome_curvature = 0.5;
        });
        assert_eq!(
            (
                r.params[dp::RADIUS],
                r.params[dp::PUPIL_OFFSET],
                r.params[dp::INDEX]
            ),
            (0.2, 0.03, 1.34)
        );
        assert_eq!(
            (r.params[dp::DISPERSION], r.params[dp::CURVATURE]),
            (0.007, 0.5)
        );
    }

    #[test]
    fn transmission_kinds_carry_their_fields_to_the_right_slots() {
        use helio_pass_lens::{
            graduated_nd as gnd, mechanical_vignette as mv, optical_vignette as ov,
        };
        let r = row_of(LensElementKind::NaturalVignette, |p| {
            p.natural_vignette_amount = 0.4
        });
        assert_eq!(r.params[0], 0.4);
        let r = row_of(LensElementKind::OpticalVignette, |p| {
            (p.cats_eye, p.barrel_radius, p.vignette_reference_f_number) = (0.5, 0.9, 2.0);
        });
        assert_eq!(
            (
                r.params[ov::CATS_EYE],
                r.params[ov::BARREL_RADIUS],
                r.params[ov::REFERENCE_F_NUMBER]
            ),
            (0.5, 0.9, 2.0)
        );
        let r = row_of(LensElementKind::MechanicalVignette, |p| {
            (p.hood_size_x, p.hood_feather) = (0.7, 0.05);
        });
        assert_eq!(r.params[mv::SHAPE], 0.0, "circular");
        assert_eq!((r.params[mv::SIZE_X], r.params[mv::FEATHER]), (0.7, 0.05));
        let r = row_of(LensElementKind::MechanicalVignette, |p| {
            p.hood_rectangular = true;
            (p.hood_size_x, p.hood_size_y, p.hood_feather) = (0.8, 0.45, 0.02);
            p.hood_rotation_deg = 90.0;
        });
        assert_eq!(r.params[mv::SHAPE], 1.0, "matte box");
        assert_eq!(
            (
                r.params[mv::SIZE_X],
                r.params[mv::SIZE_Y],
                r.params[mv::FEATHER]
            ),
            (0.8, 0.45, 0.02)
        );
        assert!((r.params[mv::ROTATION] - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        let r = row_of(LensElementKind::NdFilter, |p| p.nd_stops = 3.0);
        assert_eq!(r.params[0], 3.0);
        let r = row_of(LensElementKind::GraduatedNd, |p| {
            p.grad_nd_stops = 2.0;
            p.grad_nd_angle_deg = 180.0;
            p.grad_nd_offset = 0.1;
            p.grad_nd_softness = 0.4;
            (p.tint_r, p.tint_g, p.tint_b) = (1.0, 0.8, 0.6);
        });
        assert_eq!(
            (
                r.params[gnd::STOPS],
                r.params[gnd::OFFSET],
                r.params[gnd::SOFTNESS]
            ),
            (2.0, 0.1, 0.4)
        );
        assert!((r.params[gnd::ANGLE] - std::f32::consts::PI).abs() < 1e-6);
        assert_eq!(&r.params[gnd::TINT..gnd::TINT + 3], &[1.0, 0.8, 0.6]);
        let r = row_of(LensElementKind::ColorFilter, |p| {
            (p.tint_r, p.tint_g, p.tint_b, p.color_filter_amount) = (0.9, 0.5, 0.2, 0.75);
        });
        assert_eq!(&r.params[..4], &[0.9, 0.5, 0.2, 0.75]);
        let r = row_of(LensElementKind::TStopLoss, |p| p.t_stop_loss_amount = 0.6);
        assert_eq!(r.params[0], 0.6);
    }

    #[test]
    fn sensor_image_kinds_carry_their_fields_to_the_right_slots() {
        use helio_pass_lens::{
            astigmatism as astig, coma, corner_softness as cs, diffusion as df, dirt, glow,
            halation, smear, star, water_drops as wd,
        };
        let r = row_of(LensElementKind::SphericalGlow, |p| {
            p.glow_amount = 0.2;
            p.glow_reference_f_number = 2.0;
            p.glow_exponent = 3.0;
            p.glow_radius = 0.02;
            p.glow_tail = 0.4;
        });
        assert_eq!(
            [
                r.params[glow::AMOUNT],
                r.params[glow::REFERENCE_F_NUMBER],
                r.params[glow::EXPONENT],
                r.params[glow::RADIUS],
                r.params[glow::TAIL]
            ],
            [0.2, 2.0, 3.0, 0.02, 0.4]
        );
        let r = row_of(LensElementKind::Coma, |p| {
            (p.coma_amount, p.coma_length, p.coma_exponent, p.coma_shape) = (0.5, 0.1, 2.0, 3.0);
        });
        assert_eq!(
            [
                r.params[coma::AMOUNT],
                r.params[coma::LENGTH],
                r.params[coma::EXPONENT],
                r.params[coma::SHAPE]
            ],
            [0.5, 0.1, 2.0, 3.0]
        );
        let r = row_of(LensElementKind::Astigmatism, |p| {
            (
                p.astigmatism_amount,
                p.astigmatism_exponent,
                p.astigmatism_minor_ratio,
            ) = (-0.02, 3.0, 0.3);
        });
        assert_eq!(
            [
                r.params[astig::AMOUNT],
                r.params[astig::EXPONENT],
                r.params[astig::MINOR_RATIO]
            ],
            [-0.02, 3.0, 0.3]
        );
        let r = row_of(LensElementKind::CornerSoftness, |p| {
            (
                p.corner_softness_sigma,
                p.corner_softness_exponent,
                p.corner_softness_start,
            ) = (0.03, 1.5, 0.5);
        });
        assert_eq!(
            [
                r.params[cs::SIGMA],
                r.params[cs::EXPONENT],
                r.params[cs::START]
            ],
            [0.03, 1.5, 0.5]
        );

        for kind in [
            LensElementKind::Diffusion,
            LensElementKind::ProMist,
            LensElementKind::BlackProMist,
            LensElementKind::Glimmerglass,
            LensElementKind::Fog,
        ] {
            let r = row_of(kind, |p| {
                (
                    p.diffusion_strength,
                    p.diffusion_radius,
                    p.diffusion_tail,
                    p.glint_contrast,
                ) = (0.3, 0.04, 0.6, 0.9);
            });
            assert_eq!(
                [
                    r.params[df::STRENGTH],
                    r.params[df::RADIUS],
                    r.params[df::TAIL]
                ],
                [0.3, 0.04, 0.6],
                "{kind:?}"
            );
            let glint = if kind == LensElementKind::Glimmerglass {
                0.9
            } else {
                0.0
            };
            assert_eq!(
                r.params[df::GLINT],
                glint,
                "{kind:?}: glint only on Glimmerglass"
            );
        }

        let r = row_of(LensElementKind::StarFilter, |p| {
            p.star_points = 6;
            p.star_rotation_deg = 180.0;
            (p.star_length, p.star_intensity, p.star_falloff) = (0.3, 0.25, 2.0);
        });
        assert_eq!((r.params[star::POINTS], r.params[star::LENGTH]), (6.0, 0.3));
        assert!((r.params[star::ROTATION] - std::f32::consts::PI).abs() < 1e-6);
        assert_eq!(
            (r.params[star::INTENSITY], r.params[star::FALLOFF]),
            (0.25, 2.0)
        );
        let r = row_of(LensElementKind::Halation, |p| {
            (p.halation_strength, p.halation_radius, p.halation_tail) = (0.5, 0.03, 0.7);
            (p.halation_tint_r, p.halation_tint_g, p.halation_tint_b) = (1.0, 0.2, 0.1);
        });
        assert_eq!(
            (
                r.params[halation::STRENGTH],
                r.params[halation::RADIUS],
                r.params[halation::TAIL]
            ),
            (0.5, 0.03, 0.7)
        );
        assert_eq!(
            &r.params[halation::TINT..halation::TINT + 3],
            &[1.0, 0.2, 0.1]
        );

        let r = row_of(LensElementKind::WaterDrops, |p| {
            (p.drops_refraction, p.drops_normal_layer, p.drops_defocus) = (0.04, 2, 0.006);
        });
        assert_eq!(
            (
                r.params[wd::REFRACTION],
                r.params[wd::NORMAL_LAYER],
                r.params[wd::DEFOCUS]
            ),
            (0.04, 2.0, 0.006)
        );
        let r = row_of(LensElementKind::Smear, |p| {
            (p.smear_strength, p.smear_length, p.smear_angle_deg) = (0.4, 0.05, 90.0);
        });
        assert_eq!(
            (r.params[smear::STRENGTH], r.params[smear::LENGTH]),
            (0.4, 0.05)
        );
        assert!((r.params[smear::ANGLE] - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        let r = row_of(LensElementKind::Dirt, |p| {
            (p.dirt_strength, p.dirt_radius, p.dirt_loss, p.dirt_tail) = (0.35, 0.04, 0.2, 0.6);
        });
        assert_eq!(
            [
                r.params[dirt::STRENGTH],
                r.params[dirt::RADIUS],
                r.params[dirt::LOSS],
                r.params[dirt::TAIL]
            ],
            [0.35, 0.04, 0.2, 0.6]
        );
    }

    #[test]
    fn the_mask_region_and_aperture_texture_reach_their_slots() {
        use helio_pass_lens::{mask, pupil};
        let mut props = LensElementProps {
            kind: LensElementKind::Dirt,
            shape: LensRegionShape::Mask,
            mask_layer: 3,
            mask_channel: 1,
            mask_invert: true,
            mask_scale: 2.0,
            mask_offset_u: 0.25,
            mask_offset_v: -0.5,
            mask_tile: true,
            auto_feather: false,
            ..LensElementProps::default()
        };
        let mut lens = CameraLensComponent::default();
        lens.elements = vec![props.clone()];
        let e = lens.to_rows(0).1[0];
        assert_eq!(e.region_kind, region_kind::MASK);
        let m = &e.region;
        assert_eq!(
            (m[mask::LAYER], m[mask::CHANNEL], m[mask::INVERT]),
            (3.0, 1.0, 1.0)
        );
        assert_eq!(
            (
                m[mask::SCALE],
                m[mask::OFFSET],
                m[mask::OFFSET + 1],
                m[mask::TILE]
            ),
            (2.0, 0.25, -0.5, 1.0)
        );
        // A channel past alpha is clamped; a non-mask region leaves the slots alone.
        props.mask_channel = 9;
        lens.elements = vec![props.clone()];
        assert_eq!(lens.to_rows(0).1[0].region[mask::CHANNEL], 3.0);
        props.shape = LensRegionShape::FullFrame;
        lens.elements = vec![props];
        assert_eq!(lens.to_rows(0).1[0].region_kind, region_kind::FULL_FRAME);

        let r = row_of(LensElementKind::Pupil, |p| {
            p.aperture_texture_layer = 4;
            p.aperture_texture_amount = 0.75;
        });
        assert_eq!(
            (
                r.params[pupil::TEXTURE_LAYER],
                r.params[pupil::TEXTURE_AMOUNT]
            ),
            (4.0, 0.75)
        );
    }

    #[test]
    fn ultra_quality_selects_sixteen_spectral_samples() {
        let mut lens = CameraLensComponent::default();
        for (quality, value) in [
            (LensGatherQuality::Economical, 0),
            (LensGatherQuality::High, 1),
            (LensGatherQuality::Ultra, 2),
        ] {
            lens.quality = quality;
            assert_eq!(lens.to_rows(0).0.quality, value);
        }
    }

    #[test]
    fn new_presets_describe_the_features_they_are_named_for() {
        let rows = |preset: LensPreset| {
            let mut lens = CameraLensComponent::default();
            lens.preset = preset;
            lens.to_rows(0)
        };
        let kinds = |preset: LensPreset| -> Vec<u32> {
            let (row, e) = rows(preset);
            e[..row.element_count as usize]
                .iter()
                .map(|e| e.kind)
                .collect()
        };
        use element_kind as k;
        assert_eq!(
            kinds(LensPreset::Anamorphic2x),
            [k::PUPIL, k::ANAMORPHIC_DISTORTION, k::ANAMORPHIC_SQUEEZE]
        );
        let (_, e) = rows(LensPreset::Anamorphic2x);
        assert_eq!(e[0].params[helio_pass_lens::pupil::SQUEEZE], 2.0);
        assert_eq!(e[2].params[0], 2.0);
        assert_eq!(kinds(LensPreset::Fisheye8mm)[0], k::PROJECTION);
        assert_eq!(rows(LensPreset::Fisheye8mm).0.focal_length_mm, 8.0);
        assert_eq!(kinds(LensPreset::UnderwaterFlatPort)[0], k::FLAT_PORT);
        assert_eq!(kinds(LensPreset::ProMistQuarter), [k::PRO_MIST]);
        assert_eq!(rows(LensPreset::ProMistQuarter).1[0].params[0], 0.25);
        assert_eq!(kinds(LensPreset::BlackProMistEighth), [k::BLACK_PRO_MIST]);
        assert_eq!(rows(LensPreset::BlackProMistEighth).1[0].params[0], 0.125);
        let (_, rain) = rows(LensPreset::RainyLens);
        assert_eq!(
            (rain[0].kind, rain[0].region_kind),
            (k::WATER_DROPS, region_kind::MASK)
        );
        assert_eq!(
            rows(LensPreset::StarFilter6pt).1[0].params[helio_pass_lens::star::POINTS],
            6.0
        );
        assert_eq!(kinds(LensPreset::FilmHalation), [k::HALATION]);
        let (grad, e) = rows(LensPreset::GradNd3Stop);
        assert_eq!((e[0].kind, e[0].params[0]), (k::GRADUATED_ND, 3.0));
        assert_eq!(grad.element_count, 1);
        // Baking a new preset copies the tuned elements into the fields.
        let mut lens = CameraLensComponent::default();
        assert!(lens.bake_preset(LensPreset::Anamorphic2x));
        assert_eq!(lens.elements.len(), 3);
        assert_eq!(lens.elements[0].squeeze, 2.0);
    }

    #[test]
    fn every_kind_and_shape_round_trips_through_json_by_name_and_index() {
        for (i, kind) in LensElementKind::ALL.iter().enumerate() {
            let props = LensElementProps {
                kind: *kind,
                ..Default::default()
            };
            let back: LensElementProps =
                serde_json::from_value(serde_json::to_value(&props).unwrap()).unwrap();
            assert_eq!(back.kind, *kind);
            let by_index: LensElementKind = serde_json::from_value(serde_json::json!(i)).unwrap();
            assert_eq!(by_index, *kind);
        }
        // The first four indices are the ones saved before the new kinds existed.
        assert_eq!(LensElementKind::ALL[3], LensElementKind::LongitudinalCa);
        assert_eq!(LensRegionShape::ALL[4], LensRegionShape::RadialBand);
        assert_eq!(LensRegionShape::ALL[5], LensRegionShape::Mask);
    }

    #[test]
    fn a_level_saved_before_the_new_kinds_still_loads_and_renders_the_same() {
        let old = serde_json::json!({
            "preset": "Custom",
            "quality": 1,
            "f_number": 4.0,
            "elements": [
                { "kind": "Diopter", "focus_m": 1.5, "shape": "HalfPlane", "angle_deg": 90.0 },
                { "kind": 2, "squeeze": 1.8, "shape": 0, "blades": 9, "inherit_iris": false },
                { "kind": "SweetSpot", "shape": "Ellipse", "extra_blur_px": 20.0 },
                { "kind": "LongitudinalCa", "red_shift_dpt": -0.1 }
            ]
        });
        let lens: CameraLensComponent = serde_json::from_value(old).unwrap();
        assert_eq!(lens.quality, LensGatherQuality::High);
        assert_eq!(lens.elements.len(), 4);
        let (row, e) = lens.to_rows(0);
        assert_eq!(row.element_count, 4);
        assert_eq!(e[0].kind, element_kind::DIOPTER);
        assert_eq!(e[0].params[0], 1.5);
        assert_eq!(e[1].kind, element_kind::PUPIL);
        assert_eq!(e[1].params[helio_pass_lens::pupil::SQUEEZE], 1.8);
        assert_eq!(e[1].params[helio_pass_lens::pupil::TEXTURE_AMOUNT], 0.0);
        assert_eq!(e[2].kind, element_kind::SWEET_SPOT);
        assert_eq!(e[3].params[0], -0.1);
        // New fields took their defaults, so the element equals one built fresh.
        assert_eq!(lens.elements[0].mask_scale, 1.0);
        assert_eq!(
            lens.elements[0].diffusion_strength,
            LensElementProps::default().diffusion_strength
        );
    }

    #[test]
    fn the_new_fields_are_reflected_with_categories() {
        let props = LensElementProps::default();
        let fields = props.get_properties();
        let find = |name: &str| {
            fields
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("{name} reflected"))
        };
        for name in [
            "projection_model",
            "bc_k1",
            "mumps_kx1",
            "prism_count",
            "port_index",
            "grad_nd_stops",
            "glow_amount",
            "diffusion_strength",
            "star_points",
            "halation_strength",
            "drops_refraction",
            "dirt_loss",
            "mask_layer",
            "aperture_texture_amount",
        ] {
            find(name);
        }
        assert_eq!(find("bc_k1").category, Some("Distortion"));
        assert_eq!(find("diffusion_strength").category, Some("Diffusion"));
    }

    #[test]
    fn a_prescription_names_its_bundled_lens_layer_and_scales_its_point_spread() {
        // The editor's choices are the bank's layers, in order.
        assert_eq!(LensPrescriptionChoice::ALL.len(), BundledLens::ALL.len());
        for (choice, lens) in LensPrescriptionChoice::ALL.iter().zip(BundledLens::ALL) {
            assert_eq!(choice.bundled(), lens);
            assert_eq!(format!("{choice:?}"), format!("{lens:?}"));
            let row = row_of(LensElementKind::Prescription, |p| {
                p.prescription_lens = *choice
            });
            assert_eq!(row.kind, element_kind::PRESCRIPTION);
            assert_eq!(
                row.params[helio_pass_lens::prescription::LAYER],
                lens.layer() as f32
            );
        }
        let row = row_of(LensElementKind::Prescription, |p| p.psf_amount = 0.4);
        assert_eq!(row.params[helio_pass_lens::prescription::PSF_AMOUNT], 0.4);
        // The default is the double Gauss at full strength, as the preset.
        let row = row_of(LensElementKind::Prescription, |_| {});
        assert_eq!(row.params[helio_pass_lens::prescription::LAYER], 2.0);
        assert_eq!(row.params[helio_pass_lens::prescription::PSF_AMOUNT], 1.0);
        // Over a region, like every element.
        let row = row_of(LensElementKind::Prescription, |p| {
            p.shape = LensRegionShape::Ellipse
        });
        assert_eq!(row.region_kind, region_kind::ELLIPSE);
    }

    #[test]
    fn a_user_element_carries_its_hooks_and_all_sixteen_parameters() {
        let row = row_of(LensElementKind::User, |_| {});
        assert_eq!(
            (row.kind, row.flags),
            (element_kind::USER, 0),
            "no hook selected is inert"
        );
        let row = row_of(LensElementKind::User, |p| {
            p.user_map = true;
            p.user_focus = true;
            p.user_p0 = 1.5;
            p.user_p7 = -2.0;
            p.user_p15 = 9.0;
        });
        assert_eq!(row.flags, user::MAP | user::FOCUS);
        assert_eq!(
            (row.params[0], row.params[7], row.params[15]),
            (1.5, -2.0, 9.0)
        );
        assert_eq!(row.params[1], 0.0);
        let row = row_of(LensElementKind::User, |p| {
            (
                p.user_map,
                p.user_transmission,
                p.user_focus,
                p.user_spectral,
            ) = (true, true, true, true)
        });
        assert_eq!(
            row.flags,
            user::MAP | user::TRANSMISSION | user::FOCUS | user::SPECTRAL
        );
    }

    #[test]
    fn the_traced_preset_is_a_full_frame_prescription_of_the_double_gauss() {
        let mut lens = CameraLensComponent::default();
        lens.preset = LensPreset::DoubleGauss50mmF2Traced;
        let (row, elements) = lens.to_rows(0);
        assert_eq!(
            (row.focal_length_mm, row.f_number, row.element_count),
            (50.0, 2.0, 1)
        );
        assert_eq!(elements[0].kind, element_kind::PRESCRIPTION);
        assert_eq!(elements[0].region_kind, region_kind::FULL_FRAME);
        assert_eq!(
            elements[0].params[helio_pass_lens::prescription::LAYER],
            2.0
        );
    }

    #[test]
    fn a_lens_that_maps_past_the_frame_asks_for_overscan_by_its_rows() {
        let overscan = |preset: LensPreset, aspect: f32| {
            let mut lens = CameraLensComponent::default();
            lens.preset = preset;
            let (row, elements) = lens.to_rows(0);
            overscan_of(&row, &elements, aspect)
        };
        assert_eq!(overscan(LensPreset::Standard50mmF2_8, 16.0 / 9.0), 1.0);
        assert_eq!(overscan(LensPreset::ProMistQuarter, 16.0 / 9.0), 1.0);
        // An 8 mm equidistant fisheye reaches far past any frame: capped.
        assert_eq!(
            overscan(LensPreset::Fisheye8mm, 1.5),
            helio_pass_lens::ray_map::APPLIED_OVERSCAN_CAP
        );
        // The traced double Gauss has barrel distortion at the corners, and
        // the reach follows the aspect (the sensor width spans the image, so
        // a wider image has a narrower view and its corners angle less).
        let square = overscan(LensPreset::DoubleGauss50mmF2Traced, 1.0);
        let wide = overscan(LensPreset::DoubleGauss50mmF2Traced, 16.0 / 9.0);
        assert!(square > 1.0 && wide > 1.0, "{square} {wide}");
        assert!(
            wide < square,
            "a wider image has a narrower view: {wide} vs {square}"
        );
        // A disabled lens asks for none.
        let mut lens = CameraLensComponent::default();
        lens.preset = LensPreset::Fisheye8mm;
        lens.enabled = false;
        let (row, elements) = lens.to_rows(0);
        assert_eq!(overscan_of(&row, &elements, 1.5), 1.0);
    }

    #[test]
    fn element_blocks_are_compact_stable_and_reused() {
        let mut blocks = ElementBlocks::default();
        let a = blocks.block_of(5000);
        let b = blocks.block_of(12);
        assert_eq!((a, b), (0, 1), "compact, whatever the entity indices");
        assert_eq!(blocks.block_of(5000), a, "stable");
        assert_eq!(blocks.release(5000), Some(a));
        assert_eq!(blocks.release(5000), None);
        assert_eq!(blocks.block_of(77), a, "a freed block is reused");
    }

    /// Nudge one reflected scalar field; false for a field that isn't one.
    fn nudge(props: &mut LensElementProps, prop: &pulsar_reflection::PropertyMetadata) -> bool {
        use pulsar_reflection::EngineClass as _;
        let current = (prop.getter)(&*props);
        let new: Box<dyn std::any::Any + Send> = if let Some(v) = current.downcast_ref::<f32>() {
            Box::new(v + 0.0371)
        } else if let Some(v) = current.downcast_ref::<u32>() {
            Box::new(v + 1)
        } else if let Some(v) = current.downcast_ref::<bool>() {
            Box::new(!v)
        } else {
            return false;
        };
        let _ = props.get_properties();
        (prop.setter)(props, new);
        true
    }

    #[test]
    fn the_inspector_shows_exactly_the_fields_a_row_reads() {
        use pulsar_reflection::EngineClass as _;
        let props_meta = LensElementProps::default().get_properties();
        let mut checked = 0;
        for &kind in LensElementKind::ALL {
            let models: &[LensProjectionModel] = if kind == LensElementKind::Projection {
                LensProjectionModel::ALL
            } else {
                &[LensProjectionModel::Rectilinear]
            };
            for &shape in LensRegionShape::ALL {
                for &model in models {
                    for switches in 0..8u32 {
                        let base = LensElementProps {
                            kind,
                            shape,
                            projection_model: model,
                            inherit_iris: switches & 1 != 0,
                            hood_rectangular: switches & 2 != 0,
                            auto_feather: switches & 4 != 0,
                            polygon_vertices: 4,
                            ..LensElementProps::default()
                        };
                        let row = bytemuck::bytes_of(&base.to_row()).to_vec();
                        for prop in &props_meta {
                            let mut edited = base.clone();
                            let is_f32 = (prop.getter)(&base).downcast_ref::<f32>().is_some();
                            if !nudge(&mut edited, prop) {
                                continue;
                            }
                            let changed = bytemuck::bytes_of(&edited.to_row()) != row.as_slice();
                            if !base.reads(prop.name) {
                                assert!(
                                    !changed,
                                    "{kind:?}/{shape:?}/{model:?}: hidden `{}` changes the row",
                                    prop.name
                                );
                            } else if is_f32 {
                                assert!(
                                    changed,
                                    "{kind:?}/{shape:?}/{model:?}: shown `{}` is never read",
                                    prop.name
                                );
                            }
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert!(checked > 10_000, "{checked}");
    }

    #[test]
    fn element_lists_have_a_registered_editor() {
        let id = std::any::TypeId::of::<Vec<LensElementProps>>();
        assert!(
            pulsar_reflection::inventory::iter::<pulsar_reflection::UiPropertyEditorHint>
                .into_iter()
                .any(|hint| hint.type_id == id)
        );
    }
}
