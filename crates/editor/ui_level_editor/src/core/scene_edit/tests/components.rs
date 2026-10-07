use engine_backend::scene::{new_scene, ComponentAttachments, ObjectType, SceneWorldExt};
use pulsar_physics::PhysicsComponent;
use serde_json::{json, Value};

use super::super::{components, history, objects, SceneObjectData};

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

/// Whether a metadata record's data holds only record metadata keys.
fn metadata_only(data: &Value) -> bool {
    data.as_object()
        .is_some_and(|map| map.keys().all(|key| key.starts_with("__")))
}

fn physics_json(collision_enabled: bool) -> Value {
    let mut data = serde_json::to_value(PhysicsComponent::default()).unwrap();
    data["general"]["collision_enabled"] = json!(collision_enabled);
    data
}

#[test]
fn light_edits_remain_canonical_through_object_edits_and_history() {
    use helio_component::components::LightComponent;

    let mut scene = new_scene();
    let world = &mut scene.world;
    let id = objects::add_object(world, object("light"), None);
    components::add_component(
        world,
        &id,
        "LightComponent".into(),
        serde_json::to_value(LightComponent::default()).unwrap(),
    );
    let entity = world.entity_for(&id).unwrap();
    let instance = components::instance_at(world, &id, 0).unwrap();
    world
        .get_mut::<LightComponent>(instance)
        .unwrap()
        .intensity
        .intensity = 321.0;
    assert!(world.get::<ComponentAttachments>(entity).is_some());

    let projected = objects::get_object(world, &id).unwrap();
    assert!(objects::update_object(world, projected));
    assert_eq!(
        components::get_components(world, &id)[0].data["intensity"]["intensity"],
        json!(321.0)
    );
    assert!(metadata_only(
        &components::get_components_metadata(world, &id)[0].data
    ));
    let snapshot = history::capture_history_snapshot(world);
    objects::clear(world);
    history::restore_history_snapshot(world, &snapshot).unwrap();
    assert_eq!(
        components::get_components(world, &id)[0].data["intensity"]["intensity"],
        json!(321.0)
    );
}

#[test]
fn duplicate_lights_keep_distinct_values_when_the_live_instance_changes() {
    use helio_component::components::LightComponent;

    let mut scene = new_scene();
    let world = &mut scene.world;
    let id = objects::add_object(world, object("lights"), None);
    for intensity in [10.0, 20.0] {
        let mut light = LightComponent::default();
        light.intensity.intensity = intensity;
        components::add_component(
            world,
            &id,
            "LightComponent".into(),
            serde_json::to_value(light).unwrap(),
        );
    }
    assert_eq!(
        components::get_components(world, &id)[0].data["intensity"]["intensity"],
        json!(10.0)
    );
    assert!(components::set_component_enabled(world, &id, 0, false));
    assert_eq!(
        components::get_components(world, &id)[1].data["intensity"]["intensity"],
        json!(20.0)
    );
    assert!(components::set_component_enabled(world, &id, 0, true));
    components::reorder_component(world, &id, 1, 0);
    assert_eq!(
        components::get_components(world, &id)[0].data["intensity"]["intensity"],
        json!(20.0)
    );
    components::remove_component(world, &id, 0);
    assert_eq!(
        components::get_components(world, &id)[0].data["intensity"]["intensity"],
        json!(10.0)
    );
}

#[test]
fn component_parent_survives_typed_edits_and_history() {
    let mut scene = new_scene();
    let world = &mut scene.world;
    let id = objects::add_object(world, object("nested"), None);
    components::add_component(world, &id, "PhysicsComponent".into(), physics_json(true));
    components::add_component(world, &id, "PhysicsComponent".into(), physics_json(false));
    components::set_component_parent(world, &id, 0, Some(1));
    assert_eq!(
        components::get_components_metadata(world, &id)[0].data["__parent_index"],
        json!(1)
    );
    let projected = objects::get_object(world, &id).unwrap();
    assert!(objects::update_object(world, projected));
    let snapshot = history::capture_history_snapshot(world);
    history::restore_history_snapshot(world, &snapshot).unwrap();
    assert_eq!(
        components::get_components(world, &id)[0].data["__parent_index"],
        json!(1)
    );
    components::reorder_component(world, &id, 1, 0);
    assert_eq!(
        components::get_components(world, &id)[1].data["__parent_index"],
        json!(0)
    );
    components::remove_component(world, &id, 0);
    assert!(components::get_components(world, &id)[0]
        .data
        .get("__parent_index")
        .is_none());
}

#[test]
fn physics_component_data_is_owned_by_the_scene_db_world() {
    let mut scene = new_scene();
    let world = &mut scene.world;
    let id = objects::add_object(world, object("PhysicsBody"), None);
    components::add_component(world, &id, "PhysicsComponent".into(), physics_json(false));

    let metadata = components::get_components_metadata(world, &id);
    assert_eq!(metadata.len(), 1);
    assert!(metadata_only(&metadata[0].data));
    assert_eq!(
        components::get_components(world, &id)[0].data["general"]["collision_enabled"],
        json!(false)
    );
    let entity = components::instance_at(world, &id, 0).unwrap();
    assert!(
        !world
            .get::<PhysicsComponent>(entity)
            .unwrap()
            .general
            .collision_enabled
    );
}

#[test]
fn physics_world_edits_survive_object_updates_and_save_projection() {
    let mut scene = new_scene();
    let world = &mut scene.world;
    let id = objects::add_object(world, object("PhysicsBody"), None);
    components::add_component(world, &id, "PhysicsComponent".into(), physics_json(true));
    let mut physics = PhysicsComponent::default();
    physics.general.collision_enabled = false;
    assert!(components::set_component_value(
        world,
        &id,
        0,
        pulsar_world_registry::InstanceValue::Value(Box::new(physics)),
    ));
    let mut updated = objects::get_object(world, &id).unwrap();
    updated.transform.position = [1.0, 2.0, 3.0];
    assert!(objects::update_object(world, updated));

    assert_eq!(
        components::get_components(world, &id)[0].data["general"]["collision_enabled"],
        json!(false)
    );
    assert!(metadata_only(
        &components::get_components_metadata(world, &id)[0].data
    ));
    let entity = components::instance_at(world, &id, 0).unwrap();
    assert!(
        !world
            .get::<PhysicsComponent>(entity)
            .unwrap()
            .general
            .collision_enabled
    );
}

#[test]
fn disabling_and_reenabling_physics_preserves_the_canonical_value() {
    let mut scene = new_scene();
    let world = &mut scene.world;
    let id = objects::add_object(world, object("PhysicsBody"), None);
    components::add_component(world, &id, "PhysicsComponent".into(), physics_json(false));

    assert!(components::set_component_enabled(world, &id, 0, false));
    let instance = components::instance_at(world, &id, 0).unwrap();
    assert!(
        world.get::<PhysicsComponent>(instance).is_some(),
        "a disabled instance keeps its typed value in place"
    );
    assert!(!engine_backend::scene::attachments::is_enabled(
        world, instance
    ));

    assert!(components::set_component_enabled(world, &id, 0, true));
    assert_eq!(
        components::get_components(world, &id)[0].data["general"]["collision_enabled"],
        json!(false)
    );
    assert!(metadata_only(
        &components::get_components_metadata(world, &id)[0].data
    ));
}

/// Pulsar-Native#1035, Phase 3: an object's `props` are its own render
/// props. Component values are not copied into them, so an object edit
/// written back (`update_object`) and a save carry no stale copies.
#[test]
fn object_props_carry_no_component_copies() {
    use helio_component::components::LightComponent;

    let mut scene = new_scene();
    let world = &mut scene.world;
    let mut data = object("light");
    data.props
        .insert("icon_asset".into(), json!("icons/lamp.png"));
    let id = objects::add_object(world, data, None);
    let mut light = LightComponent::default();
    light.intensity.intensity = 42.0;
    components::add_component_value(world, &id, "LightComponent", Some(Box::new(light)));

    let read = objects::get_object(world, &id).unwrap();
    assert_eq!(
        read.props.len(),
        1,
        "only the object's own prop: {:?}",
        read.props
    );
    assert!(objects::update_object(world, read));
    let entity = world.entity_for(&id).unwrap();
    assert_eq!(
        world
            .get::<engine_backend::scene::RenderProps>(entity)
            .unwrap()
            .props
            .len(),
        1
    );
    assert!(objects::get_all_objects(world)
        .iter()
        .all(|object| !object.props.contains_key("intensity")));
}

/// An unresolved instance's payload is readable (the properties card shows
/// it rather than the class defaults).
#[test]
fn an_unresolved_payload_is_readable() {
    let mut scene = new_scene();
    let world = &mut scene.world;
    let id = objects::add_object(world, object("o"), None);
    let owner = world.entity_for(&id).unwrap();
    pulsar_world_registry::attach_unresolved(
        world,
        owner,
        engine_backend::scene::attachments::NewInstance::new("NotInThisBuild"),
        json!({ "speed": 3 }),
        "not registered".into(),
    )
    .unwrap();
    assert_eq!(
        components::unresolved_payload(world, &id, 0),
        Some(json!({ "speed": 3 }))
    );
    components::add_component_value(world, &id, "LightComponent", None);
    assert_eq!(components::unresolved_payload(world, &id, 1), None, "live");
}
