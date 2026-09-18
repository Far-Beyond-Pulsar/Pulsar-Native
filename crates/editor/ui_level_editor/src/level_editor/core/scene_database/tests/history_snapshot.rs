use super::*;

#[cfg(test)]
mod history_snapshot_tests {
    use super::*;

    fn object(name: &str, object_type: ObjectType) -> SceneObjectData {
        SceneObjectData {
            id: String::new(),
            name: name.to_string(),
            object_type,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            scene_path: String::new(),
            props: Default::default(),
            component_instances: None,
        }
    }

    #[test]
    fn capture_and_restore_round_trips_an_empty_scene() {
        let db = SceneDatabase::new();
        let snapshot = db.capture_history_snapshot();
        db.add_folder("Should be undone", None);
        assert_eq!(db.get_all_objects().len(), 1);

        db.restore_history_snapshot(&snapshot).unwrap();

        assert!(db.get_all_objects().is_empty());
    }

    #[test]
    fn restore_brings_back_a_removed_object_with_its_transform() {
        let db = SceneDatabase::new();
        let mut obj = object("Cube", ObjectType::Mesh(MeshType::Cube));
        obj.transform.position = [1.0, 2.0, 3.0];
        let id = db.add_object(obj, None);

        let snapshot = db.capture_history_snapshot();
        db.remove_object(&id);
        assert!(db.get_object(&id).is_none());

        db.restore_history_snapshot(&snapshot).unwrap();

        let restored = db.get_object(&id).expect("object restored");
        assert_eq!(restored.name, "Cube");
        assert_eq!(restored.transform.position, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn restore_brings_back_reflection_components() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Light", ObjectType::Light(LightType::Point)), None);
        db.add_component(
            &id,
            "LightComponent".to_string(),
            serde_json::json!({"intensity": 5.0}),
        );
        assert_eq!(db.get_components(&id).len(), 1);

        let snapshot = db.capture_history_snapshot();
        db.remove_component(&id, 0);
        assert!(db.get_components(&id).is_empty());

        db.restore_history_snapshot(&snapshot).unwrap();

        let components = db.get_components(&id);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].class_name, "LightComponent");
    }

    #[test]
    fn restore_preserves_hierarchy() {
        let db = SceneDatabase::new();
        let parent_id = db.add_object(object("Parent", ObjectType::Empty), None);
        let child_id = db.add_object(object("Child", ObjectType::Empty), Some(parent_id.clone()));

        let snapshot = db.capture_history_snapshot();
        db.clear();
        assert!(db.get_all_objects().is_empty());

        db.restore_history_snapshot(&snapshot).unwrap();

        let child = db.get_object(&child_id).expect("child restored");
        assert_eq!(child.parent.as_deref(), Some(parent_id.as_str()));
    }

    /// `store_revision` must advance across a restore even when the swapped-in
    /// store's raw counter lands on exactly the same value as the swapped-out
    /// one. Both restores below build a 1-object scene, so the fresh store's
    /// raw `render_revision` is identical both times — a naive equality check
    /// misses the second restore entirely, which is how an undo→redo pair
    /// would leave every panel stale.
    #[test]
    fn store_revision_advances_across_restores_with_identical_raw_counters() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("A", ObjectType::Empty), None);

        let snapshot_a = db.capture_history_snapshot();
        db.set_name(&id, "B".to_string());
        let snapshot_b = db.capture_history_snapshot();
        let _ = snapshot_b; // symmetric with the undo/redo flow below

        // Restore A, then B: two wholesale store swaps with equal object
        // counts and therefore equal raw counters after each swap.
        db.restore_history_snapshot(&snapshot_a).unwrap();
        assert_eq!(db.get_object(&id).unwrap().name, "A");
        let rev_after_first_restore = db.store_revision();

        db.restore_history_snapshot(&snapshot_a).unwrap();
        let rev_after_second_restore = db.store_revision();
        assert!(
            rev_after_second_restore > rev_after_first_restore,
            "identical raw counters across a store swap must still fold to an advanced revision"
        );
    }

    #[test]
    fn targeted_reads_match_the_full_object_read() {
        let db = SceneDatabase::new();
        let mut obj = object("Cube", ObjectType::Mesh(MeshType::Cube));
        obj.transform.position = [1.0, 2.0, 3.0];
        obj.transform.rotation = [10.0, 20.0, 30.0];
        obj.transform.scale = [2.0, 2.0, 2.0];
        obj.visible = false;
        obj.locked = true;
        let id = db.add_object(obj, None);

        let full = db.get_object(&id).unwrap();
        let t = db.get_object_transform(&id).unwrap();
        assert_eq!(t.position, full.transform.position);
        assert_eq!(t.rotation, full.transform.rotation);
        assert_eq!(t.scale, full.transform.scale);
        assert_eq!(db.get_object_name(&id).unwrap(), full.name);
        assert_eq!(
            db.get_object_visibility(&id).unwrap(),
            (full.visible, full.locked)
        );
        assert!(db.get_object_transform(&"missing".into()).is_none());
    }
}