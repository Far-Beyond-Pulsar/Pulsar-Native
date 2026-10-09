//! Persistence compatibility (Pulsar-Native#1035 acceptance, #1081): a level
//! cooked for packaging loads, through the runtime loader, the same typed
//! components as the level it was cooked from: an old flat light record
//! migrated, a nested one, a disabled one, several instances of one class,
//! and an unknown class kept as its payload.

use engine_backend::scene::{attachments, RuntimeLevel, SceneWorldExt};
use helio_component::components::LightComponent;
use pulsar_class::ClassRegistry;
use pulsar_package::cook::{cook_level, AssetResolver, AssetSet};
use serde_json::{json, Value};

/// `(class, enabled, live intensity or unresolved payload)` per instance.
fn components(level: &RuntimeLevel, id: &str) -> Vec<(String, bool, Value)> {
    let scene = level.scene();
    let scene = scene.read();
    let world = &scene.world;
    let object = world.entity_for(id).expect("object loaded");
    attachments::instances(world, object)
        .into_iter()
        .map(|instance| {
            let class = attachments::meta(world, instance)
                .unwrap()
                .class_name
                .clone();
            let enabled = attachments::is_enabled(world, instance);
            let value = match world.get::<LightComponent>(instance) {
                Some(light) => json!(light.intensity.intensity),
                None => world
                    .get::<attachments::UnresolvedComponent>(instance)
                    .map(|unresolved| unresolved.data.clone())
                    .unwrap_or(Value::Null),
            };
            (class, enabled, value)
        })
        .collect()
}

#[test]
fn a_cooked_level_loads_the_same_typed_components() {
    let project = tempfile::tempdir().unwrap();
    let mut flat = serde_json::to_value(LightComponent::default()).unwrap();
    flat["intensity"] = json!(1002.0);
    let mut nested = LightComponent::default();
    nested.general.enabled = true;
    nested.intensity.intensity = 55.0;
    let nested = serde_json::to_value(nested).unwrap();
    let level = json!({
        "version": "2.1",
        "objects": [{
            "id": "lamp", "name": "Lamp", "object_type": "Empty",
            "transform": { "position": [0.0, 1.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] },
            "parent": null, "visible": true, "locked": false, "props": {}
        }],
        // The shape the editor saves: per-object records with their
        // enabled flags (inline `component_instances` are the legacy form).
        "components": { "lamp": [
            { "index": 0, "class_name": "LightComponent", "data": flat, "enabled": true },
            { "index": 1, "class_name": "LightComponent", "data": nested, "enabled": false },
            { "index": 2, "class_name": "LightComponent", "data": nested, "enabled": true },
            { "index": 3, "class_name": "NotARealComponent", "data": { "x": 1 }, "enabled": true }
        ] },
        "metadata": {},
        "editor": { "camera": { "position": [0.0, 2.0, 8.0], "yaw": 0.0, "pitch": 0.0 } }
    });

    let (cooked, _) = cook_level(
        level.clone(),
        &ClassRegistry::scan(project.path()),
        &AssetResolver::new(project.path(), None),
        &mut AssetSet::default(),
        "main.level",
    );
    let write = |name: &str, value: &Value| {
        let path = project.path().join(name);
        std::fs::write(&path, serde_json::to_string(value).unwrap()).unwrap();
        path
    };
    let original = RuntimeLevel::load(&write("original.level", &level)).expect("original loads");
    let packaged = RuntimeLevel::load(&write("cooked.level", &cooked)).expect("cooked loads");

    let expected = vec![
        ("LightComponent".to_string(), true, json!(1002.0)),
        ("LightComponent".to_string(), false, json!(55.0)),
        ("LightComponent".to_string(), true, json!(55.0)),
        ("NotARealComponent".to_string(), true, json!({ "x": 1 })),
    ];
    assert_eq!(components(&original, "lamp"), expected);
    assert_eq!(components(&packaged, "lamp"), expected);
}
