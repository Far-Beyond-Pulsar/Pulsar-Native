//! Scene hierarchy and identity regressions, exercised directly on the
//! `pulsar_scenedb::World` through `SceneWorldExt`.

use engine_backend::scene::{ObjectType, SceneWorldExt, SpawnObject};
use pulsar_scenedb::World;

fn spawn(world: &mut World, id: &str, parent: Option<pulsar_scenedb::Entity>) -> pulsar_scenedb::Entity {
    world
        .spawn_object(SpawnObject::new(id).with_id(id).with_parent(parent))
        .unwrap()
}

#[test]
fn hierarchy_lifecycle_preserves_parent_links_and_removes_children_once() {
    let mut world = World::new();
    let parent = spawn(&mut world, "parent", None);
    let child = spawn(&mut world, "child", Some(parent));

    assert_eq!(world.parent_of(child), Some(parent));
    assert_eq!(world.children_of(Some(parent)), vec![child]);

    world.reparent(child, None).unwrap();
    assert_eq!(world.parent_of(child), None);
    assert!(world.children_of(Some(parent)).is_empty());
    assert_eq!(world.children_of(None), vec![parent, child]);

    world.despawn_tree(parent);
    assert!(world.entity_for("parent").is_none());
    assert!(world.entity_for("child").is_some());
}

#[test]
fn duplicate_ids_never_replace_the_existing_entity() {
    let mut world = World::new();
    let original = spawn(&mut world, "same", None);

    let error = world.spawn_object(SpawnObject::new("replacement").with_id("same"));

    assert!(error.is_err());
    assert_eq!(world.entity_for("same"), Some(original));
    assert_eq!(
        world.get::<engine_backend::scene::Name>(original).unwrap().0,
        "same"
    );
}

#[test]
fn reparent_rejects_self_and_descendant_cycles_without_mutating_the_tree() {
    let mut world = World::new();
    let root = spawn(&mut world, "root", None);
    let child = spawn(&mut world, "child", Some(root));
    let grandchild = spawn(&mut world, "grandchild", Some(child));

    assert!(world.reparent(root, Some(root)).is_err());
    assert!(world.reparent(root, Some(grandchild)).is_err());
    assert_eq!(world.parent_of(root), None);
    assert_eq!(world.parent_of(child), Some(root));
    assert_eq!(world.parent_of(grandchild), Some(child));
}

#[test]
fn sibling_order_follows_spawn_order_and_can_be_reordered() {
    let mut world = World::new();
    let a = spawn(&mut world, "a", None);
    let b = spawn(&mut world, "b", None);
    let c = spawn(&mut world, "c", None);
    assert_eq!(world.children_of(None), vec![a, b, c]);

    assert!(world.move_sibling_down(a));
    assert_eq!(world.children_of(None), vec![b, a, c]);
    assert!(world.move_sibling_up(c));
    assert_eq!(world.children_of(None), vec![b, c, a]);
    assert!(!world.move_sibling_up(b), "first sibling cannot move up");
}

#[test]
fn selection_is_a_single_marker_and_dies_with_the_entity() {
    let mut world = World::new();
    let a = spawn(&mut world, "a", None);
    let b = spawn(&mut world, "b", None);

    world.select(Some(a));
    assert_eq!(world.selected_entity(), Some(a));
    world.select(Some(b));
    assert_eq!(world.selected_entity(), Some(b));
    assert_eq!(world.selected_id().as_deref(), Some("b"));

    world.despawn_tree(b);
    assert_eq!(world.selected_entity(), None);
}

#[test]
fn default_object_type_is_empty() {
    let mut world = World::new();
    let entity = world.spawn_object(SpawnObject::new("empty")).unwrap();
    assert_eq!(world.get::<ObjectType>(entity), Some(&ObjectType::Empty));
}

// `telemetry_snapshot_metadata` exists when the SceneDB inspector bridge (a `render`
// dependency) enables SceneDB's `telemetry` feature.
#[cfg(feature = "render")]
#[test]
fn an_object_spawns_in_a_single_archetype() {
    let mut world = World::new();
    spawn(&mut world, "one", None);
    spawn(&mut world, "two", None);
    let populated = world
        .telemetry_snapshot_metadata()
        .archetypes
        .iter()
        .filter(|archetype| archetype.entity_count > 0)
        .count();
    assert_eq!(populated, 1, "bundle spawn must not leave a chain of partial archetypes");
}
