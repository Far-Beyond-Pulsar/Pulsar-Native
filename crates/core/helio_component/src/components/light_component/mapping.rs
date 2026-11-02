use helio::GpuLight;
use serde_json::Value;

use super::LightComponent;

impl super::LightComponentGpuMirror {
    /// Translate this `#[gpu]`-mirrored companion into Helio's GPU light
    /// representation -- single source of truth for the `LightComponent` ->
    /// `GpuLight` mapping (Pulsar-Native#561).
    ///
    /// Every field this reads (`general.light_type`, `intensity.intensity`,
    /// `color.color`, `attenuation.{range,inner_cone_angle,outer_cone_
    /// angle}`, `shadows.cast_shadows`) is ALREADY in `GpuLight`'s own units
    /// and value space by the time it lands here -- degrees->cosines and
    /// the `LightType`->`u32`/`bool`->shadow-request-sentinel remaps happen
    /// once, upload-time, via each field's own `#[gpu(as = .., with = ..)]`
    /// (`sub_props/attenuation.rs`/`general.rs`/`shadows.rs`), not in this
    /// function. What's left here is ONLY reshaping already-transformed
    /// values into `GpuLight`'s packed-vec4 field grouping (a GPU memory-
    /// layout choice, unrelated to units or semantics) plus the one piece
    /// that structurally cannot happen any earlier:
    ///
    /// `position_range`'s xyz is always `[0.0; 0.0; 0.0]` here -- `GpuLight`
    /// bakes in world-space position, which lives on a COMPLETELY SEPARATE
    /// component (`Transform`, updated every frame the object moves, on no
    /// schedule related to this component's own property edits) and simply
    /// doesn't exist yet at the moment THIS mirror gets built. The
    /// renderer's scene join combines the two on the GPU, reading the
    /// derived [`super::LightSourceRow`] (this mapping's output) and the
    /// owner object's transform row.
    ///
    /// Translates whatever row it is given; a disabled light's row carries
    /// `general.enabled == 0`, and consumers skip it.
    pub fn to_helio_gpu_light(&self) -> GpuLight {
        GpuLight {
            position_range: [0.0, 0.0, 0.0, self.attenuation.range.0],
            direction_outer: [0.0, -1.0, 0.0, self.attenuation.outer_cone_angle.0],
            color_intensity: [
                self.color.color.0[0],
                self.color.color.0[1],
                self.color.color.0[2],
                physical_intensity(
                    self.intensity.intensity.0,
                    self.intensity.intensity_units.0,
                    self.general.light_type.0,
                    self.attenuation.outer_cone_angle.0,
                ),
            ],
            shadow_index: self.shadows.cast_shadows.0,
            light_type: self.general.light_type.0,
            inner_angle: self.attenuation.inner_cone_angle.0,
            _pad: GpuLight::shadow_policy_bits(
                self.shadows.shadow_priority.0,
                self.shadows.shadow_max_resolution.0,
                self.shadows.cast_static_shadows.0 != 0,
                self.shadows.cast_dynamic_shadows.0 != 0,
                self.shadows.cast_contact_shadows.0 != 0,
            ),
            god_rays_enabled: self.volumetrics.affects_volumetric_fog.0,
            // Density belongs to the medium. Preserve the legacy inscattering
            // gain by multiplying it into the new per-light scattering gain.
            god_rays_density: 1.0,
            god_rays_weight: nonnegative(self.volumetrics.volumetric_scattering_intensity.0)
                * nonnegative(self.volumetrics.fog_inscattering_intensity.0),
            // Agreed fog-pass contract: geometric visibility strength, no
            // longer a per-step decay. Absorption is still medium-owned.
            god_rays_decay: self.shadows.cast_volumetric_shadow.0,
            god_rays_exposure: 1.0,
            ..Default::default()
        }
    }
}

fn nonnegative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

/// Point/spot shaders consume candela; directional shaders consume lux.
/// Lumens use an isotropic sphere for points and a uniform outer cone for
/// spots: cd = lm / (2*pi*(1-cos(theta))). The shader's inner-cone falloff is
/// artistic shaping after that normalization, so it does not preserve flux.
/// Directional values are always interpreted as lux, including legacy scenes
/// whose unit selector remained at its default (lumens). Unitless/candela are
/// passed through; local lux/nits remain legacy numeric values because a
/// distance/emitter area would be needed to convert those physically.
fn physical_intensity(value: f32, units: u32, kind: u32, outer_cos: f32) -> f32 {
    let value = nonnegative(value);
    if units != super::IntensityUnits::Lumens as u32 || kind == helio::LightType::Directional as u32
    {
        return value;
    }
    let solid_angle = if kind == helio::LightType::Spot as u32 {
        // Bound a degenerate cone so imported zero-angle lights stay finite.
        2.0 * std::f32::consts::PI * (1.0 - outer_cos.clamp(-1.0, 1.0)).max(1e-6)
    } else {
        4.0 * std::f32::consts::PI
    };
    (value / solid_angle).min(f32::MAX)
}

impl LightComponent {
    pub fn from_component_data(data: &Value) -> Self {
        let mut light = Self::default();
        if let Some(obj) = data.as_object() {
            light.general.apply_from_component_data(obj);
            light.intensity.apply_from_component_data(obj);
            light.color.apply_from_component_data(obj);
            light.attenuation.apply_from_component_data(obj);
            light.shadows.apply_from_component_data(obj);
            light.volumetrics.apply_from_component_data(obj);
            light.light_function.apply_from_component_data(obj);
            light.performance.apply_from_component_data(obj);
            light.advanced.apply_from_component_data(obj);
        }
        light
    }
}

#[cfg(test)]
mod tests {
    use super::super::LightType;
    use super::*;
    use helio::LightType as HelioLightType;
    use pulsar_world_registry::GpuMirrored;

    // NOTE: "a disabled light produces no GpuLight" is not this function's
    // responsibility -- `to_helio_gpu_light` translates whatever row it is
    // given, and the row's `general.enabled` tells consumers to skip it.

    #[test]
    fn mirror_carries_color_and_intensity_with_a_zeroed_position_placeholder() {
        let mut light = LightComponent::default();
        light.color.color = [0.25, 0.5, 0.75, 1.0];
        light.intensity.intensity = 42.0;
        light.attenuation.range = 10.0;

        let gpu = light.to_gpu_mirror().to_helio_gpu_light();

        // xyz is always the zeroed placeholder here: the scene join fills it
        // from the owner's transform on the GPU (see this fn's own doc).
        assert_eq!(gpu.position_range, [0.0, 0.0, 0.0, 10.0]);
        // The default unit is lumens; a point light spreads them over the
        // full sphere, so the shader gets candela.
        let candela = 42.0 / (4.0 * std::f32::consts::PI);
        assert_eq!(&gpu.color_intensity[..3], &[0.25, 0.5, 0.75]);
        assert!((gpu.color_intensity[3] - candela).abs() < 1e-5);

        light.intensity.intensity_units = super::super::IntensityUnits::Candelas;
        let gpu = light.to_gpu_mirror().to_helio_gpu_light();
        assert_eq!(
            gpu.color_intensity,
            [0.25, 0.5, 0.75, 42.0],
            "candela pass through"
        );
    }

    #[test]
    fn light_type_maps_onto_the_matching_helio_discriminant() {
        let cases = [
            (LightType::Directional, HelioLightType::Directional),
            (LightType::Point, HelioLightType::Point),
            (LightType::Spot, HelioLightType::Spot),
            // helio has no Area light type; Area falls back to Point.
            (LightType::Area, HelioLightType::Point),
        ];
        for (editor_type, expected) in cases {
            let mut light = LightComponent::default();
            light.general.light_type = editor_type;
            let gpu = light.to_gpu_mirror().to_helio_gpu_light();
            assert_eq!(
                gpu.light_type, expected as u32,
                "{editor_type:?} should map to {expected:?}"
            );
        }
    }

    #[test]
    fn cast_shadows_toggle_maps_to_the_shadow_index_sentinel() {
        let mut light = LightComponent::default();

        light.shadows.cast_shadows = true;
        assert_eq!(light.to_gpu_mirror().to_helio_gpu_light().shadow_index, 0);

        light.shadows.cast_shadows = false;
        assert_eq!(
            light.to_gpu_mirror().to_helio_gpu_light().shadow_index,
            u32::MAX
        );
    }

    #[test]
    fn spot_cone_angles_land_on_gpulight_as_cosines_with_inner_inside_outer() {
        // #172: these two fields were uploaded as raw radians while every
        // lighting shader treats them as cosines -- `smoothstep(outer_cos,
        // inner_cos, dot(-L, dir))` ran with edge0 > edge1, a REVERSED
        // smoothstep: black at the cone's center, full brightness only past
        // ~58 degrees off-axis. The mirror must hand over cosines, with
        // inner >= outer so brightness saturates INSIDE the cone.
        let mut light = LightComponent::default();
        light.general.light_type = LightType::Spot;
        light.attenuation.inner_cone_angle = 30.0;
        light.attenuation.outer_cone_angle = 45.0;

        let gpu = light.to_gpu_mirror().to_helio_gpu_light();

        let expected_inner = 30.0_f32.to_radians().cos();
        let expected_outer = 45.0_f32.to_radians().cos();
        assert!(
            (gpu.inner_angle - expected_inner).abs() < 1e-6,
            "inner_angle must be cos(30deg) = {expected_inner}, got {}",
            gpu.inner_angle
        );
        assert!(
            (gpu.direction_outer[3] - expected_outer).abs() < 1e-6,
            "direction_outer.w must be cos(45deg) = {expected_outer}, got {}",
            gpu.direction_outer[3]
        );
        // The property every shader actually depends on (saturated inside
        // the inner cone, falling off to the outer cone): radians-as-cosines
        // inverted this ordering, which WAS the #172 artifact.
        assert!(gpu.inner_angle > gpu.direction_outer[3]);
    }
}
