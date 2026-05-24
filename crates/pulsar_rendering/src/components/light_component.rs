//! Light component for scene lighting

use engine_class_derive::{EngineClass, RegisterRuntimeBehavior};
use pulsar_reflection::{
    ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner, RuntimeLightDesc,
    RuntimeLightType, ScenePropsProjector, Reflectable,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Light component for illuminating the scene
///
/// This component demonstrates:
/// - Enum properties (light type)
/// - Color properties (RGBA)
/// - Float properties with ranges
/// - Boolean properties for toggles
#[derive(EngineClass, RegisterRuntimeBehavior, Default, Clone, Debug, Serialize, Deserialize)]
#[category("Rendering")]
pub struct LightComponent {
    /// Type of light source
    #[property]
    pub light_type: LightType,

    /// Light intensity in lumens
    #[property(min = 0.0, max = 10000.0, step = 10.0)]
    pub intensity: f32,

    /// Light color (RGBA)
    #[property]
    pub color: [f32; 4],

    /// Maximum range of the light (for point and spot lights)
    #[property(min = 0.0, max = 1000.0, step = 1.0)]
    pub range: f32,

    /// Inner cone angle in degrees (for spot lights)
    #[property(min = 0.0, max = 90.0, step = 1.0)]
    pub inner_cone_angle: f32,

    /// Outer cone angle in degrees (for spot lights)
    #[property(min = 0.0, max = 90.0, step = 1.0)]
    pub outer_cone_angle: f32,

    /// Whether this light casts shadows
    #[property]
    pub cast_shadows: bool,

    /// Shadow map resolution (power of 2)
    #[property(min = 256.0, max = 4096.0, step = 256.0)]
    pub shadow_resolution: f32,

    /// Shadow bias to prevent shadow acne
    #[property(min = 0.0, max = 1.0, step = 0.001)]
    pub shadow_bias: f32,
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

impl ScenePropsProjector for LightComponent {
    const CLASS_NAME: &'static str = "LightComponent";

    fn apply_scene_props(props: &mut HashMap<String, Value>, component_data: Option<&Value>) {
        // Always clear previously projected values first.
        props.remove("color");
        props.remove("intensity");
        props.remove("range");

        let Some(data) = component_data else {
            return;
        };

        let light = Self::from_component_data(data);
        for (k, v) in light.to_scene_props() {
            props.insert(k, v);
        }
    }
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::ScenePropsApplierRegistration {
        class_name: <LightComponent as ScenePropsProjector>::CLASS_NAME,
        apply: <LightComponent as ScenePropsProjector>::apply_scene_props,
    }
}

impl LightComponent {
    /// Build a light component from reflection JSON data.
    /// Missing fields are filled with default values so partial component
    /// payloads can still project useful renderer props.
    pub fn from_component_data(data: &Value) -> Self {
        let mut light = Self::default();

        if let Some(obj) = data.as_object() {
            if let Some(v) = obj.get("intensity").and_then(|v| v.as_f64()) {
                light.intensity = v as f32;
            }
            if let Some(v) = obj.get("range").and_then(|v| v.as_f64()) {
                light.range = v as f32;
            }
            if let Some(arr) = obj.get("color").and_then(|v| v.as_array()) {
                if arr.len() >= 4 {
                    light.color = [
                        arr[0].as_f64().unwrap_or(1.0) as f32,
                        arr[1].as_f64().unwrap_or(1.0) as f32,
                        arr[2].as_f64().unwrap_or(1.0) as f32,
                        arr[3].as_f64().unwrap_or(1.0) as f32,
                    ];
                }
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
        out.insert("intensity".to_string(), Value::from(self.intensity));
        out.insert("range".to_string(), Value::from(self.range));
        out
    }
}

impl ComponentRuntimeBehavior for LightComponent {
    const CLASS_NAME: &'static str = "LightComponent";

    fn sync_component(
        owner: &RuntimeComponentOwner,
        component_index: usize,
        component_data: &Value,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        let light = Self::from_component_data(component_data);
        let light_type = match light.light_type {
            LightType::Directional => RuntimeLightType::Directional,
            LightType::Point => RuntimeLightType::Point,
            LightType::Spot => RuntimeLightType::Spot,
            LightType::Area => RuntimeLightType::Area,
        };

        context.upsert_light(RuntimeLightDesc {
            actor_key: format!("{}::light::{}", owner.scene_object_id, component_index),
            light_type,
            color: light.color,
            intensity: light.intensity,
            range: light.range,
            inner_cone_angle_deg: light.inner_cone_angle,
            outer_cone_angle_deg: light.outer_cone_angle,
        });
    }
}

