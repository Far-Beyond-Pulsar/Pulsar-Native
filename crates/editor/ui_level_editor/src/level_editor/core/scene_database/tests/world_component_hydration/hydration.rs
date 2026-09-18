use super::*;

    #[test]
    fn add_component_hydrates_the_typed_world_value() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);

        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            static_mesh_component_json("meshes/primitives/SM_Cube.fbx"),
        );

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        let hydrated = store.world().get::<StaticMeshComponent>(entity).unwrap();
        assert_eq!(
            hydrated.mesh_asset.as_str(),
            "meshes/primitives/SM_Cube.fbx"
        );
    }

    #[test]
    fn update_component_property_re_hydrates_the_typed_value() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            static_mesh_component_json("meshes/primitives/SM_Cube.fbx"),
        );

        db.update_component_property(
            &id,
            "StaticMeshComponent",
            "mesh_asset",
            serde_json::json!("meshes/primitives/SM_Sphere.fbx"),
        );

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        let hydrated = store.world().get::<StaticMeshComponent>(entity).unwrap();
        assert_eq!(
            hydrated.mesh_asset.as_str(),
            "meshes/primitives/SM_Sphere.fbx"
        );
    }

    #[test]
    fn remove_component_drops_the_typed_world_value() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            static_mesh_component_json("meshes/primitives/SM_Cube.fbx"),
        );
        {
            let store = db.store.read();
            let entity = store.entity_for(&id).unwrap();
            assert!(store.world().get::<StaticMeshComponent>(entity).is_some());
        }

        db.remove_component(&id, 0);

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store.world().get::<StaticMeshComponent>(entity).is_none());
    }

    #[test]
    fn disabling_a_component_drops_the_typed_world_value() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            static_mesh_component_json("meshes/primitives/SM_Cube.fbx"),
        );

        db.set_component_enabled(&id, 0, false);

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store.world().get::<StaticMeshComponent>(entity).is_none());
    }

    #[test]
    fn malformed_component_json_does_not_hydrate_but_does_not_panic() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);

        // `mesh_asset` should be a string; this is a type mismatch, not a
        // missing field, so it should fail hydration cleanly.
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            serde_json::json!({"mesh_asset": 12345}),
        );

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store.world().get::<StaticMeshComponent>(entity).is_none());
    }

    /// Phase B5 (Pulsar-Native#556) spot check: the migration mechanism
    /// itself is already fully proven generically (`pulsar_world_registry`'s
    /// own tests) and end-to-end on `StaticMeshComponent` above -- this
    /// isn't re-proving the mechanism per component (that would just be
    /// duplicating the same five tests seven more times), it's checking for
    /// component-specific surprises. `LightComponent` has many fields with
    /// nested enum sub-props (`IntensityUnits`, `ShadowCacheMode`, ...) --
    /// worth confirming `Default`-derived JSON round-trips through
    /// hydration cleanly, not just a single-field component like
    /// `StaticMeshComponent`.
    #[test]
    fn light_component_hydrates_via_its_default_json() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json =
            serde_json::to_value(helio_component::LightComponent::default()).unwrap();

        db.add_component(&id, "LightComponent".to_string(), default_json);

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store
            .world()
            .get::<helio_component::LightComponent>(entity)
            .is_some());
    }
