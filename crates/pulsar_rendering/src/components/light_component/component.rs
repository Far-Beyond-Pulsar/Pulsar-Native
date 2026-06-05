use engine_class_derive::engine_class;

use super::sub_props::{
    AdvancedLightProps, AttenuationLightProps, ColorLightProps, GeneralLightProps,
    IntensityLightProps, LightFunctionProps, PerformanceLightProps, ShadowLightProps,
    VolumetricLightProps,
};

#[engine_class(category = "Rendering", default, clone, debug, serialize, deserialize)]
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
    #[sub_props]
    pub general: GeneralLightProps,
    #[sub_props]
    pub intensity: IntensityLightProps,
    #[sub_props]
    pub color: ColorLightProps,
    #[sub_props]
    pub attenuation: AttenuationLightProps,
    #[sub_props]
    pub shadows: ShadowLightProps,
    #[sub_props]
    pub volumetrics: VolumetricLightProps,
    #[sub_props]
    pub light_function: LightFunctionProps,
    #[sub_props]
    pub performance: PerformanceLightProps,
    #[sub_props]
    pub advanced: AdvancedLightProps,
}
