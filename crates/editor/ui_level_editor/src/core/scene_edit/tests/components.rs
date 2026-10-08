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

/// The live typed intensity of the light at `index` (not its save record).
fn light_intensity(world: &pulsar_scenedb::World, id: &str, index: usize) -> f32 {
    let instance = components::instance_at(world, id, index).unwrap();
    world
        .get::<helio_component::components::LightComponent>(instance)
        .unwrap()
        .intensity
        .intensity
}

/// The live typed collision flag of the physics component at `index`.
fn collision_enabled(world: &pulsar_scenedb::World, id: &str, index: usize) -> bool {
    let instance = components::instance_at(world, id, index).unwrap();
    world
        .get::<PhysicsComponent>(instance)
        .unwrap()
        .general
        .collision_enabled
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
    assert_eq!(light_intensity(world, &id, 0), 321.0);
    assert!(metadata_only(
        &components::get_components_metadata(world, &id)[0].data
    ));
    let snapshot = history::capture_history_snapshot(world);
    objects::clear(world);
    history::restore_history_snapshot(world, &snapshot).unwrap();
    assert_eq!(light_intensity(world, &id, 0), 321.0);
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
    assert_eq!(light_intensity(world, &id, 0), 10.0);
    assert!(components::set_component_enabled(world, &id, 0, false));
    assert_eq!(light_intensity(world, &id, 1), 20.0);
    assert!(components::set_component_enabled(world, &id, 0, true));
    components::reorder_component(world, &id, 1, 0);
    assert_eq!(light_intensity(world, &id, 0), 20.0);
    components::remove_component(world, &id, 0);
    assert_eq!(light_intensity(world, &id, 0), 10.0);
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
    assert_eq!(collision_enabled(world, &id, 0), false);
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

    assert_eq!(collision_enabled(world, &id, 0), false);
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
    assert_eq!(collision_enabled(world, &id, 0), false);
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

/// A non-render component's whole editor lifecycle is typed: attached
/// from its default, edited through its reflected (`#[sub_props]`) setter,
/// restored by history in place, and encoded only by the save record.
#[test]
fn rigidbody_add_edit_undo_and_save_are_typed() {
    use pulsar_physics::RigidbodyComponent;

    let mut scene = new_scene();
    let world = &mut scene.world;
    let id = objects::add_object(world, object("Crate"), None);
    components::add_component_value(world, &id, "RigidbodyComponent", None);
    let instance = components::instance_at(world, &id, 0).unwrap();
    let before = history::capture_history_subset(world, &[id.clone()]);

    components::update_live_component_property(
        world,
        &id,
        "RigidbodyComponent",
        0,
        "mass",
        Box::new(12.5f32),
    )
    .unwrap();
    assert_eq!(
        world
            .get::<RigidbodyComponent>(instance)
            .unwrap()
            .general
            .mass,
        12.5
    );
    assert_eq!(
        components::get_components(world, &id)[0].data["general"]["mass"],
        json!(12.5),
        "the save record carries the typed value in the class shape"
    );

    history::restore_history_delta(world, &before, &[id.clone()]).unwrap();
    assert_eq!(
        components::instance_at(world, &id, 0),
        Some(instance),
        "restored in place"
    );
    assert_eq!(
        world
            .get::<RigidbodyComponent>(instance)
            .unwrap()
            .general
            .mass,
        RigidbodyComponent::default().general.mass
    );
}

/// Phase 6 (Pulsar-Native#1035): any number of panels follow the selected
/// object through their own subscriptions. A light edited through the
/// panel's write path and a transform moved elsewhere reach each of them
/// with their new values, and a script's change watch still sees the edit.
#[test]
fn every_panel_feed_receives_edits_with_their_values() {
    use engine_backend::scene::{SceneWorldExt, Transform};
    use helio_component::components::LightComponent;
    use pulsar_world_registry::{ComponentWatch, ObjectFeed, ObjectUpdate};

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
    let panel_a = ObjectFeed::subscribe(world, entity, || {}).unwrap();
    let panel_b = ObjectFeed::subscribe(world, entity, || {}).unwrap();
    let mut script = ComponentWatch::new();
    assert!(script.watch_class(world, "light", instance, "LightComponent"));

    let mut light = LightComponent::default();
    light.intensity.intensity = 42.0;
    assert!(components::set_component_value(
        world,
        &id,
        0,
        pulsar_world_registry::InstanceValue::Value(Box::new(light)),
    ));
    world.get_mut::<Transform>(entity).unwrap().position = [1.0, 2.0, 3.0];

    for feed in [&panel_a, &panel_b] {
        let updates = feed.take();
        let intensity = updates.iter().find_map(|u| match u {
            ObjectUpdate::Changed(d) if d.entity == instance => d
                .value
                .as_deref()?
                .downcast_ref::<LightComponent>()
                .map(|l| l.intensity.intensity),
            _ => None,
        });
        assert_eq!(intensity, Some(42.0));
        let position = updates.iter().find_map(|u| match u {
            ObjectUpdate::Changed(d) if d.entity == entity => d
                .value
                .as_deref()?
                .downcast_ref::<Transform>()
                .map(|t| t.position),
            _ => None,
        });
        assert_eq!(position, Some([1.0, 2.0, 3.0]));
    }
    assert_eq!(script.poll(world), vec!["light"]);
}
