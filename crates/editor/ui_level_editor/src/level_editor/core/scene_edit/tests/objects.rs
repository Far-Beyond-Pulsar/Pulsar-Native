use std::collections::HashSet;

use engine_backend::scene::{new_scene, ObjectType, SceneWorldExt, SpawnObject};

use super::super::{components, objects};

#[test]
fn duplicate_requested_id_is_rejected_without_creating_a_second_object() {
    let mut scene = new_scene();
    let world = &mut scene.world;
    let first = objects::add_folder(world, "original", None);
    let mut duplicate = objects::get_object(world, &first).expect("original object");
    duplicate.name = "must-not-be-added".to_string();

    let returned = objects::add_object(world, duplicate, None);

    assert!(returned.is_empty());
    assert_eq!(objects::get_all_objects(world).len(), 1);
    assert_eq!(objects::get_object(world, &first).unwrap().name, "original");
}

#[test]
fn missing_parent_is_rejected_without_orphaning_metadata_or_scene_state() {
    let mut scene = new_scene();
    let world = &mut scene.world;
    let fixture = objects::add_folder(world, "fixture", None);
    let mut object = objects::get_object(world, &fixture).expect("fixture object");
    objects::clear(world);
    object.id = "child".to_string();

    let returned = objects::add_object(world, object, Some("does-not-exist".to_string()));

    assert!(returned.is_empty());
    assert!(objects::get_all_objects(world).is_empty());
    assert!(world.entity_for("child").is_none());
    assert!(components::get_components(world, "child").is_empty());
}

#[test]
fn hierarchy_snapshot_has_one_consistent_object_and_root_projection() {
    let mut scene = new_scene();
    let world = &mut scene.world;
    let parent = objects::add_folder(world, "parent", None);
    let child = objects::add_folder(world, "child", Some(parent.clone()));

    let (all, roots) = objects::get_hierarchy_snapshot(world);
    let ids: HashSet<_> = all.iter().map(|object| object.id.as_str()).collect();

    assert_eq!(all.len(), 2);
    assert_eq!(ids.len(), 2);
    assert_eq!(roots, vec![parent.clone()]);
    assert_eq!(
        all.iter().find(|object| object.id == child).unwrap().parent,
        Some(parent)
    );
}

#[test]
fn clear_removes_the_entire_scene_and_allows_clean_reuse() {
    let mut scene = new_scene();
    let world = &mut scene.world;
    let parent = objects::add_folder(world, "parent", None);
    let _child = objects::add_folder(world, "child", Some(parent));

    objects::clear(world);

    assert!(objects::get_all_objects(world).is_empty());
    assert_eq!(objects::root_count(world), 0);
    let replacement = objects::add_folder(world, "replacement", None);
    assert!(!replacement.is_empty());
    assert_eq!(objects::get_all_objects(world).len(), 1);
}

#[test]
fn removing_and_reusing_an_identity_resolves_to_the_new_entity() {
    let mut scene = new_scene();
    let world = &mut scene.world;
    let old = world
        .spawn_object(SpawnObject::new("old").with_id("stable"))
        .unwrap();
    world.despawn_tree(old);

    assert!(world.entity_for("stable").is_none());
    let replacement = world
        .spawn_object(SpawnObject::new("replacement").with_id("stable"))
        .unwrap();
    assert_ne!(replacement, old);
    assert_eq!(world.stable_id_of(replacement), Some("stable"));
    assert_eq!(
        objects::get_object(world, "stable").unwrap().name,
        "replacement"
    );
}

#[test]
fn scene_object_properties_round_trip_through_world_components() {
    let mut scene = new_scene();
    let world = &mut scene.world;
    let id = objects::add_folder(world, "original", None);

    assert!(objects::set_name(world, &id, "renamed".to_string()));
    assert!(objects::set_visible(world, &id, false));
    assert!(objects::set_locked(world, &id, true));

    let object = objects::get_object(world, &id).expect("object remains in the World");
    assert_eq!(object.name, "renamed");
    assert_eq!(object.object_type, ObjectType::Folder);
    assert!(!object.visible);
    assert!(object.locked);
}
