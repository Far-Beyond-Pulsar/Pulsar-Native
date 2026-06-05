//! Light component for scene lighting

use engine_class_derive::{engine_class, register_runtime_behavior, register_scene_props_applier};
use helio::{GpuLight, LightType as HelioLightType, SceneActor};
use pulsar_reflection::{
    ComponentRuntimeBehavior, ComponentRuntimeContext, Reflectable, RuntimeComponentOwner,
    ScenePropsProjector, scene_id_to_tag,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Reflectable)]
pub enum IntensityUnits {
    Unitless,
    Lumens,
    Candelas,
    Lux,
    Nits,
}

impl Default for IntensityUnits {
    fn default() -> Self {
        Self::Lumens
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Reflectable)]
pub enum MobileQualityLevel {
    Low,
    Medium,
    High,
    Epic,
}

impl Default for MobileQualityLevel {
    fn default() -> Self {
        Self::High
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Reflectable)]
pub enum ShadowCacheMode {
    Auto,
    StaticOnly,
    DynamicOnly,
    Disabled,
}

impl Default for ShadowCacheMode {
    fn default() -> Self {
        Self::Auto
    }
}

/// Light component for illuminating the scene.
#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("General", category_color = "#F4C542")]
#[category("Intensity", category_color = "#F59E0B")]
#[category("Color", category_color = "#FF8AAE")]
#[category("Attenuation", category_color = "#6EC5FF")]
#[category("Shadows", category_color = "#A78BFA", default_collapsed = true)]
#[category("Volumetrics", category_color = "#7EE787", default_collapsed = true)]
#[category("Light Function", category_color = "#22D3EE", default_collapsed = true)]
#[category("Performance", category_color = "#FB7185", default_collapsed = true)]
#[category("Advanced", category_color = "#9CA3AF", default_collapsed = true)]
pub struct LightComponent {
    #[property(category = "General")]
    pub enabled: bool,
    #[property(category = "General")]
    pub affects_world: bool,
    #[property(category = "General")]
    pub light_type: LightType,
    #[property(min = 0.0, max = 255.0, step = 1.0, category = "General")]
    pub light_channels: u64,
    #[property(category = "General")]
    pub lighting_channel_0: bool,
    #[property(category = "General")]
    pub lighting_channel_1: bool,
    #[property(category = "General")]
    pub lighting_channel_2: bool,

    #[property(min = 0.0, max = 200000.0, step = 10.0, category = "Intensity")]
    pub intensity: f32,
    #[property(category = "Intensity")]
    pub intensity_units: IntensityUnits,
    #[property(min = -10.0, max = 10.0, step = 0.1, category = "Intensity")]
    pub exposure_compensation: f32,
    #[property(category = "Intensity")]
    pub inverse_squared_falloff: bool,
    #[property(min = 0.0, max = 16.0, step = 0.1, category = "Intensity")]
    pub indirect_intensity: f32,
    #[property(min = 0.0, max = 100000.0, step = 10.0, category = "Intensity")]
    pub max_draw_distance: f32,
    #[property(min = 0.0, max = 10000.0, step = 10.0, category = "Intensity")]
    pub max_distance_fade_range: f32,

    #[property(category = "Color")]
    pub color: [f32; 4],
    #[property(category = "Color")]
    pub use_temperature: bool,
    #[property(min = 1000.0, max = 20000.0, step = 50.0, category = "Color")]
    pub temperature_kelvin: f32,
    #[property(min = -1.0, max = 1.0, step = 0.01, category = "Color")]
    pub temperature_tint: f32,
    #[property(min = 0.0, max = 5.0, step = 0.01, category = "Color")]
    pub color_saturation: f32,
    #[property(min = 0.0, max = 5.0, step = 0.01, category = "Color")]
    pub color_contrast: f32,
    #[property(category = "Color")]
    pub use_physical_light_color: bool,

    #[property(min = 0.0, max = 5000.0, step = 1.0, category = "Attenuation")]
    pub range: f32,
    #[property(min = 0.0, max = 5000.0, step = 1.0, category = "Attenuation")]
    pub falloff_start: f32,
    #[property(min = 0.1, max = 16.0, step = 0.1, category = "Attenuation")]
    pub attenuation_exponent: f32,
    #[property(min = 0.0, max = 100.0, step = 0.1, category = "Attenuation")]
    pub source_radius: f32,
    #[property(min = 0.0, max = 200.0, step = 0.1, category = "Attenuation")]
    pub source_length: f32,
    #[property(min = 0.0, max = 90.0, step = 1.0, category = "Attenuation")]
    pub inner_cone_angle: f32,
    #[property(min = 0.0, max = 90.0, step = 1.0, category = "Attenuation")]
    pub outer_cone_angle: f32,

    #[property(category = "Shadows")]
    pub cast_shadows: bool,
    #[property(category = "Shadows")]
    pub cast_static_shadows: bool,
    #[property(category = "Shadows")]
    pub cast_dynamic_shadows: bool,
    #[property(category = "Shadows")]
    pub cast_volumetric_shadow: bool,
    #[property(category = "Shadows")]
    pub cast_contact_shadows: bool,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Shadows")]
    pub shadow_bias: f32,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Shadows")]
    pub shadow_normal_bias: f32,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Shadows")]
    pub shadow_slope_bias: f32,
    #[property(min = 0.0, max = 10.0, step = 0.05, category = "Shadows")]
    pub shadow_filter_sharpen: f32,
    #[property(min = 0.0, max = 10.0, step = 0.05, category = "Shadows")]
    pub shadow_softness: f32,
    #[property(min = 0.25, max = 4.0, step = 0.05, category = "Shadows")]
    pub shadow_resolution_scale: f32,
    #[property(min = 0.0, max = 5.0, step = 0.01, category = "Shadows")]
    pub contact_shadow_non_shadow_casting_intensity: f32,

    #[property(category = "Volumetrics")]
    pub affects_volumetric_fog: bool,
    #[property(min = 0.0, max = 8.0, step = 0.05, category = "Volumetrics")]
    pub volumetric_scattering_intensity: f32,
    #[property(min = 0.0, max = 8.0, step = 0.05, category = "Volumetrics")]
    pub volumetric_shadow_intensity: f32,
    #[property(min = 0.0, max = 8.0, step = 0.05, category = "Volumetrics")]
    pub fog_inscattering_intensity: f32,
    #[property(min = 0.0, max = 50.0, step = 0.1, category = "Volumetrics")]
    pub contact_shadow_length: f32,

    #[property(category = "Light Function")]
    pub light_function_material: String,
    #[property(category = "Light Function")]
    pub light_function_scale: [f32; 3],
    #[property(min = 0.0, max = 100000.0, step = 10.0, category = "Light Function")]
    pub light_function_fade_distance: f32,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Light Function")]
    pub light_function_disabled_brightness: f32,

    #[property(category = "Performance")]
    pub mobile_quality_level: MobileQualityLevel,
    #[property(category = "Performance")]
    pub ray_tracing_inclusion: bool,
    #[property(category = "Performance")]
    pub virtual_shadow_map_enabled: bool,
    #[property(category = "Performance")]
    pub shadow_cache_mode: ShadowCacheMode,
    #[property(min = 0.0, max = 65535.0, step = 1.0, category = "Performance")]
    pub per_view_visibility_mask: u64,

    #[property(category = "Advanced")]
    pub affects_translucency: bool,
    #[property(category = "Advanced")]
    pub affects_reflections: bool,
    #[property(category = "Advanced")]
    pub affects_global_illumination: bool,
    #[property(min = 0.0, max = 8.0, step = 0.01, category = "Advanced")]
    pub specular_scale: f32,
    #[property(min = 0.0, max = 8.0, step = 0.01, category = "Advanced")]
    pub diffuse_scale: f32,
}

/// Type of light source
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Reflectable)]
pub enum LightType {
    /// Directional light (like the sun) - infinite distance, parallel rays
    Directional,

    /// Point light - emits in all directions from a point
    Point,

    /// Spot light - emits in a cone from a point
    Spot,

    /// Area light - emits from a rectangular area
    Area,
}

impl Default for LightType {
    fn default() -> Self {
        LightType::Point
    }
}

impl Default for LightComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            affects_world: true,
            light_type: LightType::Point,
            light_channels: 0xFF,
            lighting_channel_0: true,
            lighting_channel_1: false,
            lighting_channel_2: false,
            intensity: 1000.0,
            intensity_units: IntensityUnits::Lumens,
            exposure_compensation: 0.0,
            inverse_squared_falloff: true,
            indirect_intensity: 1.0,
            max_draw_distance: 0.0,
            max_distance_fade_range: 0.0,
            color: [1.0, 1.0, 1.0, 1.0],
            use_temperature: false,
            temperature_kelvin: 6500.0,
            temperature_tint: 0.0,
            color_saturation: 1.0,
            color_contrast: 1.0,
            use_physical_light_color: true,
            range: 1000.0,
            falloff_start: 0.0,
            attenuation_exponent: 2.0,
            source_radius: 0.0,
            source_length: 0.0,
            inner_cone_angle: 30.0,
            outer_cone_angle: 45.0,
            cast_shadows: true,
            cast_static_shadows: true,
            cast_dynamic_shadows: true,
            cast_volumetric_shadow: true,
            cast_contact_shadows: false,
            shadow_bias: 0.5,
            shadow_normal_bias: 0.5,
            shadow_slope_bias: 0.5,
            shadow_filter_sharpen: 0.0,
            shadow_softness: 1.0,
            shadow_resolution_scale: 1.0,
            contact_shadow_non_shadow_casting_intensity: 0.0,
            affects_volumetric_fog: true,
            volumetric_scattering_intensity: 1.0,
            volumetric_shadow_intensity: 1.0,
            fog_inscattering_intensity: 1.0,
            contact_shadow_length: 0.0,
            light_function_material: String::new(),
            light_function_scale: [1.0, 1.0, 1.0],
            light_function_fade_distance: 0.0,
            light_function_disabled_brightness: 0.0,
            mobile_quality_level: MobileQualityLevel::High,
            ray_tracing_inclusion: true,
            virtual_shadow_map_enabled: true,
            shadow_cache_mode: ShadowCacheMode::Auto,
            per_view_visibility_mask: 0xFFFF,
            affects_translucency: true,
            affects_reflections: true,
            affects_global_illumination: true,
            specular_scale: 1.0,
            diffuse_scale: 1.0,
        }
    }
}

#[register_scene_props_applier]
impl ScenePropsProjector for LightComponent {
    const CLASS_NAME: &'static str = "LightComponent";

    fn apply_scene_props(props: &mut HashMap<String, Value>, component_data: Option<&Value>) {
        for key in [
            "enabled",
            "affects_world",
            "light_type",
            "light_channels",
            "lighting_channel_0",
            "lighting_channel_1",
            "lighting_channel_2",
            "intensity",
            "intensity_units",
            "exposure_compensation",
            "inverse_squared_falloff",
            "indirect_intensity",
            "max_draw_distance",
            "max_distance_fade_range",
            "color",
            "use_temperature",
            "temperature_kelvin",
            "temperature_tint",
            "color_saturation",
            "color_contrast",
            "use_physical_light_color",
            "range",
            "falloff_start",
            "attenuation_exponent",
            "source_radius",
            "source_length",
            "inner_cone_angle",
            "outer_cone_angle",
            "cast_shadows",
            "cast_static_shadows",
            "cast_dynamic_shadows",
            "cast_volumetric_shadow",
            "cast_contact_shadows",
            "shadow_bias",
            "shadow_normal_bias",
            "shadow_slope_bias",
            "shadow_filter_sharpen",
            "shadow_softness",
            "shadow_resolution_scale",
            "contact_shadow_non_shadow_casting_intensity",
            "affects_volumetric_fog",
            "volumetric_scattering_intensity",
            "volumetric_shadow_intensity",
            "fog_inscattering_intensity",
            "contact_shadow_length",
            "light_function_material",
            "light_function_scale",
            "light_function_fade_distance",
            "light_function_disabled_brightness",
            "mobile_quality_level",
            "ray_tracing_inclusion",
            "virtual_shadow_map_enabled",
            "shadow_cache_mode",
            "per_view_visibility_mask",
            "affects_translucency",
            "affects_reflections",
            "affects_global_illumination",
            "specular_scale",
            "diffuse_scale",
        ] {
            props.remove(key);
        }

        let Some(data) = component_data else {
            return;
        };

        let light = Self::from_component_data(data);
        for (k, v) in light.to_scene_props() {
            props.insert(k, v);
        }
    }
}

impl LightComponent {
    /// Build a light component from reflection JSON data.
    /// Missing fields are filled with default values so partial component
    /// payloads can still project useful renderer props.
    pub fn from_component_data(data: &Value) -> Self {
        let mut light = Self::default();
        let parse_enum = |ix: u64, max: usize| -> Option<usize> {
            let i = ix as usize;
            (i < max).then_some(i)
        };

        if let Some(obj) = data.as_object() {
            let read_bool = |k: &str| obj.get(k).and_then(|v| v.as_bool());
            let read_f32 = |k: &str| obj.get(k).and_then(|v| v.as_f64()).map(|v| v as f32);
            let read_u64 = |k: &str| obj.get(k).and_then(|v| v.as_u64());
            let read_string = |k: &str| {
                obj.get(k)
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            };
            let read_color = |k: &str| -> Option<[f32; 4]> {
                let arr = obj.get(k)?.as_array()?;
                (arr.len() >= 4).then_some([
                    arr[0].as_f64().unwrap_or(1.0) as f32,
                    arr[1].as_f64().unwrap_or(1.0) as f32,
                    arr[2].as_f64().unwrap_or(1.0) as f32,
                    arr[3].as_f64().unwrap_or(1.0) as f32,
                ])
            };
            let read_vec3 = |k: &str| -> Option<[f32; 3]> {
                let arr = obj.get(k)?.as_array()?;
                (arr.len() >= 3).then_some([
                    arr[0].as_f64().unwrap_or(1.0) as f32,
                    arr[1].as_f64().unwrap_or(1.0) as f32,
                    arr[2].as_f64().unwrap_or(1.0) as f32,
                ])
            };

            if let Some(v) = read_bool("enabled") {
                light.enabled = v;
            }
            if let Some(v) = read_bool("affects_world") {
                light.affects_world = v;
            }
            if let Some(ix) = obj.get("light_type").and_then(|v| v.as_u64()) {
                light.light_type = match parse_enum(ix, 4) {
                    Some(0) => LightType::Directional,
                    Some(1) => LightType::Point,
                    Some(2) => LightType::Spot,
                    Some(3) => LightType::Area,
                    _ => light.light_type,
                }
            }
            if let Some(v) = read_u64("light_channels") {
                light.light_channels = v;
            }
            if let Some(v) = read_bool("lighting_channel_0") {
                light.lighting_channel_0 = v;
            }
            if let Some(v) = read_bool("lighting_channel_1") {
                light.lighting_channel_1 = v;
            }
            if let Some(v) = read_bool("lighting_channel_2") {
                light.lighting_channel_2 = v;
            }
            if let Some(v) = read_f32("intensity") {
                light.intensity = v;
            }
            if let Some(ix) = read_u64("intensity_units") {
                light.intensity_units = match parse_enum(ix, 5) {
                    Some(0) => IntensityUnits::Unitless,
                    Some(1) => IntensityUnits::Lumens,
                    Some(2) => IntensityUnits::Candelas,
                    Some(3) => IntensityUnits::Lux,
                    Some(4) => IntensityUnits::Nits,
                    _ => light.intensity_units,
                };
            }
            if let Some(v) = read_f32("exposure_compensation") {
                light.exposure_compensation = v;
            }
            if let Some(v) = read_bool("inverse_squared_falloff") {
                light.inverse_squared_falloff = v;
            }
            if let Some(v) = read_f32("indirect_intensity") {
                light.indirect_intensity = v;
            }
            if let Some(v) = read_f32("max_draw_distance") {
                light.max_draw_distance = v;
            }
            if let Some(v) = read_f32("max_distance_fade_range") {
                light.max_distance_fade_range = v;
            }
            if let Some(v) = read_color("color") {
                light.color = v;
            }
            if let Some(v) = read_bool("use_temperature") {
                light.use_temperature = v;
            }
            if let Some(v) = read_f32("temperature_kelvin") {
                light.temperature_kelvin = v;
            }
            if let Some(v) = read_f32("temperature_tint") {
                light.temperature_tint = v;
            }
            if let Some(v) = read_f32("color_saturation") {
                light.color_saturation = v;
            }
            if let Some(v) = read_f32("color_contrast") {
                light.color_contrast = v;
            }
            if let Some(v) = read_bool("use_physical_light_color") {
                light.use_physical_light_color = v;
            }
            if let Some(v) = read_f32("range") {
                light.range = v;
            }
            if let Some(v) = read_f32("falloff_start") {
                light.falloff_start = v;
            }
            if let Some(v) = read_f32("attenuation_exponent") {
                light.attenuation_exponent = v;
            }
            if let Some(v) = read_f32("source_radius") {
                light.source_radius = v;
            }
            if let Some(v) = read_f32("source_length") {
                light.source_length = v;
            }
            if let Some(v) = read_f32("inner_cone_angle") {
                light.inner_cone_angle = v;
            }
            if let Some(v) = read_f32("outer_cone_angle") {
                light.outer_cone_angle = v;
            }
            if let Some(v) = read_bool("cast_shadows") {
                light.cast_shadows = v;
            }
            if let Some(v) = read_bool("cast_static_shadows") {
                light.cast_static_shadows = v;
            }
            if let Some(v) = read_bool("cast_dynamic_shadows") {
                light.cast_dynamic_shadows = v;
            }
            if let Some(v) = read_bool("cast_volumetric_shadow") {
                light.cast_volumetric_shadow = v;
            }
            if let Some(v) = read_bool("cast_contact_shadows") {
                light.cast_contact_shadows = v;
            }
            if let Some(v) = read_f32("shadow_bias") {
                light.shadow_bias = v;
            }
            if let Some(v) = read_f32("shadow_normal_bias") {
                light.shadow_normal_bias = v;
            }
            if let Some(v) = read_f32("shadow_slope_bias") {
                light.shadow_slope_bias = v;
            }
            if let Some(v) = read_f32("shadow_filter_sharpen") {
                light.shadow_filter_sharpen = v;
            }
            if let Some(v) = read_f32("shadow_softness") {
                light.shadow_softness = v;
            }
            if let Some(v) = read_f32("shadow_resolution_scale") {
                light.shadow_resolution_scale = v;
            }
            if let Some(v) = read_f32("contact_shadow_non_shadow_casting_intensity") {
                light.contact_shadow_non_shadow_casting_intensity = v;
            }
            if let Some(v) = read_bool("affects_volumetric_fog") {
                light.affects_volumetric_fog = v;
            }
            if let Some(v) = read_f32("volumetric_scattering_intensity") {
                light.volumetric_scattering_intensity = v;
            }
            if let Some(v) = read_f32("volumetric_shadow_intensity") {
                light.volumetric_shadow_intensity = v;
            }
            if let Some(v) = read_f32("fog_inscattering_intensity") {
                light.fog_inscattering_intensity = v;
            }
            if let Some(v) = read_f32("contact_shadow_length") {
                light.contact_shadow_length = v;
            }
            if let Some(v) = read_string("light_function_material") {
                light.light_function_material = v;
            }
            if let Some(v) = read_vec3("light_function_scale") {
                light.light_function_scale = v;
            }
            if let Some(v) = read_f32("light_function_fade_distance") {
                light.light_function_fade_distance = v;
            }
            if let Some(v) = read_f32("light_function_disabled_brightness") {
                light.light_function_disabled_brightness = v;
            }
            if let Some(ix) = read_u64("mobile_quality_level") {
                light.mobile_quality_level = match parse_enum(ix, 4) {
                    Some(0) => MobileQualityLevel::Low,
                    Some(1) => MobileQualityLevel::Medium,
                    Some(2) => MobileQualityLevel::High,
                    Some(3) => MobileQualityLevel::Epic,
                    _ => light.mobile_quality_level,
                };
            }
            if let Some(v) = read_bool("ray_tracing_inclusion") {
                light.ray_tracing_inclusion = v;
            }
            if let Some(v) = read_bool("virtual_shadow_map_enabled") {
                light.virtual_shadow_map_enabled = v;
            }
            if let Some(ix) = read_u64("shadow_cache_mode") {
                light.shadow_cache_mode = match parse_enum(ix, 4) {
                    Some(0) => ShadowCacheMode::Auto,
                    Some(1) => ShadowCacheMode::StaticOnly,
                    Some(2) => ShadowCacheMode::DynamicOnly,
                    Some(3) => ShadowCacheMode::Disabled,
                    _ => light.shadow_cache_mode,
                };
            }
            if let Some(v) = read_u64("per_view_visibility_mask") {
                light.per_view_visibility_mask = v;
            }
            if let Some(v) = read_bool("affects_translucency") {
                light.affects_translucency = v;
            }
            if let Some(v) = read_bool("affects_reflections") {
                light.affects_reflections = v;
            }
            if let Some(v) = read_bool("affects_global_illumination") {
                light.affects_global_illumination = v;
            }
            if let Some(v) = read_f32("specular_scale") {
                light.specular_scale = v;
            }
            if let Some(v) = read_f32("diffuse_scale") {
                light.diffuse_scale = v;
            }
        }

        light
    }

    /// Project renderer-facing scene props from the reflection component.
    pub fn to_scene_props(&self) -> HashMap<String, Value> {
        let mut out = HashMap::new();
        out.insert(
            "color".to_string(),
            serde_json::json!([self.color[0], self.color[1], self.color[2], self.color[3]]),
        );
        out.insert("enabled".to_string(), Value::from(self.enabled));
        out.insert("affects_world".to_string(), Value::from(self.affects_world));
        out.insert(
            "light_type".to_string(),
            Value::from(self.light_type as u64),
        );
        out.insert("light_channels".to_string(), Value::from(self.light_channels));
        out.insert(
            "lighting_channel_0".to_string(),
            Value::from(self.lighting_channel_0),
        );
        out.insert(
            "lighting_channel_1".to_string(),
            Value::from(self.lighting_channel_1),
        );
        out.insert(
            "lighting_channel_2".to_string(),
            Value::from(self.lighting_channel_2),
        );
        out.insert("intensity".to_string(), Value::from(self.intensity));
        out.insert(
            "intensity_units".to_string(),
            Value::from(self.intensity_units as u64),
        );
        out.insert(
            "exposure_compensation".to_string(),
            Value::from(self.exposure_compensation),
        );
        out.insert(
            "inverse_squared_falloff".to_string(),
            Value::from(self.inverse_squared_falloff),
        );
        out.insert(
            "indirect_intensity".to_string(),
            Value::from(self.indirect_intensity),
        );
        out.insert(
            "max_draw_distance".to_string(),
            Value::from(self.max_draw_distance),
        );
        out.insert(
            "max_distance_fade_range".to_string(),
            Value::from(self.max_distance_fade_range),
        );
        out.insert(
            "use_physical_light_color".to_string(),
            Value::from(self.use_physical_light_color),
        );
        out.insert(
            "temperature_tint".to_string(),
            Value::from(self.temperature_tint),
        );
        out.insert(
            "color_saturation".to_string(),
            Value::from(self.color_saturation),
        );
        out.insert("color_contrast".to_string(), Value::from(self.color_contrast));
        out.insert("range".to_string(), Value::from(self.range));
        out.insert("falloff_start".to_string(), Value::from(self.falloff_start));
        out.insert(
            "attenuation_exponent".to_string(),
            Value::from(self.attenuation_exponent),
        );
        out.insert("source_radius".to_string(), Value::from(self.source_radius));
        out.insert("source_length".to_string(), Value::from(self.source_length));
        out.insert(
            "inner_cone_angle".to_string(),
            Value::from(self.inner_cone_angle),
        );
        out.insert(
            "outer_cone_angle".to_string(),
            Value::from(self.outer_cone_angle),
        );
        out.insert("cast_shadows".to_string(), Value::from(self.cast_shadows));
        out.insert(
            "cast_static_shadows".to_string(),
            Value::from(self.cast_static_shadows),
        );
        out.insert(
            "cast_dynamic_shadows".to_string(),
            Value::from(self.cast_dynamic_shadows),
        );
        out.insert(
            "cast_volumetric_shadow".to_string(),
            Value::from(self.cast_volumetric_shadow),
        );
        out.insert(
            "cast_contact_shadows".to_string(),
            Value::from(self.cast_contact_shadows),
        );
        out.insert("shadow_bias".to_string(), Value::from(self.shadow_bias));
        out.insert(
            "shadow_normal_bias".to_string(),
            Value::from(self.shadow_normal_bias),
        );
        out.insert(
            "shadow_slope_bias".to_string(),
            Value::from(self.shadow_slope_bias),
        );
        out.insert(
            "shadow_filter_sharpen".to_string(),
            Value::from(self.shadow_filter_sharpen),
        );
        out.insert(
            "shadow_softness".to_string(),
            Value::from(self.shadow_softness),
        );
        out.insert(
            "shadow_resolution_scale".to_string(),
            Value::from(self.shadow_resolution_scale),
        );
        out.insert(
            "volumetric_scattering_intensity".to_string(),
            Value::from(self.volumetric_scattering_intensity),
        );
        out.insert(
            "volumetric_shadow_intensity".to_string(),
            Value::from(self.volumetric_shadow_intensity),
        );
        out.insert(
            "fog_inscattering_intensity".to_string(),
            Value::from(self.fog_inscattering_intensity),
        );
        out.insert(
            "contact_shadow_length".to_string(),
            Value::from(self.contact_shadow_length),
        );
        out.insert(
            "contact_shadow_non_shadow_casting_intensity".to_string(),
            Value::from(self.contact_shadow_non_shadow_casting_intensity),
        );
        out.insert(
            "affects_volumetric_fog".to_string(),
            Value::from(self.affects_volumetric_fog),
        );
        out.insert(
            "use_temperature".to_string(),
            Value::from(self.use_temperature),
        );
        out.insert(
            "temperature_kelvin".to_string(),
            Value::from(self.temperature_kelvin),
        );
        out.insert(
            "light_function_material".to_string(),
            Value::from(self.light_function_material.clone()),
        );
        out.insert(
            "light_function_scale".to_string(),
            serde_json::json!([
                self.light_function_scale[0],
                self.light_function_scale[1],
                self.light_function_scale[2]
            ]),
        );
        out.insert(
            "light_function_fade_distance".to_string(),
            Value::from(self.light_function_fade_distance),
        );
        out.insert(
            "light_function_disabled_brightness".to_string(),
            Value::from(self.light_function_disabled_brightness),
        );
        out.insert(
            "mobile_quality_level".to_string(),
            Value::from(self.mobile_quality_level as u64),
        );
        out.insert(
            "ray_tracing_inclusion".to_string(),
            Value::from(self.ray_tracing_inclusion),
        );
        out.insert(
            "virtual_shadow_map_enabled".to_string(),
            Value::from(self.virtual_shadow_map_enabled),
        );
        out.insert(
            "shadow_cache_mode".to_string(),
            Value::from(self.shadow_cache_mode as u64),
        );
        out.insert(
            "per_view_visibility_mask".to_string(),
            Value::from(self.per_view_visibility_mask),
        );
        out.insert(
            "affects_translucency".to_string(),
            Value::from(self.affects_translucency),
        );
        out.insert(
            "affects_reflections".to_string(),
            Value::from(self.affects_reflections),
        );
        out.insert(
            "affects_global_illumination".to_string(),
            Value::from(self.affects_global_illumination),
        );
        out.insert("specular_scale".to_string(), Value::from(self.specular_scale));
        out.insert("diffuse_scale".to_string(), Value::from(self.diffuse_scale));
        out
    }
}

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for LightComponent {
    const CLASS_NAME: &'static str = "LightComponent";

    fn sync_component(
        owner: &RuntimeComponentOwner,
        component_index: usize,
        component_data: &Value,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        let light = Self::from_component_data(component_data);
        if !light.enabled {
            return;
        }

        let helio_type = match light.light_type {
            LightType::Directional => HelioLightType::Directional,
            LightType::Point => HelioLightType::Point,
            LightType::Spot => HelioLightType::Spot,
            LightType::Area => HelioLightType::Point, // helio has no Area; nearest equivalent
        };

        let [px, py, pz] = owner.position;

        // Build the GpuLight directly — single source of truth for how a
        // LightComponent maps to the GPU.  The context handles insert-vs-update
        // and all internal helio tracking.
        let gpu = GpuLight {
            position_range: [px, py, pz, light.range],
            direction_outer: [0.0, -1.0, 0.0, light.outer_cone_angle.to_radians()],
            color_intensity: [
                light.color[0],
                light.color[1],
                light.color[2],
                light.intensity,
            ],
            // shadow_index 0 = shadows enabled (helio assigns the atlas slot in flush()).
            // u32::MAX = shadows disabled.
            shadow_index: if light.cast_shadows { 0 } else { u32::MAX },
            light_type: helio_type as u32,
            inner_angle: light.inner_cone_angle.to_radians(),
            _pad: 0,
        };

        // Tag the actor with a hash of the SceneDb ID so the picker can
        // identify it without any external reverse-lookup map.
        let tag = scene_id_to_tag(owner.scene_object_id);
        context
            .renderer_mut()
            .scene_mut()
            .insert_actor(SceneActor::light_with_tag(gpu, tag));
    }
}
