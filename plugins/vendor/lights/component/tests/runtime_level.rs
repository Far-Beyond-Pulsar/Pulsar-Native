//! A level's lights load as the lights' components: registered classes
//! hydrate to typed values, legacy shapes migrate, load errors name the
//! property, persisted order and enabled state are kept, and a level with
//! two directional lights loads with one casting. (Moved from
//! `engine_backend::scene::runtime_level`'s tests with the light component.)

use engine_backend::scene::{RuntimeLevel, RuntimeLevelError, SceneWorldExt};
use lights_component::LightComponent;
use pulsar_scene::format::SceneFile;
use pulsar_scenedb::World;
use serde_json::Value;

/// The object's enabled light instances' values.
fn lights<'w>(world: &'w World, id: &str) -> Vec<&'w LightComponent> {
    pulsar_scene_model::attachments::enabled_components_of::<LightComponent>(
        world,
        world.entity_for(id).unwrap(),
    )
    .into_iter()
    .map(|(_, light)| light)
    .collect()
}

/// Build the component-instance array the way the editor itself writes
/// it: `serde_json::to_value` of the FULL typed component (level files
/// always carry complete component JSON, never sparse fragments -- the
/// hydrate fns deserialize into the real structs, which don't default
/// missing sub-groups).
fn light_instances_json(intensity: f32) -> Value {
    let mut light = LightComponent::default();
    light.general.enabled = true;
    light.intensity.intensity = intensity;
    serde_json::json!([
        { "index": 0, "class_name": "LightComponent", "data": serde_json::to_value(&light).unwrap() }
    ])
}

const SAMPLE_LEVEL: &str = r#"{
    "version": "2.1",
    "objects": [
        {
            "id": "sun", "name": "Sun", "object_type": {"Light": "Point"},
            "transform": {"position": [1.0, 5.0, 2.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]},
            "parent": null, "visible": true, "locked": false, "props": {}
        },
        {
            "id": "group", "name": "Group", "object_type": "Folder",
            "transform": {"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]},
            "parent": null, "visible": true, "locked": false, "props": {}
        },
        {
            "id": "cube", "name": "Cube", "object_type": {"Mesh": "Cube"},
            "transform": {"position": [4.0, 0.0, 0.0], "rotation": [0.0, 90.0, 0.0], "scale": [2.0, 2.0, 2.0]},
            "parent": "group", "visible": false, "locked": true, "props": {}
        }
    ],
    "components": {},
    "metadata": {},
    "editor": {"camera": {"position": [10.0, 20.0, 30.0], "yaw": 1.0, "pitch": -0.25}}
}"#;

/// The object's attached component records, in order.
fn records(world: &World, id: &str) -> Vec<pulsar_scene_model::ComponentInstance> {
    pulsar_world_registry::component_records(world, world.entity_for(id).unwrap())
}

fn sample_level() -> RuntimeLevel {
    let file: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
    RuntimeLevel::from_scene_file(file).expect("sample hydrates")
}

fn level_with_sun_components(components_section: Value, instances: Option<Value>) -> SceneFile {
    let mut file: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
    if let Some(instances) = instances {
        file.objects[0].component_instances = Some(instances);
    }
    file.components = components_section;
    file
}

/// #637: registered classes hydrate to typed World values.
#[test]
fn registered_component_classes_hydrate_to_typed_values() {
    let file = level_with_sun_components(Value::Null, Some(light_instances_json(750.0)));
    let level = RuntimeLevel::from_scene_file(file).unwrap();
    let scene = level.scene();
    let scene = scene.read();
    let world = &scene.world;

    let sun = world.entity_for("sun").unwrap();
    let (instance, light) =
        pulsar_scene_model::attachments::single_enabled_component_of::<LightComponent>(world, sun)
            .unwrap()
            .expect("hydrated");
    assert_eq!(light.intensity.intensity, 750.0);
    assert!(
        world.get::<LightComponent>(sun).is_none(),
        "the value lives on its instance entity, not the object"
    );
    assert!(
        world
            .get::<lights_component::LightComponentGpuMirror>(instance)
            .is_none(),
        "the GPU companion is the light's own GPU row, never a component"
    );
}

/// Pulsar-Native#1035, Phase 3: a light saved with `intensity` as a
/// bare number (the flat property form; the class nests it in its
/// `IntensityLightProps` group) is migrated at load and hydrates live,
/// keeping the saved value, instead of failing the level.
#[test]
fn a_legacy_flat_light_is_migrated_and_loads() {
    let mut data = serde_json::to_value(LightComponent::default()).unwrap();
    data["intensity"] = serde_json::json!(1002.0);
    let instances = serde_json::json!([
        { "index": 0, "class_name": "LightComponent", "data": data }
    ]);
    let file = level_with_sun_components(Value::Null, Some(instances));
    let level = RuntimeLevel::from_scene_file(file).expect("the migrated light loads");
    let scene = level.scene();
    let scene = scene.read();
    let world = &scene.world;
    let sun = world.entity_for("sun").unwrap();
    let lights = pulsar_scene_model::attachments::instances(world, sun);
    let light = world.get::<LightComponent>(lights[0]).expect("live");
    assert_eq!(light.intensity.intensity, 1002.0);
}

/// Persistence compatibility (Pulsar-Native#1035 acceptance, #1081): a
/// registered component whose data does not decode fails the load with
/// an error naming the object, the component class and the property.
#[test]
fn a_load_error_names_object_component_and_property() {
    let mut data = serde_json::to_value(LightComponent::default()).unwrap();
    data["intensity"]["intensity"] = serde_json::json!("bright");
    let instances = serde_json::json!([
        { "index": 0, "class_name": "LightComponent", "data": data }
    ]);
    let file = level_with_sun_components(Value::Null, Some(instances));
    let error = RuntimeLevel::from_scene_file(file)
        .err()
        .expect("an undecodable light refuses the level");
    let RuntimeLevelError::ComponentHydration {
        object_id,
        class_name,
        message,
    } = &error
    else {
        panic!("unexpected error: {error}");
    };
    assert_eq!(object_id, "sun");
    assert_eq!(class_name, "LightComponent");
    assert!(
        message.contains("intensity.intensity: invalid type"),
        "{message}"
    );
    let shown = error.to_string();
    for part in ["sun", "LightComponent", "intensity.intensity"] {
        assert!(shown.contains(part), "{shown}");
    }
}

/// #637: unregistered classes stay attached as unresolved JSON.
#[test]
fn unregistered_classes_stay_metadata_json() {
    let instances = serde_json::json!([
        { "index": 0, "class_name": "NotARealComponent", "data": { "x": 1 } }
    ]);
    let file = level_with_sun_components(Value::Null, Some(instances));
    let level = RuntimeLevel::from_scene_file(file).unwrap();
    let scene = level.scene();
    let scene = scene.read();
    let world = &scene.world;

    let sun = world.entity_for("sun").unwrap();
    let records = records(world, "sun");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].class_name, "NotARealComponent");
    assert_eq!(records[0].data["x"], serde_json::json!(1));
    let instance = pulsar_scene_model::attachments::instances(world, sun)[0];
    assert!(
        world
            .get::<pulsar_scene_model::UnresolvedComponent>(instance)
            .is_some()
    );
    assert!(lights(world, "sun").is_empty());
}

/// #637: a non-empty persisted `components` map is authoritative over
/// per-object `component_instances`; disabled records attach disabled.
#[test]
fn persisted_components_map_wins_and_respects_enabled() {
    let mut disabled = LightComponent::default();
    disabled.general.enabled = true;
    disabled.intensity.intensity = 42.0;
    let mut enabled = LightComponent::default();
    enabled.general.enabled = true;
    enabled.intensity.intensity = 99.0;
    // Inline instance data that must LOSE to the persisted map.
    let stale_inline = light_instances_json(1111.0);

    let components = serde_json::json!({
        "sun": [
            { "index": 0, "class_name": "LightComponent", "data": serde_json::to_value(&disabled).unwrap(), "enabled": false },
            { "index": 1, "class_name": "LightComponent", "data": serde_json::to_value(&enabled).unwrap(), "enabled": true }
        ]
    });
    let file = level_with_sun_components(components, Some(stale_inline));
    let level = RuntimeLevel::from_scene_file(file).unwrap();
    let scene = level.scene();
    let scene = scene.read();
    let world = &scene.world;

    let enabled = lights(world, "sun");
    assert_eq!(enabled.len(), 1, "persisted map drove hydration");
    assert_eq!(
        enabled[0].intensity.intensity, 99.0,
        "only the enabled record is live"
    );
    let records = records(world, "sun");
    assert_eq!(records.len(), 2, "the disabled record stays attached");
    assert!(!records[0].enabled && records[1].enabled);
}

#[test]
fn persisted_empty_component_list_is_authoritative() {
    let file = level_with_sun_components(
        serde_json::json!({ "sun": [] }),
        Some(light_instances_json(1111.0)),
    );
    let level = RuntimeLevel::from_scene_file(file).unwrap();
    let scene = level.scene();
    let scene = scene.read();
    let world = &scene.world;
    let sun = world.entity_for("sun").unwrap();

    assert!(
        pulsar_scene_model::attachments::instances(world, sun).is_empty(),
        "an explicit empty persisted list removes the inline component"
    );
}

#[test]
fn persisted_component_order_and_enabled_state_are_kept_on_the_instances() {
    let mut disabled = LightComponent::default();
    disabled.general.enabled = true;
    let mut enabled = LightComponent::default();
    enabled.general.enabled = true;
    enabled.intensity.intensity = 99.0;

    let file = level_with_sun_components(
        serde_json::json!({
            "sun": [
                {
                    "index": 7,
                    "class_name": "LightComponent",
                    "data": serde_json::to_value(&disabled).unwrap(),
                    "enabled": false
                },
                {
                    "index": 3,
                    "class_name": "NotARealComponent",
                    "data": { "x": 1 },
                    "enabled": true
                },
                {
                    "index": 11,
                    "class_name": "LightComponent",
                    "data": serde_json::to_value(&enabled).unwrap(),
                    "enabled": true
                }
            ]
        }),
        Some(light_instances_json(1111.0)),
    );
    let level = RuntimeLevel::from_scene_file(file).unwrap();
    let scene = level.scene();
    let scene = scene.read();
    let world = &scene.world;
    let records = records(world, "sun");

    let shape: Vec<_> = records
        .iter()
        .map(|record| (record.class_name.as_str(), record.enabled))
        .collect();
    assert_eq!(
        shape,
        [
            ("LightComponent", false),
            ("NotARealComponent", true),
            ("LightComponent", true),
        ]
    );
    assert_eq!(
        records[2].data["intensity"]["intensity"],
        serde_json::json!(99.0)
    );
}

/// #921: an old level whose Blueprint object carries
/// `ScriptComponent { script_asset }` loads as a `ClassInstance` with the
/// class's prefab components built on it (the script driver follows
/// that component, #922).
#[test]
fn legacy_script_component_levels_load_as_class_instances() {
    let project = tempfile::tempdir().unwrap();
    let class_dir = project.path().join("src").join("classes").join("Lamp");
    std::fs::create_dir_all(&class_dir).unwrap();
    std::fs::write(class_dir.join("graph_save.json"), "{}").unwrap();
    let mut light = LightComponent::default();
    light.intensity.intensity = 42.0;
    std::fs::write(
        class_dir.join("prefab.json"),
        serde_json::json!({
            "prefab_version": 1, "name": "Lamp",
            "components": [
                { "class_name": "LightComponent", "enabled": true, "data": serde_json::to_value(&light).unwrap() }
            ]
        })
        .to_string(),
    )
    .unwrap();
    let registry = pulsar_class::ClassRegistry::scan(project.path());

    let level_path = project.path().join("old.level");
    std::fs::write(
        &level_path,
        serde_json::json!({
            "version": "2.1",
            "objects": [
                { "id": "lamp", "name": "Lamp", "object_type": "Blueprint", "props": {},
                  "transform": { "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] } }
            ],
            "components": {
                "lamp": [ { "class_name": "ScriptComponent", "enabled": true,
                            "data": { "script_asset": "C:/elsewhere/src/classes/Lamp" } } ]
            }
        })
        .to_string(),
    )
    .unwrap();

    let level = RuntimeLevel::load_with_classes(&level_path, &registry).expect("old level loads");
    let scene = level.scene();
    let scene = scene.read();
    let world = &scene.world;
    let lamp = world.entity_for("lamp").unwrap();
    let instance =
        pulsar_class::world::class_instance_of(world, lamp).expect("migrated to ClassInstance");
    assert_eq!(instance.class_name, "Lamp");
    assert!(!instance.class.is_empty(), "resolved to the class GUID");
    assert_eq!(
        lights(world, "lamp")
            .iter()
            .map(|light| light.intensity.intensity)
            .collect::<Vec<_>>(),
        [42.0],
        "prefab component built on the placed object"
    );
}

/// One directional light per level: a file with two loads with the
/// first casting and the second kept as authored, its instance
/// disabled.
#[test]
fn a_level_with_two_directional_lights_loads_with_one() {
    use lights_component::LightType;
    let mut sun = LightComponent::default();
    sun.general.light_type = LightType::Directional;
    let data = serde_json::to_value(&sun).unwrap();
    let instances = serde_json::json!([
        { "index": 0, "class_name": "LightComponent", "data": data },
        { "index": 1, "class_name": "LightComponent", "data": data }
    ]);
    let file = level_with_sun_components(Value::Null, Some(instances));
    let level = RuntimeLevel::from_scene_file(file).unwrap();
    let scene = level.scene();
    let scene = scene.read();
    let world = &scene.world;
    let lights =
        pulsar_scene_model::attachments::instances(world, world.entity_for("sun").unwrap());
    assert_eq!(lights_component::directional_lights(world), [lights[0]]);
    assert!(!pulsar_scene_model::attachments::is_enabled(
        world, lights[1]
    ));
    let kept = world.get::<LightComponent>(lights[1]).unwrap();
    assert_eq!(kept.general.light_type, LightType::Directional);
}
