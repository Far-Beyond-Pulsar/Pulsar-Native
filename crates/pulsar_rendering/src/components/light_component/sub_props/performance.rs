use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

use super::super::{MobileQualityLevel, ShadowCacheMode};

#[engine_class(clone, debug, serialize, deserialize)]
#[category("Performance", category_color = "#FB7185", default_collapsed = true)]
pub struct PerformanceLightProps {
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
}

impl Default for PerformanceLightProps {
    fn default() -> Self {
        Self {
            mobile_quality_level: MobileQualityLevel::High,
            ray_tracing_inclusion: true,
            virtual_shadow_map_enabled: true,
            shadow_cache_mode: ShadowCacheMode::Auto,
            per_view_visibility_mask: 0xFFFF,
        }
    }
}

impl PerformanceLightProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(ix) = obj.get("mobile_quality_level").and_then(|v| v.as_u64()) {
            self.mobile_quality_level = match ix {
                0 => MobileQualityLevel::Low,
                1 => MobileQualityLevel::Medium,
                2 => MobileQualityLevel::High,
                3 => MobileQualityLevel::Epic,
                _ => self.mobile_quality_level,
            };
        }
        if let Some(v) = obj.get("ray_tracing_inclusion").and_then(|v| v.as_bool()) {
            self.ray_tracing_inclusion = v;
        }
        if let Some(v) = obj.get("virtual_shadow_map_enabled").and_then(|v| v.as_bool()) {
            self.virtual_shadow_map_enabled = v;
        }
        if let Some(ix) = obj.get("shadow_cache_mode").and_then(|v| v.as_u64()) {
            self.shadow_cache_mode = match ix {
                0 => ShadowCacheMode::Auto,
                1 => ShadowCacheMode::StaticOnly,
                2 => ShadowCacheMode::DynamicOnly,
                3 => ShadowCacheMode::Disabled,
                _ => self.shadow_cache_mode,
            };
        }
        if let Some(v) = obj.get("per_view_visibility_mask").and_then(|v| v.as_u64()) {
            self.per_view_visibility_mask = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
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
    }
}
