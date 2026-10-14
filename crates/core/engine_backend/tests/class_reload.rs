//! A world without the editor follows class edits (SceneDB#84): a Blueprint
//! asset update rebuilds every placed instance of the class from its new
//! definition, keeping the instance's own edits, both when the update is
//! applied to a world directly and when it arrives on the host bus for a
//! world a game loop holds a reload subscription for.
//!
//! One test, because the host bus is process-wide.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use engine_backend::scene::{
    attachments, classes, new_scene, reload_updated_asset, subscribe_asset_reloads, ObjectType,
    SceneWorldExt, SpawnObject,
};
use helio_component::components::LightComponent;
use pulsar_class::ClassInstance;
use pulsar_events::{AssetKind, AssetUpdated};
use pulsar_scenedb::{Entity, World};
use serde_json::{json, Value};

fn light_json(intensity: f32) -> Value {
    let mut light = LightComponent::default();
    light.intensity.intensity = intensity;
    serde_json::to_value(light).unwrap()
}

/// A project with a `name` class holding one LightComponent.
fn project_with_class(project: &Path, name: &str, intensity: f32) -> PathBuf {
    let dir = project.join("src").join("classes").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("graph_save.json"), "{}").unwrap();
    write_prefab(&dir, name, intensity);
    dir
}

/// Write the class's prefab, keeping the slot UUIDs a previous load
/// assigned (as the Blueprint editor does when it saves a class).
fn write_prefab(dir: &Path, name: &str, intensity: f32) {
    let slot_id = std::fs::read_to_string(dir.join("prefab.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|prefab| prefab["components"][0].get("slot_id").cloned());
    let mut prefab = json!({
        "prefab_version": 1,
        "name": name,
        "components": [
            { "class_name": "LightComponent", "enabled": true, "data": light_json(intensity) }
        ],
    });
    if let Some(slot_id) = slot_id {
        prefab["components"][0]["slot_id"] = slot_id;
    }
    std::fs::write(dir.join("prefab.json"), prefab.to_string()).unwrap();
}

fn place(world: &mut World, dir: &Path) -> Entity {
    let registry = classes::registry_for_class_dir(dir);
    let def = registry
        .resolve_script_asset(&dir.display().to_string())
        .expect("a class directory")
        .load_definition()
        .expect("a class definition");
    let spawn = SpawnObject::new(def.name.clone()).with_object_type(ObjectType::Blueprint);
    pulsar_class::world::instantiate_class(world, &def, ClassInstance::default(), spawn)
        .expect("placed")
        .root()
}

/// A level with one object, `lamp`, bound to class `name`, loaded the way a
/// game loads one (the class is built when the level hydrates).
fn load_level_with(project: &Path, name: &str) -> engine_backend::scene::RuntimeLevel {
    let file: pulsar_scene::format::SceneFile = serde_json::from_value(json!({
        "version": "2.1",
        "objects": [{
            "id": "lamp", "name": "Lamp", "object_type": "Empty",
            "transform": {"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]},
            "parent": null, "visible": true, "locked": false, "props": {}
        }],
        "components": {},
        "metadata": {},
        "blueprint_bindings": { "lamp": [{ "class_name": name, "overrides": {} }] }
    }))
    .unwrap();
    let registry = pulsar_class::ClassRegistry::scan(project);
    engine_backend::scene::RuntimeLevel::from_scene_file_with_classes(file, &registry)
        .expect("level loads")
}

fn light_entity(world: &World, root: Entity) -> Entity {
    attachments::enabled_components_of::<LightComponent>(world, root)
        .first()
        .expect("a LightComponent")
        .0
}

fn intensity(world: &World, root: Entity) -> f32 {
    world
        .get::<LightComponent>(light_entity(world, root))
        .unwrap()
        .intensity
        .intensity
}

fn class_update(dir: &Path) -> AssetUpdated {
    AssetUpdated::new(AssetKind::Blueprint).with_path(dir.to_path_buf())
}

#[test]
fn class_updates_rebuild_instances_in_worlds_without_the_editor() {
    let project = tempfile::tempdir().unwrap();
    let lamp = project_with_class(project.path(), "Lamp", 1.0);
    let other = project_with_class(project.path(), "Other", 1.0);

    // ── Applied to a world directly ────────────────────────────────────────
    let mut scene = new_scene();
    let world = &mut scene.world;
    let edited = place(world, &lamp);
    let untouched = place(world, &lamp);
    let unrelated = place(world, &other);
    assert_eq!(intensity(world, edited), 1.0);

    // The instance's own edit survives class rebuilds; the class default
    // reaches the instance that kept it.
    let light = light_entity(world, edited);
    world
        .get_mut::<LightComponent>(light)
        .unwrap()
        .intensity
        .intensity = 7.0;
    write_prefab(&lamp, "Lamp", 3.0);
    let rebuilt = reload_updated_asset(world, &class_update(&lamp));
    assert!(rebuilt >= 2, "both Lamp instances are rebuilt ({rebuilt})");
    assert_eq!(intensity(world, untouched), 3.0, "the new default");
    assert_eq!(intensity(world, edited), 7.0, "the instance's own edit");
    assert_eq!(
        intensity(world, unrelated),
        1.0,
        "another class is left alone"
    );

    // A second edit is diffed against the definition the instances were
    // rebuilt from, so the old default does not become an override.
    write_prefab(&lamp, "Lamp", 4.0);
    reload_updated_asset(world, &class_update(&lamp));
    assert_eq!(intensity(world, untouched), 4.0);
    assert_eq!(intensity(world, edited), 7.0);

    // An update for another class rebuilds only its instances.
    write_prefab(&other, "Other", 2.0);
    let rebuilt = reload_updated_asset(world, &class_update(&other));
    assert!(rebuilt >= 1);
    assert_eq!(intensity(world, unrelated), 2.0);
    assert_eq!(intensity(world, untouched), 4.0);

    // ── A level a game loaded ──────────────────────────────────────────────
    // Its instances record what they were built from as they load, so the
    // first class edit keeps the edits made since loading.
    let level = load_level_with(project.path(), "Lamp");
    let loaded_scene = level.scene();
    {
        let mut loaded = loaded_scene.write();
        let world = &mut loaded.world;
        let lamp_root = world.entity_for("lamp").expect("lamp loaded");
        assert_eq!(intensity(world, lamp_root), 4.0, "built from the class");
        let light = light_entity(world, lamp_root);
        world
            .get_mut::<LightComponent>(light)
            .unwrap()
            .intensity
            .intensity = 8.0;
        write_prefab(&lamp, "Lamp", 4.5);
        reload_updated_asset(world, &class_update(&lamp));
        assert_eq!(
            intensity(world, lamp_root),
            8.0,
            "the edit made after loading survives the first class edit"
        );
    }

    // ── Through the host bus, for a world a game loop holds ────────────────
    let game: engine_backend::scene::SharedScene = Arc::new(parking_lot::RwLock::new(new_scene()));
    let placed = place(&mut game.write().world, &lamp);
    let subscription = subscribe_asset_reloads(&game);
    write_prefab(&lamp, "Lamp", 5.0);
    pulsar_events::publish_asset_updated(class_update(&lamp));
    assert_eq!(
        intensity(&game.read().world, placed),
        5.0,
        "the game world rebuilt its instance with no editor running"
    );

    // Dropping the subscription stops it.
    drop(subscription);
    write_prefab(&lamp, "Lamp", 6.0);
    pulsar_events::publish_asset_updated(class_update(&lamp));
    assert_eq!(intensity(&game.read().world, placed), 5.0);
}
