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
