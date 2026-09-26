//! Placed class instances in the editor (#921): placement, save/load by
//! reference with overrides, migration, duplicate, history, details data.

use std::path::{Path, PathBuf};

use engine_backend::scene::{new_scene, SceneWorldExt};
use helio_component::components::LightComponent;
use pulsar_class::ClassRegistry;
use pulsar_scenedb::World;
use serde_json::{json, Value};

use super::super::{classes, components, history, level_io, objects, Transform};

fn light_json(intensity: f32) -> Value {
    let mut light = LightComponent::default();
    light.intensity.intensity = intensity;
    serde_json::to_value(light).unwrap()
}

/// A project with a `Lamp` class holding two LightComponents.
fn project_with_lamp(intensity: f32) -> (tempfile::TempDir, PathBuf) {
    let project = tempfile::tempdir().unwrap();
    let dir = project.path().join("src").join("classes").join("Lamp");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("graph_save.json"), "{}").unwrap();
    write_lamp_prefab(&dir, intensity);
    (project, dir)
}

/// Write the Lamp prefab, keeping the slot UUIDs a previous load assigned
/// (as the Blueprint editor does when it saves a class).
fn write_lamp_prefab(dir: &Path, intensity: f32) {
    let existing: Vec<Value> = std::fs::read_to_string(dir.join("prefab.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["components"].as_array().cloned())
        .unwrap_or_default();
    let mut prefab = json!({
        "prefab_version": 1,
        "name": "Lamp",
        "components": [
            { "class_name": "LightComponent", "enabled": true, "data": light_json(intensity) },
            { "class_name": "LightComponent", "enabled": true, "data": light_json(2.0) }
        ],
        "blueprint_class": { "class_path": "", "variable_defaults": { "speed": "5.0" } }
    });
    for (i, old) in existing.iter().enumerate() {
        if let Some(id) = old.get("slot_id") {
            prefab["components"][i]["slot_id"] = id.clone();
        }
    }
    std::fs::write(dir.join("prefab.json"), prefab.to_string()).unwrap();
}

/// Slot UUID of Lamp prefab component `index` (assigned on first load).
fn slot(dir: &Path, index: usize) -> String {
    let prefab: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("prefab.json")).unwrap()).unwrap();
    prefab["components"][index]["slot_id"]
        .as_str()
        .expect("slot id assigned")
        .to_string()
}

fn light(world: &World, id: &str) -> LightComponent {
    let entity = world.entity_for(id).unwrap();
    world
        .get::<LightComponent>(entity)
        .cloned()
        .expect("LightComponent")
}

fn children(world: &World, id: &str) -> Vec<String> {
    objects::get_children(world, id)
        .into_iter()
        .filter(|child| classes::is_generated_child(world, child))
        .collect()
}

fn place(world: &mut World, dir: &Path, x: f32) -> String {
    let transform = Transform {
        position: [x, 0.0, 0.0],
        ..Default::default()
    };
    classes::instantiate_class_dir(world, dir, &transform, None).unwrap()[0].clone()
}

#[test]
fn placing_a_class_twice_gives_two_full_instances() {
    let (_project, dir) = project_with_lamp(1.0);
    let mut scene = new_scene();
    let world = &mut scene.world;
    let a = place(world, &dir, 0.0);
    let b = place(world, &dir, 4.0);
    assert_ne!(a, b);
    for id in [&a, &b] {
        assert!(classes::is_class_root(world, id));
        assert_eq!(light(world, id).intensity.intensity, 1.0);
        let kids = children(world, id);
        assert_eq!(kids.len(), 1, "second LightComponent on a child object");
        assert_eq!(light(world, &kids[0]).intensity.intensity, 2.0);
        // The editor's component list carries the slot markers.
        let names = components::get_component_class_names(world, id);
        assert_eq!(names, ["ClassInstance", "LightComponent"]);
    }
    // Moving the root moves its generated child.
    objects::set_transform(world, &b, Some([9.0, 0.0, 0.0]), None, None);
    let kid = &children(world, &b)[0];
    assert_eq!(
        objects::get_object_transform(world, kid).unwrap().position,
        [9.0, 0.0, 0.0]
    );
}

#[test]
fn save_writes_only_overrides_and_load_follows_class_edits() {
    let (project, dir) = project_with_lamp(1.0);
    let registry = ClassRegistry::scan(project.path());
    let mut scene = new_scene();
    let world = &mut scene.world;
    let a = place(world, &dir, 0.0);
    let b = place(world, &dir, 1.0);

    // Override one value on a, and a variable.
    let entity = world.entity_for(&a).unwrap();
    world
        .get_mut::<LightComponent>(entity)
        .unwrap()
        .intensity
        .intensity = 9.0;
    assert!(classes::set_variable(
        world,
        &a,
        "speed",
        json!(7.0),
        &registry
    ));

    let level = project.path().join("test.level");
    level_io::save_with_classes(world, &level, None, &registry).unwrap();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&level).unwrap()).unwrap();

    // Generated children are not saved; roots carry only the class reference
    // and differences.
    assert_eq!(saved["objects"].as_array().unwrap().len(), 2);
    let a_components = saved["components"][&a].as_array().unwrap();
    assert_eq!(
        a_components.len(),
        1,
        "slot components are rebuilt, not saved"
    );
    let a_instance = &a_components[0]["data"];
    assert_eq!(
        a_instance["component_overrides"],
        json!({ slot(&dir, 0): { "intensity": { "intensity": 9.0 } } })
    );
    assert_eq!(a_instance["variable_overrides"], json!({ "speed": 7.0 }));
    let b_instance = &saved["components"][&b][0]["data"];
    assert!(b_instance.get("component_overrides").is_none());
    assert!(saved.get("blueprint_bindings").is_none());

    // Edit the class default, reload.
    write_lamp_prefab(&dir, 3.0);
    let mut reloaded = new_scene();
    let world = &mut reloaded.world;
    level_io::load_with_classes(world, &level, &registry).unwrap();
    assert_eq!(light(world, &a).intensity.intensity, 9.0, "override kept");
    assert_eq!(
        light(world, &b).intensity.intensity,
        3.0,
        "class edit reaches b"
    );
    assert_eq!(children(world, &a).len(), 1);
    assert_eq!(
        classes::class_instance(world, &a)
            .unwrap()
            .variable_overrides["speed"],
        json!(7.0)
    );

    // Saving again without edits writes the same overrides.
    level_io::save_with_classes(world, &level, None, &registry).unwrap();
    let again: Value = serde_json::from_str(&std::fs::read_to_string(&level).unwrap()).unwrap();
    assert_eq!(
        again["components"][&a][0]["data"],
        saved["components"][&a][0]["data"]
    );
}

#[test]
fn old_levels_migrate_on_load_and_save_without_the_old_fields() {
    let (project, _dir) = project_with_lamp(1.0);
    let registry = ClassRegistry::scan(project.path());
    let level = project.path().join("old.level");
    let object = |id: &str, ty: &str| {
        json!({
            "id": id, "name": id, "object_type": ty,
            "transform": { "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] },
            "visible": true, "locked": false, "parent": null, "children": [], "scene_path": id, "props": {}
        })
    };
    std::fs::write(
        &level,
        json!({
            "version": "2.1",
            "objects": [object("dropped", "Blueprint"), object("bound", "Empty")],
            "components": {
                "dropped": [ { "class_name": "ScriptComponent", "enabled": true,
                               "data": { "script_asset": "D:/other/machine/src/classes/Lamp" } } ]
            },
            "blueprint_bindings": { "bound": [ { "class_name": "Lamp", "overrides": { "speed": 1.5 } } ] },
            "metadata": { "created": "", "modified": "", "editor_version": "" }
        })
        .to_string(),
    )
    .unwrap();

    let mut scene = new_scene();
    let world = &mut scene.world;
    level_io::load_with_classes(world, &level, &registry).unwrap();
    for id in ["dropped", "bound"] {
        assert!(classes::is_class_root(world, id), "{id} migrated");
        assert_eq!(
            light(world, id).intensity.intensity,
            1.0,
            "{id} has the class components"
        );
        assert!(!components::get_component_class_names(world, id)
            .contains(&"ScriptComponent".to_string()));
    }
    assert_eq!(
        classes::class_instance(world, "bound")
            .unwrap()
            .variable_overrides["speed"],
        json!(1.5)
    );

    level_io::save_with_classes(world, &level, None, &registry).unwrap();
    let text = std::fs::read_to_string(&level).unwrap();
    assert!(!text.contains("ScriptComponent"));
    assert!(!text.contains("blueprint_bindings"));
    assert!(!text.contains("script_asset"));
}

#[test]
fn duplicate_and_history_keep_the_class_link() {
    let (project, dir) = project_with_lamp(1.0);
    let registry = ClassRegistry::scan(project.path());
    let mut scene = new_scene();
    let world = &mut scene.world;
    let a = place(world, &dir, 0.0);
    let entity = world.entity_for(&a).unwrap();
    world
        .get_mut::<LightComponent>(entity)
        .unwrap()
        .intensity
        .intensity = 6.0;

    // Class-aware duplicate: a new full instance with the same overrides.
    let copy = classes::duplicate_instance_with(world, &a, &registry).unwrap();
    assert!(classes::is_class_root(world, &copy));
    assert_eq!(light(world, &copy).intensity.intensity, 6.0);
    assert_eq!(children(world, &copy).len(), 1);

    // Without a resolvable class the plain copy still keeps the link.
    let plain = objects::duplicate_object(world, &a).unwrap();
    assert!(classes::class_instance(world, &plain).is_some());

    // Undo/redo snapshots restore roots and generated children.
    let snapshot = history::capture_history_snapshot(world);
    objects::clear(world);
    history::restore_history_snapshot(world, &snapshot).unwrap();
    assert!(classes::is_class_root(world, &a));
    assert_eq!(children(world, &a).len(), 1);
    assert_eq!(light(world, &a).intensity.intensity, 6.0);
    assert_eq!(
        classes::current_overrides(world, &a, &registry)
            .unwrap()
            .component_overrides[&slot(&dir, 0)],
        json!({ "intensity": { "intensity": 6.0 } })
    );
}

#[test]
fn details_view_marks_overrides_and_reverts_them() {
    let (project, dir) = project_with_lamp(1.0);
    let registry = ClassRegistry::scan(project.path());
    let mut scene = new_scene();
    let world = &mut scene.world;
    let a = place(world, &dir, 0.0);
    let entity = world.entity_for(&a).unwrap();
    world
        .get_mut::<LightComponent>(entity)
        .unwrap()
        .intensity
        .intensity = 9.0;
    classes::set_variable(world, &a, "speed", json!(8.0), &registry);

    let view = classes::class_instance_view(world, &a, &registry).unwrap();
    assert!(view.resolved);
    assert_eq!(view.class_name, "Lamp");
    let speed = view.variables.iter().find(|v| v.name == "speed").unwrap();
    assert!(speed.overridden);
    assert_eq!(speed.value, json!(8.0));
    assert_eq!(view.slots.len(), 2);
    let root_slot = &view.slots[0];
    assert_eq!(root_slot.object_id.as_deref(), Some(a.as_str()));
    assert_eq!(root_slot.overridden.len(), 1);
    assert_eq!(root_slot.overridden[0].path, "intensity.intensity");
    assert_eq!(root_slot.overridden[0].default, json!(1.0));
    assert!(view.slots[1].overridden.is_empty());

    assert!(classes::revert_slot(
        world,
        &a,
        &slot(&dir, 0),
        Some("intensity.intensity"),
        &registry
    ));
    assert_eq!(light(world, &a).intensity.intensity, 1.0);
    assert!(classes::revert_variable(world, &a, "speed"));
    let view = classes::class_instance_view(world, &a, &registry).unwrap();
    assert!(view.slots.iter().all(|s| s.overridden.is_empty()));
    assert!(view.variables.iter().all(|v| !v.overridden));
}

/// #921: publishing `AssetUpdated` for a class rebuilds every placed
/// instance from the new definition, keeping each instance's overrides
/// (including unsaved edits), and queues the event for a running game.
#[test]
fn class_asset_updates_rebuild_placed_instances() {
    use crate::level_editor::core::asset_updates;
    use crate::level_editor::state::LevelEditorState;
    use plugin_editor_api::{publish_asset_updated, AssetKind, AssetUpdated};

    let (_project, dir) = project_with_lamp(1.0);
    let state = std::sync::Arc::new(parking_lot::RwLock::new(LevelEditorState::new()));
    let (a, b) = {
        let st = state.read();
        let mut world = st.scene.world_mut();
        let a = place(&mut world, &dir, 0.0);
        let b = place(&mut world, &dir, 1.0);
        // An unsaved edit on a.
        let entity = world.entity_for(&a).unwrap();
        world
            .get_mut::<LightComponent>(entity)
            .unwrap()
            .intensity
            .intensity = 9.0;
        (a, b)
    };
    state.write().play.pie.active = true;
    let _subscription = asset_updates::subscribe_class_updates(state.clone());

    // The Blueprint editor saves the class with a new default, then publishes.
    write_lamp_prefab(&dir, 3.0);
    let event = AssetUpdated::new(AssetKind::Blueprint).with_path(dir.clone());
    publish_asset_updated(event.clone());

    let st = state.read();
    let world = st.scene.world();
    assert_eq!(
        light(&world, &a).intensity.intensity,
        9.0,
        "the instance's own edit is kept"
    );
    assert_eq!(
        light(&world, &b).intensity.intensity,
        3.0,
        "the new class default reaches b"
    );
    assert_eq!(
        children(&world, &a).len(),
        1,
        "generated children rebuilt, not duplicated"
    );
    assert_eq!(
        st.play.pie.pending_asset_updates,
        [event],
        "forwarded to the running game"
    );
}

/// #935: a class edit reaches every placed instance's LIVE components and
/// the renderer hears about it: after `AssetUpdated`, both instances' lights
/// (root and generated child) have the new color, and each light entity is
/// in the change events the renderer drains to re-derive its light rows,
/// without touching any instance.
#[test]
fn class_color_edit_reaches_live_lights_and_their_render_rows() {
    use crate::level_editor::core::asset_updates;
    use crate::level_editor::state::LevelEditorState;
    use plugin_editor_api::{AssetKind, AssetUpdated};

    let (_project, dir) = project_with_lamp(1.0);
    let state = std::sync::Arc::new(parking_lot::RwLock::new(LevelEditorState::new()));
    let (a, b) = {
        let st = state.read();
        let mut world = st.scene.world_mut();
        let a = place(&mut world, &dir, 0.0);
        let b = place(&mut world, &dir, 4.0);
        // The renderer armed its row subscriptions and drained its events.
        engine_backend::scene::arm_render_row_subscriptions(&mut world);
        world.take_component_change_events();
        (a, b)
    };
    let classes_before = state.read().scene.class_updates;

    // The class's lights turn red.
    let mut prefab: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("prefab.json")).unwrap()).unwrap();
    for component in prefab["components"].as_array_mut().unwrap() {
        component["data"]["color"]["color"] = json!([1.0, 0.0, 0.0, 1.0]);
    }
    std::fs::write(dir.join("prefab.json"), prefab.to_string()).unwrap();
    let touched = asset_updates::handle_asset_update(
        &state,
        &AssetUpdated::new(AssetKind::Blueprint).with_path(dir.clone()),
    );
    assert!(!touched.is_empty());
    assert_ne!(state.read().scene.class_updates, classes_before, "the editor is woken");

    let st = state.read();
    let mut world = st.scene.world_mut();
    let dirty: std::collections::HashSet<_> =
        world.take_component_change_events().into_iter().map(|e| e.entity).collect();
    for id in [&a, &b] {
        let kids = children(&world, id);
        assert_eq!(kids.len(), 1);
        for object in [id, &kids[0]] {
            assert_eq!(light(&world, object).color.color, [1.0, 0.0, 0.0, 1.0], "{object}: live light");
            let entity = world.entity_for(object).unwrap();
            assert!(dirty.contains(&entity), "{object}: the renderer re-derives its light row");
        }
    }
    // What the renderer does with them: rows follow the live lights.
    engine_backend::scene::sync_editor_light_rows(&mut world, true, Some(&dirty));
}

/// Reverting a class-slot property and setting/reverting a class variable
/// go through the normal command path, so undo brings the override back.
#[test]
fn reverts_and_variable_edits_are_undoable_commands() {
    use crate::level_editor::commands::{execute_command, SceneCommand};
    use crate::level_editor::state::LevelEditorState;

    let (project, dir) = project_with_lamp(1.0);
    classes::set_fallback_project_root(Some(project.path().to_path_buf()));
    let mut state = LevelEditorState::new();
    let a = {
        let mut world = state.scene.world_mut();
        let a = place(&mut world, &dir, 0.0);
        let entity = world.entity_for(&a).unwrap();
        world
            .get_mut::<LightComponent>(entity)
            .unwrap()
            .intensity
            .intensity = 9.0;
        a
    };
    let index = components::get_component_class_names(&state.scene.world(), &a)
        .iter()
        .position(|c| c == "LightComponent")
        .unwrap();

    // Revert the slot property to the class default.
    let result = execute_command(
        &mut state,
        SceneCommand::RevertComponentProperty {
            id: a.clone(),
            class_name: "LightComponent".into(),
            component_index: index,
            prop_name: "intensity".into(),
        },
    );
    assert!(result.changed, "{}", result.no_op_reason);
    assert_eq!(light(&state.scene.world(), &a).intensity.intensity, 1.0);
    state.scene.undo();
    assert_eq!(
        light(&state.scene.world(), &a).intensity.intensity,
        9.0,
        "undo restores the override"
    );

    // Class variable: set, revert, undo.
    execute_command(
        &mut state,
        SceneCommand::SetClassVariable {
            id: a.clone(),
            name: "speed".into(),
            value: Some(json!(8.0)),
        },
    );
    assert_eq!(
        classes::class_instance(&state.scene.world(), &a)
            .unwrap()
            .variable_overrides["speed"],
        json!(8.0)
    );
    execute_command(
        &mut state,
        SceneCommand::SetClassVariable {
            id: a.clone(),
            name: "speed".into(),
            value: None,
        },
    );
    assert!(classes::class_instance(&state.scene.world(), &a)
        .unwrap()
        .variable_overrides
        .is_empty());
    state.scene.undo();
    assert_eq!(
        classes::class_instance(&state.scene.world(), &a)
            .unwrap()
            .variable_overrides["speed"],
        json!(8.0)
    );
    classes::set_fallback_project_root(None);
}

/// The hierarchy shows a placed class as one class object: its generated
/// children are class-owned; `ClassInstance` is not offered as a component.
#[test]
fn placed_classes_are_class_objects_with_owned_children() {
    let (_project, dir) = project_with_lamp(1.0);
    let mut scene = new_scene();
    let world = &mut scene.world;
    let a = place(world, &dir, 0.0);
    let kids = children(world, &a);
    assert!(classes::is_class_root(world, &a));
    assert!(kids.iter().all(|k| classes::is_generated_child(world, k)));
    assert!(!classes::is_generated_child(world, &a));
    // Slot ids are UUIDs, resolved into placement handles.
    let slot0 = slot(&dir, 0);
    assert!(pulsar_class::is_slot_uuid(&slot0));
    let root = world.entity_for(&a).unwrap();
    let placement = pulsar_class::world::placement(world, root);
    assert_eq!(placement.handle(&slot0).unwrap().entity, root);
}

/// #925: Stop restores the editor world exactly as it was before the first
/// Play, including removing objects scripts spawned during Play
/// (`world::spawn` builds class instances with runtime StableIds) and
/// undoing gameplay edits; a second Play while running keeps the pre-Play
/// snapshot.
#[test]
fn stop_restores_the_pre_play_world_and_removes_runtime_spawns() {
    use crate::level_editor::state::LevelEditorState;
    use engine_backend::scene::{ObjectType, SpawnObject};

    let (_project, dir) = project_with_lamp(1.0);
    let mut state = LevelEditorState::new();
    let a = place(&mut state.scene.world_mut(), &dir, 0.0);
    let describe = |state: &LevelEditorState| {
        let world = state.scene.world();
        let mut objects: Vec<String> = objects::get_all_objects(&world)
            .into_iter()
            .map(|o| format!("{} {:?} {:?} {:?}", o.id, o.parent, o.transform.position, components::get_component_class_names(&world, &o.id)))
            .collect();
        objects.sort();
        (objects, light(&world, &a).intensity.intensity)
    };
    let before = describe(&state);

    state.scene.enter_play_mode();
    {
        let mut world = state.scene.world_mut();
        // Gameplay moves the lamp and dims it.
        objects::set_transform(&mut world, &a, Some([5.0, 0.0, 0.0]), None, None);
        let entity = world.entity_for(&a).unwrap();
        world.get_mut::<LightComponent>(entity).unwrap().intensity.intensity = 0.25;
        // A script spawns a Lamp at runtime, and another under the first.
        let def = classes::registry_for_class_dir(&dir).by_name("Lamp").unwrap().load_definition().unwrap();
        for (id, parent) in [("Lamp_rt1", None), ("Lamp_rt2", Some(entity))] {
            let reserved = world.spawn();
            let spec = SpawnObject {
                stable_id: Some(id.into()),
                name: "Lamp".into(),
                parent,
                transform: Default::default(),
                visibility: Default::default(),
                object_type: ObjectType::Blueprint,
            };
            pulsar_class::world::instantiate_class_into(&mut world, &def, Default::default(), spec, reserved).unwrap();
        }
    }
    // Play again while running (a hot reload): the snapshot stays.
    state.scene.enter_play_mode();
    assert_ne!(describe(&state), before);

    state.scene.exit_play_mode();
    assert_eq!(describe(&state), before, "the editor world is back exactly");
    let world = state.scene.world();
    assert!(world.entity_for("Lamp_rt1").is_none() && world.entity_for("Lamp_rt2").is_none(), "runtime spawns are gone");
    drop(world);
    assert!(!state.scene.has_play_snapshot());
}
