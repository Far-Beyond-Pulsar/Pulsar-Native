use engine_class_derive::register_world_component;

use super::LightComponent;

/// `LightComponent`'s JSON boundary decoder. Levels authored before the
/// sub-props migration store the flat property shape the editor once used
/// (`intensity` as a bare number); the Rust representation groups those same
/// properties into sub-props (`general`, `intensity`, etc.). Accept both, so
/// those projects load, and decode once into the typed value.
///
/// Nothing else is light-specific about storing one: the decoded value is
/// inserted through SceneDB's erased insert, and its generated
/// `LightComponentGpuMirror` row (including `general.enabled`) follows every
/// write through SceneDB's mirror dispatch.
fn decode_light_component(data: &serde_json::Value) -> Result<LightComponent, String> {
    if data.get("general").is_some() {
        pulsar_world_registry::decode_json(data)
    } else {
        Ok(LightComponent::from_component_data(data))
    }
}

// Helio holds no light actor: the light's derived `LightSourceRow` follows
// every write through SceneDB, removing the component clears it, and a
// disabled light's row says so (`general.enabled` is part of it).
#[register_world_component(decode = decode_light_component)]
impl LightComponent {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_nested_shape() {
        let mut light = LightComponent::default();
        light.general.enabled = false;
        light.intensity.intensity = 77.0;
        let json = serde_json::to_value(&light).expect("LightComponent must serialize");

        let decoded = decode_light_component(&json).unwrap();
        assert!(!decoded.general.enabled);
        assert_eq!(decoded.intensity.intensity, 77.0);
    }

    #[test]
    fn decodes_lights_saved_before_newer_fields() {
        // A sun saved before `shadows.shadow_priority` and
        // `shadow_max_resolution` existed failed to load, so the scene had
        // no sun and the sky and terrain rendered black.
        let mut json = serde_json::to_value(LightComponent::default()).unwrap();
        let shadows = json["shadows"].as_object_mut().unwrap();
        shadows.remove("shadow_priority");
        shadows.remove("shadow_max_resolution");
        shadows.insert("shadow_bias".into(), serde_json::json!(0.25));
        let decoded = decode_light_component(&json).unwrap();
        assert_eq!(
            decoded.shadows.shadow_priority,
            LightComponent::default().shadows.shadow_priority
        );
        assert_eq!(
            decoded.shadows.shadow_max_resolution,
            LightComponent::default().shadows.shadow_max_resolution
        );
        assert_eq!(decoded.shadows.shadow_bias, 0.25);
        // A whole sub-group missing loads too.
        json.as_object_mut().unwrap().remove("volumetrics");
        assert!(decode_light_component(&json).is_ok());
    }

    #[test]
    fn decodes_the_legacy_flat_shape() {
        // The shape that failed to hydrate in the 2026-10-04 editor log.
        let json = serde_json::json!({ "enabled": true, "intensity": 1002.0 });
        let decoded = decode_light_component(&json).unwrap();
        assert!(decoded.general.enabled);
        assert_eq!(decoded.intensity.intensity, 1002.0);
    }
}
