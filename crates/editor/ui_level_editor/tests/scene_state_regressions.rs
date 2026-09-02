use std::collections::HashSet;

use engine_backend::scene::{ObjectType, WorldSceneStore};
use ui_level_editor::{SceneDatabase, SceneObjectData};

fn object_with_id(id: &str) -> SceneObjectData {
    let db = SceneDatabase::new();
    let created = db.add_folder("fixture", None);
    let mut object = db.get_object(&created).expect("fixture object");
    object.id = id.to_string();
    object.name = "requested-id".to_string();
    object
}

#[test]
fn duplicate_requested_id_is_rejected_without_creating_a_second_object() {
    let db = SceneDatabase::new();
    let first = db.add_folder("original", None);
    let mut duplicate = db.get_object(&first).expect("original object");
    duplicate.name = "must-not-be-added".to_string();

    let returned = db.add_object(duplicate, None);

    assert!(returned.is_empty());
    assert_eq!(db.get_all_objects().len(), 1);
    assert_eq!(db.get_object(&first).unwrap().name, "original");
}

#[test]
fn missing_parent_is_rejected_without_orphaning_metadata_or_scene_state() {
    let db = SceneDatabase::new();
    let object = object_with_id("child");

    let returned = db.add_object(object, Some("does-not-exist".to_string()));

    assert!(returned.is_empty());
    assert!(db.get_all_objects().is_empty());
    assert_eq!(db.component_count(&"child".to_string()), 0);
}

#[test]
fn hierarchy_snapshot_has_one_consistent_object_and_root_projection() {
    let db = SceneDatabase::new();
    let parent = db.add_folder("parent", None);
    let child = db.add_folder("child", Some(parent.clone()));

    let (objects, roots) = db.get_hierarchy_snapshot();
    let ids: HashSet<_> = objects.iter().map(|object| object.id.as_str()).collect();

    assert_eq!(objects.len(), 2);
    assert_eq!(ids.len(), 2);
    assert_eq!(roots, vec![parent.clone()]);
    assert_eq!(
        objects
            .iter()
            .find(|object| object.id == child)
            .unwrap()
            .parent,
        Some(parent)
    );
}

#[test]
fn clear_removes_the_entire_scene_and_allows_clean_reuse_of_the_store() {
    let db = SceneDatabase::new();
    let parent = db.add_folder("parent", None);
    let _child = db.add_folder("child", Some(parent));

    db.clear();

    assert!(db.get_all_objects().is_empty());
    assert_eq!(db.root_count(), 0);
    let replacement = db.add_folder("replacement", None);
    assert!(!replacement.is_empty());
    assert_eq!(db.get_all_objects().len(), 1);
}

#[test]
fn world_store_removal_queue_does_not_hide_a_reused_identity() {
    let mut store = WorldSceneStore::new();
    let old = store
        .spawn(Some("stable".to_string()), "old", None)
        .unwrap();
    store.despawn(old);

    let removed = store.take_removed_ids();
    assert_eq!(removed, vec!["stable"]);

    let replacement = store
        .spawn(Some("stable".to_string()), "replacement", None)
        .unwrap();
    assert_eq!(store.stable_id_of(replacement), Some("stable"));
    assert_eq!(store.get_object("stable").unwrap().name, "replacement");
}

#[test]
fn scene_object_properties_round_trip_through_world_components() {
    let db = SceneDatabase::new();
    let id = db.add_folder("original", None);

    assert!(db.set_name(&id, "renamed".to_string()));
    assert!(db.set_visible(&id, false));
    assert!(db.set_locked(&id, true));

    let object = db.get_object(&id).expect("object remains in the World");
    assert_eq!(object.name, "renamed");
    assert_eq!(object.object_type, ObjectType::Folder);
    assert!(!object.visible);
    assert!(object.locked);
}
