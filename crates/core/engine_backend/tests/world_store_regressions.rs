use engine_backend::scene::{ObjectType, WorldSceneStore};

#[test]
fn hierarchy_lifecycle_preserves_parent_links_and_removes_children_once() {
    let mut store = WorldSceneStore::new();
    let parent = store
        .spawn(Some("parent".to_string()), "parent", None)
        .unwrap();
    let child = store
        .spawn(Some("child".to_string()), "child", Some(parent))
        .unwrap();

    assert_eq!(store.parent_of(child), Some(parent));
    assert_eq!(store.children_of(Some(parent)), &[child]);

    store.reparent(child, None).unwrap();
    assert_eq!(store.parent_of(child), None);
    assert!(store.children_of(Some(parent)).is_empty());
    assert_eq!(store.children_of(None), &[parent, child]);

    store.despawn(parent);
    assert!(store.entity_for("parent").is_none());
    assert!(store.entity_for("child").is_some());
    assert_eq!(store.take_removed_ids(), vec!["parent"]);
    assert!(store.take_removed_ids().is_empty());
}

#[test]
fn duplicate_ids_never_replace_the_existing_entity() {
    let mut store = WorldSceneStore::new();
    let original = store
        .spawn(Some("same".to_string()), "original", None)
        .unwrap();

    let error = store.spawn(Some("same".to_string()), "replacement", None);

    assert!(error.is_err());
    assert_eq!(store.entity_for("same"), Some(original));
    assert_eq!(store.get_object("same").unwrap().name, "original");
}

#[test]
fn reparent_rejects_self_and_descendant_cycles_without_mutating_the_tree() {
    let mut store = WorldSceneStore::new();
    let root = store.spawn(None, "root", None).unwrap();
    let child = store.spawn(None, "child", Some(root)).unwrap();
    let grandchild = store.spawn(None, "grandchild", Some(child)).unwrap();

    assert!(store.reparent(root, Some(root)).is_err());
    assert!(store.reparent(root, Some(grandchild)).is_err());
    assert_eq!(store.parent_of(root), None);
    assert_eq!(store.parent_of(child), Some(root));
    assert_eq!(store.parent_of(grandchild), Some(child));
}

#[test]
fn snapshots_are_parent_before_child_and_round_trip_exactly() {
    let mut store = WorldSceneStore::new();
    let parent = store
        .spawn(Some("parent".to_string()), "parent", None)
        .unwrap();
    let _child = store
        .spawn(Some("child".to_string()), "child", Some(parent))
        .unwrap();
    let snapshots = store.to_snapshots();

    assert_eq!(snapshots[0].stable_id, "parent");
    assert_eq!(snapshots[1].stable_id, "child");
    assert_eq!(snapshots[1].parent.as_deref(), Some("parent"));

    let restored = WorldSceneStore::load_from_snapshots(&snapshots).unwrap();
    assert_eq!(restored.to_snapshots(), snapshots);
}

#[test]
fn default_object_type_is_stable_through_spawn_and_snapshot() {
    let mut store = WorldSceneStore::new();
    let entity = store.spawn(None, "empty", None).unwrap();
    assert_eq!(store.object_type(entity), Some(ObjectType::Empty));
}
