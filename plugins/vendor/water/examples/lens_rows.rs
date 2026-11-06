//! The lens plugin's rows as a standalone host authors them (a camera lens and
//! its elements), as the lens examples declare them.
#![allow(dead_code)]

use pulsar_scenedb_derive::SceneStore;

/// Ties a component to its contract row: same size, every field at the same
/// offset, stored under the contract's key. Adds the conversions. (As
/// `helio_components::same_layout!`, for rows Helio no longer has.)
macro_rules! same_layout {
    ($component:ident, $row:path, $contract:path, $key:literal; $($field:ident),* $(,)?) => {
        const _: () = {
            assert!(::core::mem::size_of::<$component>() == ::core::mem::size_of::<$row>());
            assert!(::core::mem::align_of::<$component>() == ::core::mem::align_of::<$row>());
            $(
                assert!(
                    ::core::mem::offset_of!($component, $field)
                        == ::core::mem::offset_of!($row, $field)
                );
            )*
            $contract.assert_producer::<$component>($key);
        };

        impl ::core::convert::From<$row> for $component {
            fn from(row: $row) -> Self {
                ::bytemuck::cast(row)
            }
        }

        impl ::core::convert::From<$component> for $row {
            fn from(component: $component) -> Self {
                ::bytemuck::cast(component)
            }
        }

        impl $component {
            /// This component as its contract row.
            pub fn row(&self) -> $row {
                ::bytemuck::cast(*self)
            }
        }

        impl ::core::ops::Deref for $component {
            type Target = $row;
            fn deref(&self) -> &$row {
                ::bytemuck::cast_ref(self)
            }
        }

        impl ::core::ops::DerefMut for $component {
            fn deref_mut(&mut self) -> &mut $row {
                ::bytemuck::cast_mut(self)
            }
        }
    };
}

/// A camera's lens. One row per view; see the module doc.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "camera_lenses")]
pub struct LensComponent {
    #[gpu]
    pub view_id: u32,
    #[gpu]
    pub enabled: u32,
    /// [`FOCUS_MANUAL`] or [`FOCUS_AUTO_POINT`].
    #[gpu]
    pub focus_mode: u32,
    /// 0 economical, 1 high (more gather taps), 2 ultra. Also the number of
    /// wavelength samples of lateral chromatic aberration: 3, 7, 16.
    #[gpu]
    pub quality: u32,
    #[gpu]
    pub focal_length_mm: f32,
    #[gpu]
    pub f_number: f32,
    /// Transmission stop; the light lost against the f-number is applied by
    /// a [`element_kind::T_STOP_LOSS`] element.
    #[gpu]
    pub t_stop: f32,
    #[gpu]
    pub sensor_width_mm: f32,
    /// Zero derives it from the width at 3:2.
    #[gpu]
    pub sensor_height_mm: f32,
    #[gpu]
    pub focus_distance_m: f32,
    /// Autofocus point in uv (y down).
    #[gpu]
    pub af_u: f32,
    #[gpu]
    pub af_v: f32,
    /// Autofocus ease rate in 1/s (<= 0: instant).
    #[gpu]
    pub af_speed: f32,
    /// Largest circle of confusion diameter in pixels (<= 0: 32).
    #[gpu]
    pub max_coc_px: f32,
    /// First row of this lens's elements in the `lens_elements` buffer.
    #[gpu]
    pub element_first: u32,
    #[gpu]
    pub element_count: u32,
    /// Scheimpflug: focal plane tilt in radians about an axis in the image
    /// plane at `tilt_axis` radians from +x.
    #[gpu]
    pub tilt_angle: f32,
    #[gpu]
    pub tilt_axis: f32,
    /// Petzval field curvature in dioptres at the frame corner; the focus
    /// shifts by `field_curvature * r^2`.
    #[gpu]
    pub field_curvature: f32,
    /// Iris blade count (0 = circular); shapes the flare pupil.
    #[gpu]
    pub aperture_blades: u32,
    /// Iris rotation in radians.
    #[gpu]
    pub aperture_rotation: f32,
    /// Autofocus sampling radius in uv (<= 0: 0.02).
    #[gpu]
    pub af_radius: f32,
    /// The overscan factor (>= 1; 0 reads as 1) the camera rendering this view
    /// widened its field of view by: the image the pass is given shows
    /// `overscan` times the frame's tangent extent about the frame centre, and
    /// the pass maps it back to the frame. Author it with
    /// `ray_map::applied_overscan` from the lens and elements the
    /// camera widened by; see the README.
    #[gpu]
    pub overscan: f32,
    #[gpu]
    pub _pad1: f32,
}
same_layout!(LensComponent, helio_pass_lens::LensRow, helio_pass_lens::CAMERA_LENSES, "camera_lenses"; view_id, enabled, focus_mode, quality, focal_length_mm, f_number, t_stop, sensor_width_mm, sensor_height_mm, focus_distance_m, af_u, af_v, af_speed, max_coc_px, element_first, element_count, tilt_angle, tilt_axis, field_curvature, aperture_blades, aperture_rotation, af_radius, overscan, _pad1);

/// One optical element in front of the sensor. Rows are indexed from
/// [`LensComponent::element_first`]; kind [`element_kind::NONE`] rows are
/// skipped, so a gap in the buffer is inert.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "lens_elements")]
pub struct LensElementComponent {
    /// [`element_kind`].
    #[gpu]
    pub kind: u32,
    /// [`region_kind`].
    #[gpu]
    pub region_kind: u32,
    /// Polygon vertex count.
    #[gpu]
    pub region_count: u32,
    #[gpu]
    pub flags: u32,
    /// Width of the region's soft edge in image-height units. Negative:
    /// derived from the aperture (see [`element_kind::DIOPTER`]).
    #[gpu]
    pub feather: f32,
    #[gpu]
    pub _pad0: f32,
    #[gpu]
    pub _pad1: f32,
    #[gpu]
    pub _pad2: f32,
    /// Kind-specific parameters.
    #[gpu]
    pub params: [f32; 16],
    /// Region-kind-specific geometry.
    #[gpu]
    pub region: [f32; 16],
}
same_layout!(LensElementComponent, helio_pass_lens::LensElementRow, helio_pass_lens::LENS_ELEMENTS, "lens_elements"; kind, region_kind, region_count, flags, feather, _pad0, _pad1, _pad2, params, region);
