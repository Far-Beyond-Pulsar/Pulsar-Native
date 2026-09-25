//! Class instances in a real World: instantiation, slot lookup, override
//! round trips and unresolved classes (Pulsar-Native#921).

use engine_class_derive::{engine_class, register_runtime_behavior, register_world_component};
use pulsar_class::world::{
    class_instance_of, collect_overrides, expand_all, generated_children, instantiate_class,
    is_generated_child, placement, slot_map, store_class_instance,
};
use pulsar_class::{ClassInstance, ClassRegistry, CLASS_INSTANCE};
use pulsar_reflection::{ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner};
use pulsar_scene_model::{ComponentInstance, SceneWorldExt, SpawnObject, Transform};
use pulsar_scenedb::{Entity, World};
use serde_json::json;

#[engine_class(category = "Test", default, clone, debug, serialize, deserialize)]
pub struct TestLamp {
    #[property]
    pub intensity: f32,
    #[property]
    pub label: String,
}

#[register_world_component]
#[register_runtime_behavior]
impl ComponentRuntimeBehavior for TestLamp {
    const CLASS_NAME: &'static str = "TestLamp";
    fn sync_component(
        _owner: &RuntimeComponentOwner,
        _index: usize,
        _component: &Self,
        _context: &mut dyn ComponentRuntimeContext,
    ) {
    }
}

fn write_class(project: &std::path::Path, name: &str, prefab: serde_json::Value) {
    let dir = project.join("src").join("classes").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("graph_save.json"), "{}").unwrap();
    std::fs::write(
        dir.join("prefab.json"),
        serde_json::to_string_pretty(&prefab).unwrap(),
    )
    .unwrap();
}

/// Slot UUID of prefab component `index`.
fn slot(def: &pulsar_class::ClassDefinition, index: usize) -> String {
    def.prefab.components[index].slot_id.clone()
}

/// Edit the first component's defaults in place, keeping slot ids (what the
/// Blueprint editor does on save).
fn edit_lamp_default(project: &std::path::Path, intensity: f64, label: &str) {
    let path = project.join("src/classes/Lamp/prefab.json");
    let mut prefab: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    prefab["components"][0]["data"] = json!({ "intensity": intensity, "label": label });
    std::fs::write(&path, prefab.to_string()).unwrap();
}

fn lamp_prefab(intensity: f64, label: &str) -> serde_json::Value {
    json!({
        "prefab_version": 1,
        "name": "Lamp",
        "components": [
            { "class_name": "TestLamp", "enabled": true, "data": { "intensity": intensity, "label": label } },
            { "class_name": "TestLamp", "enabled": true, "data": { "intensity": 2.0, "label": "second" } }
        ],
        "blueprint_class": { "class_path": "", "variable_defaults": { "speed": "5.0" } }
    })
}

fn spawn_at(name: &str, x: f32) -> SpawnObject {
    SpawnObject::new(name)
        .with_id(name)
        .with_transform(Transform {
            position: [x, 0.0, 0.0],
            ..Default::default()
        })
}

fn lamp(world: &World, entity: Entity) -> TestLamp {
    world
        .get::<TestLamp>(entity)
        .cloned()
        .expect("typed TestLamp")
}

#[test]
fn two_components_of_one_type_put_the_second_on_a_child() {
    let project = tempfile::tempdir().unwrap();
    write_class(project.path(), "Lamp", lamp_prefab(1.0, "first"));
    let registry = ClassRegistry::scan(project.path());
    let def = registry.by_name("Lamp").unwrap().load_definition().unwrap();

    let mut world = World::new();
    let a = instantiate_class(
        &mut world,
        &def,
        ClassInstance::default(),
        spawn_at("a", 1.0),
    )
    .unwrap()
    .root();
    let b = instantiate_class(
        &mut world,
        &def,
        ClassInstance::default(),
        spawn_at("b", 5.0),
    )
    .unwrap()
    .root();

    for root in [a, b] {
        let instance = class_instance_of(&world, root).unwrap();
        assert_eq!(instance.class, def.id, "root references the class GUID");
        assert_eq!(lamp(&world, root).intensity, 1.0, "first copy on the root");

        let children = generated_children(&world, root);
        assert_eq!(children.len(), 1, "second copy becomes a child entity");
        let child = children[0];
        assert!(is_generated_child(&world, child));
        assert_eq!(lamp(&world, child).label, "second");

        // Slot UUIDs resolve once into handles on this instance.
        let placed = placement(&world, root);
        assert_eq!(placed.handle(&slot(&def, 0)).unwrap().entity, root);
        assert_eq!(placed.handle(&slot(&def, 1)).unwrap().entity, child);
        assert_eq!(placed.children, [child]);
        assert_eq!(slot_map(&world, root).len(), 2);
    }
    // Children sit at their root's transform and have deterministic ids.
    let b_child = generated_children(&world, b)[0];
    assert_eq!(
        world.get::<Transform>(b_child).unwrap().position,
        [5.0, 0.0, 0.0]
    );
    assert_eq!(
        world.stable_id_of(b_child).map(str::to_string),
        Some(format!("b#{}", slot(&def, 1)))
    );
}

/// Save (overrides only), edit the class default, reload: overridden values
/// stay, everything else follows the class.
#[test]
fn overrides_survive_a_class_edit_and_the_rest_updates() {
    let project = tempfile::tempdir().unwrap();
    write_class(project.path(), "Lamp", lamp_prefab(1.0, "first"));
    let registry = ClassRegistry::scan(project.path());
    let def = registry.by_name("Lamp").unwrap().load_definition().unwrap();

    let mut world = World::new();
    let a = instantiate_class(
        &mut world,
        &def,
        ClassInstance::default(),
        spawn_at("a", 0.0),
    )
    .unwrap()
    .root();
    let b = instantiate_class(
        &mut world,
        &def,
        ClassInstance::default(),
        spawn_at("b", 0.0),
    )
    .unwrap()
    .root();

    // Edit instance a: one root component value, one child value, one variable.
    world.get_mut::<TestLamp>(a).unwrap().intensity = 9.0;
    let a_child = generated_children(&world, a)[0];
    world.get_mut::<TestLamp>(a_child).unwrap().label = "custom".into();
    let mut instance = class_instance_of(&world, a).unwrap();
    instance
        .variable_overrides
        .insert("speed".into(), json!(7.0));
    store_class_instance(&mut world, a, &instance);

    // "Save": only the diffs are recorded.
    let saved_a = collect_overrides(&world, a, &def);
    let saved_b = collect_overrides(&world, b, &def);
    assert_eq!(
        saved_a.component_overrides[&slot(&def, 0)],
        json!({ "intensity": 9.0 })
    );
    assert_eq!(
        saved_a.component_overrides[&slot(&def, 1)],
        json!({ "label": "custom" })
    );
    assert_eq!(saved_a.variable_overrides["speed"], json!(7.0));
    assert!(
        saved_b.component_overrides.is_empty(),
        "untouched instance stores nothing"
    );
    assert!(saved_b.variable_overrides.is_empty());

    // A variable set back to its default is not an override.
    let mut same = saved_b.clone();
    same.variable_overrides.insert("speed".into(), json!(5.0));
    store_class_instance(&mut world, b, &same);
    assert!(collect_overrides(&world, b, &def)
        .variable_overrides
        .is_empty());

    // Edit the class default, then "load" a fresh world from the saved data.
    edit_lamp_default(project.path(), 3.0, "renamed");
    let registry = ClassRegistry::scan(project.path());
    let mut loaded = World::new();
    for (id, saved) in [("a", &saved_a), ("b", &saved_b)] {
        let root = loaded
            .spawn_object(SpawnObject::new(id).with_id(id))
            .unwrap();
        pulsar_class::world::attach_components(
            &mut loaded,
            root,
            vec![ComponentInstance {
                class_name: CLASS_INSTANCE.into(),
                enabled: true,
                data: saved.to_value(),
            }],
        );
    }
    let report = expand_all(&mut loaded, &registry);
    assert!(report.unresolved.is_empty());

    let a = loaded.entity_for("a").unwrap();
    let b = loaded.entity_for("b").unwrap();
    assert_eq!(lamp(&loaded, a).intensity, 9.0, "override kept");
    assert_eq!(
        lamp(&loaded, a).label,
        "renamed",
        "non-overridden value follows the class"
    );
    assert_eq!(
        lamp(&loaded, b).intensity,
        3.0,
        "class edit reaches the other instance"
    );
    assert_eq!(
        lamp(&loaded, generated_children(&loaded, a)[0]).label,
        "custom"
    );
    assert_eq!(
        lamp(&loaded, generated_children(&loaded, b)[0]).label,
        "second"
    );
    assert_eq!(
        class_instance_of(&loaded, a).unwrap().variable_overrides["speed"],
        json!(7.0)
    );
}

#[test]
fn a_missing_class_stays_unresolved_without_losing_data() {
    let project = tempfile::tempdir().unwrap();
    let registry = ClassRegistry::scan(project.path());
    let mut world = World::new();
    let root = world
        .spawn_object(SpawnObject::new("x").with_id("x"))
        .unwrap();
    let mut instance = ClassInstance::new("guid-gone".into(), "Gone");
    instance.variable_overrides.insert("hp".into(), json!(3));
    instance
        .component_overrides
        .insert("TestLamp_0".into(), json!({ "intensity": 4.0 }));
    pulsar_class::world::attach_components(
        &mut world,
        root,
        vec![ComponentInstance {
            class_name: CLASS_INSTANCE.into(),
            enabled: true,
            data: instance.to_value(),
        }],
    );
    let report = expand_all(&mut world, &registry);
    assert_eq!(report.unresolved, ["x"]);
    let kept = class_instance_of(&world, root).unwrap();
    assert_eq!(kept.class_name, "Gone");
    assert_eq!(kept.variable_overrides["hp"], json!(3));
    assert_eq!(
        kept.component_overrides["TestLamp_0"],
        json!({ "intensity": 4.0 })
    );
    assert!(generated_children(&world, root).is_empty());
}

#[test]
fn removed_slots_are_recorded_and_respected() {
    let project = tempfile::tempdir().unwrap();
    write_class(project.path(), "Lamp", lamp_prefab(1.0, "first"));
    let registry = ClassRegistry::scan(project.path());
    let def = registry.by_name("Lamp").unwrap().load_definition().unwrap();
    let mut world = World::new();
    let root = instantiate_class(
        &mut world,
        &def,
        ClassInstance::default(),
        spawn_at("r", 0.0),
    )
    .unwrap()
    .root();
    let child = generated_children(&world, root)[0];
    world.despawn_tree(child);
    let saved = collect_overrides(&world, root, &def);
    assert_eq!(
        saved.component_overrides[&slot(&def, 1)],
        json!({ "__removed": true })
    );

    store_class_instance(&mut world, root, &saved);
    pulsar_class::world::expand_class_instance(&mut world, root, &def);
    assert!(
        generated_children(&world, root).is_empty(),
        "removed slot is not rebuilt"
    );
    assert_eq!(lamp(&world, root).intensity, 1.0);
}
