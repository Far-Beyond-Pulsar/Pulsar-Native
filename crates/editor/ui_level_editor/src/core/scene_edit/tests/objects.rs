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

    let (all, roots) = objects::get_hierarchy_projection(world);
    let ids: HashSet<_> = all.iter().map(|object| object.id.as_str()).collect();

    assert_eq!(all.len(), 2);
    assert_eq!(ids.len(), 2);
    assert_eq!(roots, vec![parent.clone()]);
    assert_eq!(
        all.iter()
            .find(|object| object.id == parent)
            .unwrap()
            .children,
        vec![child.clone()]
    );
    assert!(all
        .iter()
        .find(|object| object.id == child)
        .unwrap()
        .children
        .is_empty());
    assert_eq!(
        objects::get_object(world, &child).unwrap().parent,
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

#[test]
fn a_folders_contents_follow_it_without_being_rewritten() {
    use engine_backend::scene::hierarchy_transform::world_transform_of;
    let mut scene = new_scene();
    let world = &mut scene.world;
    let folder = objects::add_folder(world, "folder", None);
    let item = objects::add_folder(world, "item", None);
    objects::set_transform(world, &item, Some([1.0, 2.0, 3.0]), None, None);

    // Put in the folder (which is at the origin), the object stays where it is.
    assert!(objects::reparent_object(world, &item, Some(folder.clone())));
    let entity = world.entity_for(&item).unwrap();
    assert_eq!(
        world_transform_of(world, entity).unwrap().position,
        [1.0, 2.0, 3.0]
    );

    // Moving the folder moves the object in the world. Its own stored,
    // parent-relative transform is not touched: nothing is rewritten per object.
    assert!(objects::set_transform(
        world,
        &folder,
        Some([10.0, 0.0, 0.0]),
        None,
        None
    ));
    assert_eq!(
        objects::get_object(world, &item).unwrap().transform.position,
        [1.0, 2.0, 3.0]
    );
    let moved = world_transform_of(world, entity).unwrap().position;
    assert!(
        moved
            .iter()
            .zip([11.0, 2.0, 3.0])
            .all(|(a, b)| (a - b).abs() < 1e-4),
        "{moved:?}"
    );
}

#[test]
fn reparenting_to_the_root_keeps_the_world_pose() {
    use engine_backend::scene::hierarchy_transform::world_transform_of;
    let mut scene = new_scene();
    let world = &mut scene.world;
    let folder = objects::add_folder(world, "folder", None);
    objects::set_transform(world, &folder, Some([10.0, 0.0, 0.0]), None, None);
    let item = objects::add_folder(world, "item", Some(folder.clone()));
    objects::set_transform(world, &item, Some([1.0, 0.0, 0.0]), None, None);
    let entity = world.entity_for(&item).unwrap();
    assert_eq!(world_transform_of(world, entity).unwrap().position[0], 11.0);

    assert!(objects::reparent_object(world, &item, None));

    assert_eq!(
        objects::get_object(world, &item).unwrap().transform.position[0],
        11.0
    );
}

#[test]
fn duplicating_a_folder_duplicates_its_contents_into_the_copy() {
    let mut scene = new_scene();
    let world = &mut scene.world;
    let folder = objects::add_folder(world, "folder", None);
    let item = objects::add_folder(world, "item", Some(folder.clone()));
    objects::set_transform(world, &item, Some([1.0, 2.0, 3.0]), None, None);
    let inner = objects::add_folder(world, "inner", Some(folder.clone()));
    let deep = objects::add_folder(world, "deep", Some(inner.clone()));
    let before = objects::get_all_objects(world).len();

    let copy = objects::duplicate_object(world, &folder).expect("duplicated");

    assert_eq!(objects::get_all_objects(world).len(), before * 2, "every object was copied");
    let copy_data = objects::get_object(world, &copy).unwrap();
    assert_eq!(copy_data.name, "folder (Copy)");
    assert_eq!(copy_data.children.len(), 2, "the copy holds copies of both contents");
    // The originals still hold theirs, untouched.
    assert_eq!(objects::get_object(world, &folder).unwrap().children, [item.clone(), inner.clone()]);
    assert_eq!(objects::get_object(world, &inner).unwrap().children, [deep]);

    let mut names = Vec::new();
    for id in &copy_data.children {
        let child = objects::get_object(world, id).unwrap();
        assert_ne!(*id, item);
        assert_ne!(*id, inner);
        if child.name == "item" {
            assert_eq!(child.transform.position, [1.0, 2.0, 3.0], "same place in the new folder");
        }
        if child.name == "inner" {
            assert_eq!(child.children.len(), 1, "nested contents are copied too");
            assert_eq!(objects::get_object(world, &child.children[0]).unwrap().name, "deep");
        }
        names.push(child.name);
    }
    names.sort();
    assert_eq!(names, ["inner", "item"], "contents keep their names");
}

#[test]
fn the_rigid_frame_prop_makes_a_frame_and_copies_keep_it() {
    use engine_backend::scene::frames::{Frame, FRAME_PROP};
    let mut scene = new_scene();
    let world = &mut scene.world;
    let ship = objects::add_folder(world, "ship", None);
    let entity = world.entity_for(&ship).unwrap();
    assert!(world.get::<Frame>(entity).is_none());

    // The prop is the authored truth; updating the object applies it.
    let mut data = objects::get_object(world, &ship).unwrap();
    data.props
        .insert(FRAME_PROP.to_string(), serde_json::Value::Bool(true));
    assert!(objects::update_object(world, data));
    let slot = world.get::<Frame>(entity).expect("now a frame").slot;

    // A copy is a frame of its own (its own slot), contents included.
    let inside = objects::add_folder(world, "inside", Some(ship.clone()));
    let copy = objects::duplicate_object(world, &ship).expect("copy");
    let copy_entity = world.entity_for(&copy).unwrap();
    let copy_slot = world.get::<Frame>(copy_entity).expect("the copy is a frame").slot;
    assert_ne!(slot, copy_slot);
    let _ = inside;

    // Clearing the prop makes it an ordinary group again.
    let mut data = objects::get_object(world, &ship).unwrap();
    data.props.remove(FRAME_PROP);
    assert!(objects::update_object(world, data));
    assert!(world.get::<Frame>(entity).is_none());
    assert!(world.get::<Frame>(copy_entity).is_some(), "the copy is untouched");
}
