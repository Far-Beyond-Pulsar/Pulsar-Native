use super::*;

    /// `PortalComponent` is the trickiest of B5's list -- its
    /// `sync_component` pairs two components sharing a `portal_id` via
    /// `PortalLinkCache`, tracked independently of storage. Worth confirming
    /// two portal-typed objects both hydrate correctly (the pairing logic
    /// itself is unaffected by storage, but this proves *that* claim rather
    /// than just asserting it).
    #[test]
    fn portal_component_hydrates_on_both_sides_of_a_pair() {
        let db = SceneDatabase::new();
        let a = db.add_object(object("PortalA"), None);
        let b = db.add_object(object("PortalB"), None);
        let default_json =
            serde_json::to_value(helio_component::PortalComponent::default()).unwrap();

        db.add_component(&a, "PortalComponent".to_string(), default_json.clone());
        db.add_component(&b, "PortalComponent".to_string(), default_json);

        let store = db.store.read();
        let entity_a = store.entity_for(&a).unwrap();
        let entity_b = store.entity_for(&b).unwrap();
        assert!(store
            .world()
            .get::<helio_component::PortalComponent>(entity_a)
            .is_some());
        assert!(store
            .world()
            .get::<helio_component::PortalComponent>(entity_b)
            .is_some());
    }

    /// Phase D (Pulsar-Native#558): `ReflectionCaptureComponent` is the
    /// first newly-authored (not migrated) component to go through this
    /// mechanism -- same spot-check shape as B5's, confirming the
    /// already-proven mechanism holds for brand-new components too, not
    /// just migrated ones.
    #[test]
    fn reflection_capture_component_hydrates_via_its_default_json() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Probe"), None);
        let default_json =
            serde_json::to_value(helio_component::ReflectionCaptureComponent::default()).unwrap();

        db.add_component(&id, "ReflectionCaptureComponent".to_string(), default_json);

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store
            .world()
            .get::<helio_component::ReflectionCaptureComponent>(entity)
            .is_some());
    }

    #[test]
    fn water_volume_component_hydrates_via_its_default_json() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Lake"), None);
        let default_json =
            serde_json::to_value(helio_component::WaterVolumeComponent::default()).unwrap();

        db.add_component(&id, "WaterVolumeComponent".to_string(), default_json);

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store
            .world()
            .get::<helio_component::WaterVolumeComponent>(entity)
            .is_some());
    }

    #[test]
    fn post_process_volume_component_hydrates_via_its_default_json() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("GlobalPostFx"), None);
        let default_json =
            serde_json::to_value(helio_component::PostProcessVolumeComponent::default()).unwrap();

        db.add_component(&id, "PostProcessVolumeComponent".to_string(), default_json);

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store
            .world()
            .get::<helio_component::PostProcessVolumeComponent>(entity)
            .is_some());
    }

    // ── World subscriptions (Pulsar-Native#575, SceneDB#47) ────────────────

    /// Pulsar-Native#519: two instances of the SAME class on one object are
    /// two independent value stores. `World` can hold only the first
    /// enabled instance typed; the duplicate keeps its OWN metadata JSON,
    /// and edits route by index -- editing instance 1 must never touch
    /// instance 0 (or the reverse), on either the read or write side.
    #[test]
    fn duplicate_class_instances_hold_independent_field_values() {
        use helio_component::LightComponent;
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json = serde_json::to_value(LightComponent::default()).unwrap();
        db.add_component(&id, "LightComponent".to_string(), default_json.clone());
        db.add_component(&id, "LightComponent".to_string(), default_json);

        // Exactly one live-typed representative, and it's instance 0.
        assert_eq!(
            db.live_typed_component_index(&id, "LightComponent"),
            Some(0),
            "of N duplicates, only the first enabled one is World-typed"
        );

        // Distinct edits to each instance, by index.
        db.update_live_component_property(
            &id,
            "LightComponent",
            1,
            "intensity",
            Box::new(111.0_f32) as Box<dyn Any + Send>,
        )
        .expect("duplicate-instance edit routes by index");
        db.update_live_component_property(
            &id,
            "LightComponent",
            0,
            "intensity",
            Box::new(222.0_f32) as Box<dyn Any + Send>,
        )
        .expect("live-typed edit applies");

        // Write side stayed per-instance: the World-typed value (and its
        // metadata mirror at index 0) carries ONLY instance 0's edit;
        // instance 1's blob carries only its own.
        let components = db.get_components(&id);
        assert_eq!(
            components[0].data.pointer("/intensity/intensity"),
            Some(&serde_json::json!(222.0)),
        );
        assert_eq!(
            components[1].data.pointer("/intensity/intensity"),
            Some(&serde_json::json!(111.0)),
            "instance 1's stored value must be its own -- neither the World \
             overlay nor instance 0's edit may clobber it"
        );
        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        let hydrated = store.world().get::<LightComponent>(entity).unwrap();
        assert_eq!(hydrated.intensity.intensity, 222.0);
    }

    /// Pulsar-Native#519 follow-through: when the current live-typed
    /// instance goes away (removed), the NEXT duplicate becomes the
    /// representative and is re-hydrated from ITS OWN edited JSON -- not
    /// from anything instance 0 left behind.
    #[test]
    fn removing_the_live_instance_promotes_the_duplicate_from_its_own_json() {
        use helio_component::LightComponent;
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json = serde_json::to_value(LightComponent::default()).unwrap();
        db.add_component(&id, "LightComponent".to_string(), default_json.clone());
        db.add_component(&id, "LightComponent".to_string(), default_json);

        // Give each instance a distinct intensity BEFORE any removal.
        db.update_live_component_property(
            &id,
            "LightComponent",
            1,
            "intensity",
            Box::new(111.0_f32) as Box<dyn Any + Send>,
        )
        .unwrap();

        // Remove instance 0; instance 1 (intensity 111.0) is now first.
        db.remove_component(&id, 0);

        assert_eq!(
            db.live_typed_component_index(&id, "LightComponent"),
            Some(0),
            "the surviving duplicate is now the class's live-typed instance"
        );
        let components = db.get_components(&id);
        assert_eq!(
            components[0].data.pointer("/intensity/intensity"),
            Some(&serde_json::json!(111.0)),
            "promotion must adopt the duplicate's OWN field values"
        );
    }

    /// The index is a hard identity check, not advisory: a stale index
    /// pointing at a different class must refuse the edit rather than land
    /// it in some other instance.
    #[test]
    fn update_live_component_property_refuses_a_mismatched_component_index() {
        use helio_component::{LightComponent, StaticMeshComponent};
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Thing"), None);
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            static_mesh_component_json("meshes/primitives/SM_Cube.fbx"),
        );
        db.add_component(
            &id,
            "LightComponent".to_string(),
            serde_json::to_value(LightComponent::default()).unwrap(),
        );

        // Index 0 is the StaticMeshComponent; claiming it for a Light edit
        // must bounce the value straight back, unmodified.
        let value = Box::new(500.0_f32) as Box<dyn Any + Send>;
        let result =
            db.update_live_component_property(&id, "LightComponent", 0, "intensity", value);
        assert!(result.is_err(), "class/index mismatch must refuse");
    }

    /// The properties panel's core contract: arm once per card, edit the
    /// live value through the real write path, and the subscription delivers
    /// exactly one event tagged with that card's id. A drain empties; an
    /// unsubscribed card hears nothing further.
    #[test]
    fn subscribe_component_delivers_events_for_live_edits_to_that_card_only() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            static_mesh_component_json("meshes/primitives/SM_Cube.fbx"),
        );

        let sub = db
            .subscribe_component(&id, "StaticMeshComponent")
            .expect("registered class with a live entity subscribes");
        assert!(
            db.take_world_component_events().is_empty(),
            "arming alone must deliver nothing"
        );
        // Drain is emptying, not peeking: a second drain is empty too.
        assert!(db.take_world_component_events().is_empty());

        // Real mutation through the same typed path the panel's setter
        // closures ride (World::get_mut -> Mut guard -> into_inner).
        {
            let mut store = db.store.write();
            let entity = store.entity_for(&id).unwrap();
            let instance = pulsar_world_registry::get_world_component_as_engine_class_mut(
                "StaticMeshComponent",
                store.world_mut(),
                entity,
            )
            .unwrap();
            let concrete = instance
                .as_any_mut()
                .downcast_mut::<StaticMeshComponent>()
                .unwrap();
            concrete.mesh_asset = "meshes/primitives/SM_Sphere.fbx".into();
        }

        let events: Vec<_> = db
            .take_world_component_events()
            .into_iter()
            .filter(|e| e.subscription == sub)
            .collect();
        assert_eq!(events.len(), 1, "one real mutation = exactly one event");
        assert_eq!(events[0].kind, pulsar_scenedb::ComponentChangeKind::Mutated);
        assert!(db.take_world_component_events().is_empty());

        // After unsubscribe, the same kind of write stays silent.
        db.unsubscribe_component(sub);
        {
            let mut store = db.store.write();
            let entity = store.entity_for(&id).unwrap();
            let instance = pulsar_world_registry::get_world_component_as_engine_class_mut(
                "StaticMeshComponent",
                store.world_mut(),
                entity,
            )
            .unwrap();
            let concrete = instance
                .as_any_mut()
                .downcast_mut::<StaticMeshComponent>()
                .unwrap();
            concrete.mesh_asset = "meshes/primitives/SM_Cone.fbx".into();
        }
        assert!(db.take_world_component_events().is_empty());
    }

    /// Unregistered classes have no live `World` representation -- there is
    /// nothing to subscribe to, and `None` (not an error) is the answer.
    #[test]
    fn subscribing_an_unregistered_class_is_none() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        assert!(db
            .subscribe_component(&id, "NoSuchComponentClass")
            .is_none());
    }

    /// Undo/redo swaps the whole `World` out from under any outstanding
    /// subscriptions (they live inside it) without firing events. The epoch
    /// is the only signal that this happened, so it MUST advance across a
    /// restore even when the snapshot content is identical.
    #[test]
    fn restore_history_snapshot_bumps_the_subscriptions_epoch() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);

        let before = db.subscriptions_epoch();
        let snapshot = db.capture_history_snapshot();

        db.restore_history_snapshot(&snapshot).unwrap();
        assert_ne!(
            db.subscriptions_epoch(),
            before,
            "a store swap must invalidate every outstanding subscription"
        );
    }

    /// The lifecycle owner creates the SceneDB-backed store once; consumers
    /// such as the renderer receive only a shared handle to that same store.
    #[test]
    fn shared_store_is_the_database_scene() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        let store = db.shared_store();

        let store = store.read();
        let entity = store.entity_for(&id).expect("object lives in shared store");
        assert_eq!(store.name(entity), Some("Cube"));
    }