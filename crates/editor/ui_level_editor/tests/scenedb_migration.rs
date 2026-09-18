use engine_backend::scene::ObjectType;
use pulsar_physics::PhysicsComponent;
use serde_json::json;
use ui_level_editor::{SceneDatabase, SceneObjectData};

fn object(name: &str) -> SceneObjectData {
    SceneObjectData {
        id: String::new(),
        name: name.to_string(),
        object_type: ObjectType::Empty,
        transform: Default::default(),
        visible: true,
        locked: false,
        parent: None,
        children: Vec::new(),
        scene_path: String::new(),
        props: Default::default(),
        component_instances: None,
    }
}

fn physics_json(collision_enabled: bool) -> serde_json::Value {
    let mut data = serde_json::to_value(PhysicsComponent::default()).unwrap();
    data["general"]["collision_enabled"] = json!(collision_enabled);
    data
}

#[test]
fn light_edits_remain_canonical_through_object_edits_and_history() {
    use helio_component::components::LightComponent;
    let db = SceneDatabase::new();
    let id = db.add_object(object("light"), None);
    db.add_component(
        &id,
        "LightComponent".into(),
        serde_json::to_value(LightComponent::default()).unwrap(),
    );
    {
        let shared = db.shared_store();
        let mut store = shared.write();
        let entity = store.entity_for(&id).unwrap();
        store
            .world_mut()
            .get_mut::<LightComponent>(entity)
            .unwrap()
            .intensity
            .intensity = 321.0;
        assert!(store
            .world()
            .get::<engine_backend::scene::ComponentAttachments>(entity)
            .is_some());
    }
    db.update_object(db.get_object(&id).unwrap());
    assert_eq!(
        db.get_components(&id)[0].data["intensity"]["intensity"],
        json!(321.0)
    );
    assert!(db.get_components_metadata(&id)[0].data.is_null());
    let snapshot = db.capture_history_snapshot();
    db.clear();
    db.restore_history_snapshot(&snapshot).unwrap();
    assert_eq!(
        db.get_components(&id)[0].data["intensity"]["intensity"],
        json!(321.0)
    );
}

#[test]
fn duplicate_lights_keep_distinct_values_when_the_live_instance_changes() {
    use helio_component::components::LightComponent;
    let db = SceneDatabase::new();
    let id = db.add_object(object("lights"), None);
    for intensity in [10.0, 20.0] {
        let mut light = LightComponent::default();
        light.intensity.intensity = intensity;
        db.add_component(
            &id,
            "LightComponent".into(),
            serde_json::to_value(light).unwrap(),
        );
    }
    assert_eq!(
        db.get_components(&id)[0].data["intensity"]["intensity"],
        json!(10.0)
    );
    db.set_component_enabled(&id, 0, false);
    assert_eq!(
        db.get_components(&id)[1].data["intensity"]["intensity"],
        json!(20.0)
    );
    db.set_component_enabled(&id, 0, true);
    db.reorder_component(&id, 1, 0);
    assert_eq!(
        db.get_components(&id)[0].data["intensity"]["intensity"],
        json!(20.0)
    );
    db.remove_component(&id, 0);
    assert_eq!(
        db.get_components(&id)[0].data["intensity"]["intensity"],
        json!(10.0)
    );
}

#[test]
fn component_parent_survives_typed_edits_and_history() {
    let db = SceneDatabase::new();
    let id = db.add_object(object("nested"), None);
    db.add_component(&id, "PhysicsComponent".into(), physics_json(true));
    db.add_component(&id, "PhysicsComponent".into(), physics_json(false));
    db.set_component_parent(&id, 0, Some(1));
    assert_eq!(
        db.get_components_metadata(&id)[0].data["__parent_index"],
        json!(1)
    );
    db.update_object(db.get_object(&id).unwrap());
    let snapshot = db.capture_history_snapshot();
    db.restore_history_snapshot(&snapshot).unwrap();
    assert_eq!(db.get_components(&id)[0].data["__parent_index"], json!(1));
    db.reorder_component(&id, 1, 0);
    assert_eq!(db.get_components(&id)[1].data["__parent_index"], json!(0));
    db.remove_component(&id, 0);
    assert!(db.get_components(&id)[0].data.get("__parent_index").is_none());
}

#[test]
fn physics_component_data_is_owned_by_the_scene_db_world() {
    let db = SceneDatabase::new();
    let object_id = db.add_object(object("PhysicsBody"), None);

    db.add_component(
        &object_id,
        "PhysicsComponent".to_string(),
        physics_json(false),
    );

    let metadata = db.get_components_metadata(&object_id);
    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].data, serde_json::Value::Null);

    let components = db.get_components(&object_id);
    assert_eq!(
        components[0].data["general"]["collision_enabled"],
        json!(false)
    );

    let store = db.shared_store();
    let store = store.read();
    let entity = store.entity_for(&object_id).unwrap();
    let physics = store.world().get::<PhysicsComponent>(entity).unwrap();
    assert!(!physics.general.collision_enabled);
}

#[test]
fn physics_world_edits_survive_legacy_object_updates_and_save_projection() {
    let db = SceneDatabase::new();
    let object_id = db.add_object(object("PhysicsBody"), None);
    db.add_component(
        &object_id,
        "PhysicsComponent".to_string(),
        physics_json(true),
    );

    db.update_component(&object_id, 0, physics_json(false));
    let mut updated = db.get_object(&object_id).unwrap();
    updated.transform.position = [1.0, 2.0, 3.0];
    assert!(db.update_object(updated));

    let component = &db.get_components(&object_id)[0];
    assert_eq!(component.data["general"]["collision_enabled"], json!(false));
    assert_eq!(
        db.get_components_metadata(&object_id)[0].data,
        serde_json::Value::Null
    );

    let store = db.shared_store();
    let store = store.read();
    let entity = store.entity_for(&object_id).unwrap();
    let physics = store.world().get::<PhysicsComponent>(entity).unwrap();
    assert!(!physics.general.collision_enabled);
}

#[test]
fn disabling_and_reenabling_physics_preserves_the_canonical_value() {
    let db = SceneDatabase::new();
    let object_id = db.add_object(object("PhysicsBody"), None);
    db.add_component(
        &object_id,
        "PhysicsComponent".to_string(),
        physics_json(false),
    );

    assert!(db.set_component_enabled(&object_id, 0, false));
    {
        let store = db.shared_store();
        let store = store.read();
        let entity = store.entity_for(&object_id).unwrap();
        assert!(store.world().get::<PhysicsComponent>(entity).is_none());
    }

    assert!(db.set_component_enabled(&object_id, 0, true));
    let component = &db.get_components(&object_id)[0];
    assert_eq!(component.data["general"]["collision_enabled"], json!(false));
    assert_eq!(
        db.get_components_metadata(&object_id)[0].data,
        serde_json::Value::Null
    );
}
