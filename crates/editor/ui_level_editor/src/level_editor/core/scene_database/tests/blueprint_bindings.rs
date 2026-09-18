use super::*;

#[cfg(test)]
mod blueprint_bindings_preservation_tests {
    //! #650 — editor saves must never destroy a level's Blueprint-binding
    //! section (the editor cannot author it yet, but the runtime loader
    //! consumes it).

    use super::*;

    fn sample_bindings() -> pulsar_scene::BlueprintBindings {
        let mut bindings = pulsar_scene::BlueprintBindings::new();
        bindings.insert(
            "lever_a".to_string(),
            vec![pulsar_scene::BlueprintBinding {
                class_name: "Lever".to_string(),
                overrides: {
                    let mut map = std::collections::HashMap::new();
                    map.insert("speed".to_string(), serde_json::json!(7.5));
                    map
                },
            }],
        );
        bindings
    }

    /// A save over an existing file preserves its `blueprint_bindings`
    /// section byte-for-value, keyed by StableId with overrides intact.
    #[test]
    fn saving_preserves_an_authored_bindings_section() {
        let dir =
            std::env::temp_dir().join(format!("pulsar_650_editor_save_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let path = dir.join("roundtrip.level.json");

        // Seed a file as if hand-authored / written by the runtime tooling
        // (full v2.x shape — objects carry their required fields).
        let seeded = format!(
            r#"{{ "version": "2.1",
                 "objects": [],
                 "metadata": {{"created": "2026-01-01T00:00:00Z", "modified": "2026-01-01T00:00:00Z", "editor_version": "0.1.0"}},
                 "blueprint_bindings": {{"lever_a": [{{"class_name": "Lever", "overrides": {{"speed": 7.5}}}}]}} }}"#
        );
        virtual_fs::write_file(&path, seeded.as_bytes()).expect("seed file");

        // An ordinary editor save (fresh LevelFile construction) must keep it.
        let db = SceneDatabase::new();
        db.save_to_file_with_editor_camera(&path, None, None)
            .expect("save");

        let saved: LevelFile = {
            let bytes = virtual_fs::read_file(&path).expect("read back");
            serde_json::from_str(&String::from_utf8(bytes).unwrap()).expect("parse")
        };
        assert_eq!(
            saved.blueprint_bindings,
            sample_bindings(),
            "bindings survive re-save"
        );

        // Files without the section still save cleanly (no phantom key).
        let bare = dir.join("bare.level.json");
        virtual_fs::write_file(
            &bare,
            r#"{ "version": "2.1", "objects": [],
                 "metadata": {"created": "2026-01-01T00:00:00Z", "modified": "2026-01-01T00:00:00Z", "editor_version": "0.1.0"} }"#
                .as_bytes(),
        )
        .expect("seed bare");
        db.save_to_file(&bare).expect("save bare");
        let saved_bare: LevelFile = {
            let bytes = virtual_fs::read_file(&bare).expect("read back");
            serde_json::from_str(&String::from_utf8(bytes).unwrap()).expect("parse")
        };
        assert!(saved_bare.blueprint_bindings.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Loading a file carrying bindings succeeds (the section is additive,
    /// ignored by the editor today) and the objects load untouched.
    #[test]
    fn loading_a_bound_level_succeeds_and_ignores_the_section_for_now() {
        let db = SceneDatabase::new();
        let json = r#"{
            "version": "2.1",
            "objects": [
                { "id": "lever_a", "name": "Lever A", "object_type": {"Mesh": "Cube"},
                  "transform": {"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]},
                  "parent": null, "visible": true, "locked": false,
                  "children": [], "scene_path": "", "props": {} }
            ],
            "metadata": {"created": "2026-01-01T00:00:00Z", "modified": "2026-01-01T00:00:00Z", "editor_version": "0.1.0"},
            "blueprint_bindings": { "lever_a": [ { "class_name": "Lever", "overrides": {} } ] }
        }"#;
        let path = std::env::temp_dir().join(format!(
            "pulsar_650_editor_load_{}.json",
            std::process::id()
        ));
        virtual_fs::write_file(&path, json.as_bytes()).expect("write");

        db.load_from_file(&path).expect("bound levels load");
        let objects = db.get_all_objects();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].id, "lever_a");

        let _ = std::fs::remove_file(&path);
    }
}