use super::*;

    /// Pulsar-Native#561 regression test: editing a `#[sub_props]`-nested
    /// leaf field (e.g. `LightComponent.intensity.intensity`) through
    /// `update_live_component_property` must land in the correct nested
    /// location and must not disturb any sibling field or sub-group -- the
    /// exact failure mode of the bug this fixes was a flat top-level JSON
    /// write either landing on a JSON key the struct doesn't have (silently
    /// dropped) or, worse, overwriting a whole nested sub-struct with a bare
    /// scalar when the leaf name happened to collide with its parent
    /// sub-props field's own name (`color`/`color`).
    #[test]
    fn update_live_component_property_edits_only_the_targeted_nested_leaf() {
        use helio_component::LightComponent;
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json = serde_json::to_value(LightComponent::default()).unwrap();
        db.add_component(&id, "LightComponent".to_string(), default_json);

        // `intensity` is a leaf field inside `IntensityLightProps`, itself
        // reached through `LightComponent.intensity: IntensityLightProps` --
        // a flat top-level JSON write would land on a key `LightComponent`
        // doesn't have at all (silently ignored by serde on next load), not
        // `data.intensity.intensity`.
        let applied = db.update_live_component_property(
            &id,
            "LightComponent",
            0,
            "intensity",
            Box::new(500.0_f32) as Box<dyn Any + Send>,
        );
        assert!(
            applied.is_ok(),
            "live edit should apply directly, no JSON fallback needed"
        );

        // `color` is a leaf field inside `ColorLightProps`, whose *parent*
        // sub-props field on `LightComponent` is also named `color` -- the
        // exact name collision that made the old flat write corrupt the
        // whole nested object instead of just failing quietly.
        let applied = db.update_live_component_property(
            &id,
            "LightComponent",
            0,
            "color",
            Box::new([0.25_f32, 0.5, 0.75, 1.0]) as Box<dyn Any + Send>,
        );
        assert!(applied.is_ok());

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        let hydrated = store.world().get::<LightComponent>(entity).unwrap();

        assert_eq!(hydrated.intensity.intensity, 500.0);
        assert_eq!(hydrated.color.color, [0.25, 0.5, 0.75, 1.0]);

        // Every sibling field on both touched sub-groups, and every
        // untouched sub-group, must still match `Default` exactly -- proving
        // the edit was scoped to just the one targeted leaf, not a
        // sub-struct-clobbering overwrite.
        let expected = LightComponent::default();
        assert_eq!(
            hydrated.intensity.intensity_units,
            expected.intensity.intensity_units
        );
        assert_eq!(
            hydrated.intensity.exposure_compensation,
            expected.intensity.exposure_compensation
        );
        assert_eq!(
            hydrated.color.use_temperature,
            expected.color.use_temperature
        );
        assert_eq!(
            hydrated.color.temperature_kelvin,
            expected.color.temperature_kelvin
        );
        // `GeneralLightProps`/`AttenuationLightProps`/`ShadowLightProps`
        // don't derive `PartialEq` -- compare via their own `Serialize`
        // impl instead (both already derive it for the JSON boundary).
        assert_eq!(
            serde_json::to_value(&hydrated.general).unwrap(),
            serde_json::to_value(&expected.general).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&hydrated.attenuation).unwrap(),
            serde_json::to_value(&expected.attenuation).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&hydrated.shadows).unwrap(),
            serde_json::to_value(&expected.shadows).unwrap()
        );
    }

    /// Regression test for the actual bug behind "the properties panel
    /// shows the right value, but the light in the scene never changes":
    /// `update_live_component_property` mutated the live `World` component
    /// correctly, but never touched `WorldSceneStore`'s own dirty-tracking
    /// (`dirty`/`dirty_gen`/`render_revision`), which is the *only* thing
    /// `HelioRenderer::render_frame` checks to decide whether a sync pass
    /// (`sync_scene`/`sync_scene_delta` -- the thing that actually pushes a
    /// component's current value into Helio's scene) should run at all.
    /// A `World`-correct edit that never bumps `render_revision` is
    /// invisible to the renderer, indefinitely, even though every direct
    /// `World` read (this method's own `read_live_component_property`
    /// counterpart, and the properties panel that calls it) sees it fine.
    #[test]
    fn update_live_component_property_marks_the_object_dirty_for_the_renderer() {
        use helio_component::LightComponent;
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json = serde_json::to_value(LightComponent::default()).unwrap();
        db.add_component(&id, "LightComponent".to_string(), default_json);

        let revision_before = db.store.read().render_revision();

        db.update_live_component_property(
            &id,
            "LightComponent",
            0,
            "intensity",
            Box::new(1000.0_f32) as Box<dyn Any + Send>,
        )
        .expect("live edit should apply");

        assert!(
            db.store.read().render_revision() > revision_before,
            "a live component edit must bump render_revision, or HelioRenderer's \
             render_frame never even attempts a sync pass for it -- the World value \
             would be correct (readable directly) but never reach the actual scene"
        );

        let flags = db.store.write().take_dirty_flags(&id);
        assert!(
            flags.contains(engine_backend::scene::ObjectDirtyFlags::COMPONENTS),
            "dirty flags must include COMPONENTS so sync picks the object's \
             components back up, not just its transform"
        );
    }

    /// Pulsar-Native#561 regression test: `update_live_component_property`
    /// writes straight to `World` and nowhere else -- `get_components`
    /// (what both the properties panel's card list and
    /// `save_to_file_with_editor_camera` read) must still see the edit, by
    /// resolving `data` fresh off the live `World` value rather than
    /// trusting `component_store`'s now-stale stored copy. Without this, a live
    /// edit would render correctly in the properties panel (which reads
    /// each field individually via `read_live_component_property`) but be
    /// silently lost on save -- exactly the kind of two-competing-copies
    /// bug this whole fix exists to eliminate.
    #[test]
    fn live_edit_is_visible_through_get_components_not_just_the_live_read_path() {
        use helio_component::LightComponent;
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json = serde_json::to_value(LightComponent::default()).unwrap();
        db.add_component(&id, "LightComponent".to_string(), default_json);

        db.update_live_component_property(
            &id,
            "LightComponent",
            0,
            "intensity",
            Box::new(750.0_f32) as Box<dyn Any + Send>,
        )
        .expect("LightComponent is World-registered, edit should apply live");

        let components = db.get_components(&id);
        let light = components
            .iter()
            .find(|c| c.class_name == "LightComponent")
            .expect("LightComponent should still be attached");
        assert_eq!(
            light.data.get("intensity").and_then(|v| v.get("intensity")),
            Some(&serde_json::json!(750.0)),
            "get_components (and therefore save-to-disk) must reflect the live edit, \
             not component_store's stale stored JSON"
        );
    }

    /// Pulsar-Native#561 regression test for Bug B (the light-color crash's
    /// second, independent cause): `update_live_component_property` writes
    /// straight to `World`, but before this fix never persisted back into
    /// `component_store`. `sync_registered_component_props_to_scene_db` -- which
    /// runs on *every* transform/name/visibility/legacy-component edit, not
    /// just component-property edits -- re-hydrates every `World`-registered
    /// component from `component_store`'s (stale, pre-edit) JSON. Net effect
    /// before the fix: a live-edited property was visible immediately, then
    /// silently reverted the moment the user made *any other* edit to the
    /// same object. This test edits a component property live, then performs
    /// a wholly unrelated `update_object` (a transform move) on the SAME
    /// object, and asserts the property edit survived -- the exact sequence
    /// that used to clobber it.
    #[test]
    fn update_live_component_property_survives_an_unrelated_update_object_call() {
        use helio_component::LightComponent;
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json = serde_json::to_value(LightComponent::default()).unwrap();
        db.add_component(&id, "LightComponent".to_string(), default_json);

        db.update_live_component_property(
            &id,
            "LightComponent",
            0,
            "intensity",
            Box::new(750.0_f32) as Box<dyn Any + Send>,
        )
        .expect("LightComponent is World-registered, edit should apply live");

        // An edit to something else entirely on the same object -- this used
        // to be exactly what triggered the clobber, since `update_object`
        // calls `sync_registered_component_props_to_scene_db` unconditionally.
        let mut moved = db.get_object(&id).expect("object should exist");
        moved.transform.position = [1.0, 2.0, 3.0];
        db.update_object(moved);

        let components = db.get_components(&id);
        let light = components
            .iter()
            .find(|c| c.class_name == "LightComponent")
            .expect("LightComponent should still be attached");
        assert_eq!(
            light.data.get("intensity").and_then(|v| v.get("intensity")),
            Some(&serde_json::json!(750.0)),
            "an unrelated update_object call must not revert a live component \
             property edit -- component_store and World must never diverge for \
             typed-path edits"
        );

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        let hydrated = store.world().get::<LightComponent>(entity).unwrap();
        assert_eq!(
            hydrated.intensity.intensity, 750.0,
            "the live World value itself must also survive, not just what \
             get_components reports"
        );
    }
