//! Light conversion from SolidRS to Helio GPU light data.

use helio::GpuLight;
use helio::LightType;
use solid_rs::scene::{AreaLight, DirectionalLight, Light, PointLight, SpotLight};

/// Convert a SolidRS light to Helio's GPU light format.
pub fn convert_light(light: &Light) -> Option<GpuLight> {
    match light {
        Light::Directional(dir_light) => Some(convert_directional(dir_light)),
        Light::Point(point_light) => Some(convert_point(point_light)),
        Light::Spot(spot_light) => Some(convert_spot(spot_light)),
        Light::Area(area_light) => {
            log::warn!(
                "Area light '{}' not supported in Helio yet - converting to point light",
                area_light.base.name
            );
            Some(convert_area_as_point(area_light))
        }
    }
}

fn convert_directional(light: &DirectionalLight) -> GpuLight {
    GpuLight {
        position_range: [0.0, 0.0, 0.0, f32::MAX],
        direction_outer: [0.0, -1.0, 0.0, 0.0],
        color_intensity: [
            light.base.color.x,
            light.base.color.y,
            light.base.color.z,
            light.base.intensity,
        ],
        shadow_index: u32::MAX,
        light_type: LightType::Directional as u32,
        inner_angle: 0.0,
        _pad: 0,
        ..Default::default()
    }
}

fn convert_point(light: &PointLight) -> GpuLight {
    let range = light.range.unwrap_or(10.0);
    GpuLight {
        position_range: [0.0, 0.0, 0.0, range],
        direction_outer: [0.0, 0.0, -1.0, 0.0],
        color_intensity: [
            light.base.color.x,
            light.base.color.y,
            light.base.color.z,
            light.base.intensity,
        ],
        shadow_index: u32::MAX,
        light_type: LightType::Point as u32,
        inner_angle: 0.0,
        _pad: 0,
        ..Default::default()
    }
}

fn convert_spot(light: &SpotLight) -> GpuLight {
    let range = light.range.unwrap_or(10.0);
    let inner_angle = light.inner_cone_angle / 2.0;
    let outer_angle = light.outer_cone_angle / 2.0;

    GpuLight {
        position_range: [0.0, 0.0, 0.0, range],
        direction_outer: [0.0, 0.0, -1.0, outer_angle.cos()],
        color_intensity: [
            light.base.color.x,
            light.base.color.y,
            light.base.color.z,
            light.base.intensity,
        ],
        shadow_index: u32::MAX,
        light_type: LightType::Spot as u32,
        inner_angle: inner_angle.cos(),
        _pad: 0,
        ..Default::default()
    }
}

fn convert_area_as_point(light: &AreaLight) -> GpuLight {
    let range = (light.width.max(light.height) * 5.0).max(10.0);
    GpuLight {
        position_range: [0.0, 0.0, 0.0, range],
        direction_outer: [0.0, 0.0, -1.0, 0.0],
        color_intensity: [
            light.base.color.x,
            light.base.color.y,
            light.base.color.z,
            light.base.intensity,
        ],
        shadow_index: u32::MAX,
        light_type: LightType::Point as u32,
        inner_angle: 0.0,
        _pad: 0,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solid_rs::glam::Vec3;
    use solid_rs::scene::LightBase;

    #[test]
    fn test_convert_directional_light() {
        let light = DirectionalLight {
            base: LightBase {
                name: "Sun".to_string(),
                color: Vec3::new(1.0, 0.9, 0.8),
                intensity: 5.0,
            },
            extensions: Default::default(),
        };

        let scene_light = convert_directional(&light);
        assert_eq!(scene_light.color_intensity, [1.0, 0.9, 0.8, 5.0]);
        assert_eq!(scene_light.light_type, LightType::Directional as u32);
    }

    #[test]
    fn test_convert_point_light() {
        let light = PointLight {
            base: LightBase {
                name: "Bulb".to_string(),
                color: Vec3::ONE,
                intensity: 100.0,
            },
            range: Some(15.0),
            extensions: Default::default(),
        };

        let scene_light = convert_point(&light);
        assert_eq!(scene_light.position_range[3], 15.0);
        assert_eq!(scene_light.color_intensity[3], 100.0);
    }
}
