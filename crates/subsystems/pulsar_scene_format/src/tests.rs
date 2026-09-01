//! Verification for the canonical schema (Pulsar-Native#557, Phase B6).
//!
//! Three obligations, from the issue's own verification list:
//!
//! 1. **Round-trip** — a representative scene survives
//!    serialize → deserialize unchanged.
//! 2. **Backward compat** — a v2 nested-transform document *and* a true v1
//!    flat-transform document both deserialize correctly. Because both the
//!    editor's `LevelFile`/`SceneObjectData` and the runtime's
//!    `SceneFile`/`SceneObject` are aliases of these types, passing here
//!    means both consumers support both formats.
//! 3. **Superset dispositions** — the editor's full `ObjectType` coverage and
//!    `LightType::Area` parse, and unknown spellings still degrade instead of
//!    failing.

use super::*;

// ── Fixtures ───────────────────────────────────────────────────────────────

/// A v2.x document in exactly the shape the editor writes today: string
/// version, nested `transform`, `children`/`scene_path`/`locked` present, a
/// typed `components` section, `metadata`, and an `editor.camera`.
const V2_NESTED: &str = r#"{
    "version": "2.1",
    "objects": [
        {
            "id": "sun", "name": "Sun", "object_type": {"Light": "Point"},
            "transform": {"position": [1.0, 5.0, 2.0], "rotation": [10.0, 20.0, 30.0], "scale": [2.0, 2.0, 2.0]},
            "visible": true, "locked": false, "parent": null,
            "children": ["cube"], "scene_path": "Sun", "props": {"intensity": 4.0}
        },
        {
            "id": "cube", "name": "Cube", "object_type": {"Mesh": "Cube"},
            "transform": {"position": [0.0, 1.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]},
            "visible": true, "locked": true, "parent": "sun",
            "children": [], "scene_path": "Sun/Cube", "props": {},
            "component_instances": [{"class_name": "StaticMeshComponent", "data": {"mesh_asset": "a.mesh"}}]
        }
    ],
    "components": {
        "cube": [{"class_name": "StaticMeshComponent", "enabled": true, "data": {"mesh_asset": "a.mesh"}}]
    },
    "blueprint_bindings": {
        "cube": [{"class_name": "Lever", "overrides": {"speed": 7.5}}]
    },
    "metadata": {"created": "2026-01-01T00:00:00Z", "modified": "2026-01-02T00:00:00Z", "editor_version": "0.1.0"},
    "editor": {"camera": {"position": [3.0, 4.0, 5.0], "yaw": 0.5, "pitch": -0.25}}
}"#;

/// A **true v1** document: integer version, flat top-level `position`/
/// `rotation`/`scale` per object, no nested `transform`, and none of the
/// editor-only sections. Before B6 only the runtime type modelled this; the
/// editor's own type did not, so this fixture is the regression guard for
/// disposition 2 ("v1 flat-transform fallback is kept, on both consumers").
const V1_FLAT: &str = r#"{
    "version": 1,
    "objects": [
        {
            "id": "ground", "name": "Ground", "object_type": {"Mesh": "Plane"},
            "position": [1.0, 2.0, 3.0],
            "rotation": [0.0, 90.0, 0.0],
            "scale": [10.0, 1.0, 10.0],
            "props": {"base_color": [0.2, 0.3, 0.4, 1.0]}
        },
        {
            "id": "lamp", "name": "Lamp", "object_type": {"Light": "Spot"},
            "position": [0.0, 5.0, 0.0]
        }
    ]
}"#;

// ── 1. Round-trip ──────────────────────────────────────────────────────────

#[test]
fn representative_scene_round_trips() {
    let original: SceneFile = serde_json::from_str(V2_NESTED).expect("fixture parses");
    let text = serde_json::to_string(&original).expect("serialize");
    let again: SceneFile = serde_json::from_str(&text).expect("re-parse");

    assert_eq!(again.version_string(), "2.1");
    assert_eq!(again.objects.len(), original.objects.len());
    for (a, b) in original.objects.iter().zip(&again.objects) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.name, b.name);
        assert_eq!(a.object_type, b.object_type);
        assert_eq!(a.transform, b.transform);
        assert_eq!(a.visible, b.visible);
        assert_eq!(a.locked, b.locked);
        assert_eq!(a.parent, b.parent);
        assert_eq!(a.children, b.children);
        assert_eq!(a.scene_path, b.scene_path);
        assert_eq!(a.props, b.props);
        assert_eq!(a.component_instances, b.component_instances);
    }
    assert_eq!(again.blueprint_bindings, original.blueprint_bindings);
    assert_eq!(again.metadata.created, original.metadata.created);
    assert_eq!(again.metadata.modified, original.metadata.modified);
    assert_eq!(again.metadata.editor_version, original.metadata.editor_version);
    assert_eq!(
        again.editor.as_ref().and_then(|e| e.camera),
        original.editor.as_ref().and_then(|e| e.camera),
    );

    let cube_components = &again.components["cube"];
    assert_eq!(cube_components.len(), 1);
    assert_eq!(cube_components[0].class_name, "StaticMeshComponent");
    assert!(cube_components[0].enabled);
    assert_eq!(cube_components[0].data["mesh_asset"], "a.mesh");
}

// ── 2. Backward compatibility ──────────────────────────────────────────────

#[test]
fn v2_nested_transform_document_loads() {
    let file: SceneFile = serde_json::from_str(V2_NESTED).expect("v2 parses");
    assert!(file.is_supported_version());

    let sun = &file.objects[0];
    assert_eq!(sun.object_type, ObjectType::Light(LightType::Point));
    assert_eq!(sun.world_position(), [1.0, 5.0, 2.0]);
    assert_eq!(sun.world_rotation(), [10.0, 20.0, 30.0]);
    assert_eq!(sun.world_scale(), [2.0, 2.0, 2.0]);
    assert_eq!(sun.children, vec!["cube".to_string()]);
    assert_eq!(sun.scene_path, "Sun");
    assert!(!sun.locked);

    let cube = &file.objects[1];
    assert_eq!(cube.object_type, ObjectType::Mesh(MeshType::Cube));
    assert_eq!(cube.parent.as_deref(), Some("sun"));
    assert!(cube.locked);
    assert!(cube.component_instances.is_some());

    let camera = file.editor.and_then(|e| e.camera).expect("camera present");
    assert_eq!(camera.position, [3.0, 4.0, 5.0]);
    assert_eq!(camera.yaw, 0.5);
    assert_eq!(camera.pitch, -0.25);
}

#[test]
fn v1_flat_transform_document_loads_on_both_consumers() {
    let file: SceneFile = serde_json::from_str(V1_FLAT).expect("v1 parses");
    assert_eq!(file.version_string(), "1");
    assert!(file.is_supported_version());

    // The flat fields are folded into the nested transform, so the v1 file
    // reads back through exactly the same accessors a v2 file does.
    let ground = &file.objects[0];
    assert_eq!(ground.world_position(), [1.0, 2.0, 3.0]);
    assert_eq!(ground.world_rotation(), [0.0, 90.0, 0.0]);
    assert_eq!(ground.world_scale(), [10.0, 1.0, 10.0]);
    assert_eq!(ground.transform.position, [1.0, 2.0, 3.0]);
    assert_eq!(ground.object_type, ObjectType::Mesh(MeshType::Plane));

    // Absent optional fields take the documented defaults.
    let lamp = &file.objects[1];
    assert_eq!(lamp.world_position(), [0.0, 5.0, 0.0]);
    assert_eq!(lamp.world_scale(), [1.0, 1.0, 1.0], "scale defaults to unit");
    assert!(lamp.visible, "visible defaults to true");
    assert!(!lamp.locked);
    assert_eq!(lamp.parent, None);
    assert!(lamp.children.is_empty());
    assert_eq!(lamp.scene_path, "");
    assert!(lamp.props.is_empty());

    // Editor-only sections are absent, not fatal.
    assert!(file.components.is_empty());
    assert!(file.blueprint_bindings.is_empty());
    assert_eq!(file.metadata.created, "");
    assert!(file.editor.is_none());
}

#[test]
fn a_v1_file_that_is_loaded_and_re_saved_keeps_its_transform() {
    // Before B6 the flat fields lived on the struct alongside an all-default
    // nested `transform`, so a load/save cycle serialized both and the
    // "which wins" rule had to be re-applied by every reader. Folding at
    // parse time means the re-saved file is plain v2.
    let file: SceneFile = serde_json::from_str(V1_FLAT).expect("v1 parses");
    let text = serde_json::to_string(&file).expect("serialize");
    let again: SceneFile = serde_json::from_str(&text).expect("re-parse");
    assert_eq!(again.objects[0].world_position(), [1.0, 2.0, 3.0]);
    assert_eq!(again.objects[0].world_scale(), [10.0, 1.0, 10.0]);

    // The re-saved object carries only the nested `transform`: the flat keys
    // are a deserialize-only fallback, never re-emitted.
    let raw: Value = serde_json::from_str(&text).expect("valid json");
    let object = raw["objects"][0].as_object().expect("object");
    assert!(object.contains_key("transform"));
    for flat in ["position", "rotation", "scale"] {
        assert!(!object.contains_key(flat), "`{flat}` must not be re-emitted");
    }
}

#[test]
fn nested_transform_wins_over_flat_fields_when_both_are_present() {
    // The pre-B6 `world_position()`/`world_rotation()`/`world_scale()` rule,
    // preserved verbatim: a non-default nested component wins, otherwise the
    // flat field is used.
    let json = r#"{
        "version": "2.1",
        "objects": [{
            "id": "x", "name": "X", "object_type": "Empty",
            "transform": {"position": [9.0, 9.0, 9.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]},
            "position": [1.0, 1.0, 1.0], "rotation": [4.0, 5.0, 6.0], "scale": [7.0, 7.0, 7.0]
        }]
    }"#;
    let file: SceneFile = serde_json::from_str(json).expect("parses");
    let obj = &file.objects[0];
    assert_eq!(obj.world_position(), [9.0, 9.0, 9.0], "nested position wins");
    assert_eq!(obj.world_rotation(), [4.0, 5.0, 6.0], "default nested rotation yields to flat");
    assert_eq!(obj.world_scale(), [7.0, 7.0, 7.0], "default nested scale yields to flat");
}

// ── 3. Superset dispositions ───────────────────────────────────────────────

#[test]
fn object_type_covers_the_editor_superset() {
    let json = r#"{
        "version": "2.1",
        "objects": [
            {"id": "a", "name": "A", "object_type": "ParticleSystem"},
            {"id": "b", "name": "B", "object_type": "AudioSource"},
            {"id": "c", "name": "C", "object_type": "Blueprint"},
            {"id": "d", "name": "D", "object_type": {"Light": "Area"}},
            {"id": "e", "name": "E", "object_type": "Folder"},
            {"id": "f", "name": "F", "object_type": "Camera"}
        ]
    }"#;
    let file: SceneFile = serde_json::from_str(json).expect("superset parses");
    let types: Vec<ObjectType> = file.objects.iter().map(|o| o.object_type).collect();
    assert_eq!(
        types,
        vec![
            ObjectType::ParticleSystem,
            ObjectType::AudioSource,
            ObjectType::Blueprint,
            ObjectType::Light(LightType::Area),
            ObjectType::Folder,
            ObjectType::Camera,
        ]
    );

    // …and every one of them survives a round-trip through the wire format.
    let text = serde_json::to_string(&file).expect("serialize");
    let again: SceneFile = serde_json::from_str(&text).expect("re-parse");
    assert_eq!(
        again.objects.iter().map(|o| o.object_type).collect::<Vec<_>>(),
        types
    );
    assert!(text.contains(r#""object_type":"ParticleSystem""#));
    assert!(text.contains(r#""object_type":{"Light":"Area"}"#));
}

#[test]
fn unrecognised_type_spellings_degrade_instead_of_failing() {
    let json = r#"{
        "version": "2.1",
        "objects": [
            {"id": "a", "name": "A", "object_type": "SomethingFromTheFuture"},
            {"id": "b", "name": "B", "object_type": {"Mesh": "Torus"}},
            {"id": "c", "name": "C", "object_type": {"Light": "Volumetric"}},
            {"id": "d", "name": "D", "object_type": {"Nonsense": 3}},
            {"id": "e", "name": "E", "object_type": 42}
        ]
    }"#;
    let file: SceneFile = serde_json::from_str(json).expect("unknown types never fail the parse");
    assert_eq!(file.objects[0].object_type, ObjectType::Empty);
    assert_eq!(file.objects[1].object_type, ObjectType::Mesh(MeshType::Cube));
    assert_eq!(file.objects[2].object_type, ObjectType::Light(LightType::Point));
    assert_eq!(file.objects[3].object_type, ObjectType::Empty);
    assert_eq!(file.objects[4].object_type, ObjectType::Empty);
}

// ── Leniency of the editor-authored sections ───────────────────────────────

#[test]
fn malformed_editor_sections_are_dropped_not_fatal() {
    // The runtime carried these as opaque `Value`s before B6; typing them
    // must not turn a previously-loadable file into a parse error.
    let json = r#"{
        "version": "2.1",
        "objects": [],
        "components": {"x": [{"data": {}}, {"class_name": "Ok", "data": {}}], "y": 5},
        "metadata": "not an object",
        "editor": 12
    }"#;
    let file: SceneFile = serde_json::from_str(json).expect("lenient sections");
    assert_eq!(file.components["x"].len(), 1, "class_name-less entry skipped");
    assert_eq!(file.components["x"][0].class_name, "Ok");
    assert!(file.components["x"][0].enabled, "missing `enabled` means enabled");
    assert!(!file.components.contains_key("y"), "non-array entry dropped");
    assert_eq!(file.metadata.created, "");
    assert!(file.editor.is_none());
}

#[test]
fn component_enabled_flag_round_trips() {
    let json = r#"{
        "version": "2.1", "objects": [],
        "components": {"x": [{"class_name": "A", "enabled": false, "data": {"v": 1}}]}
    }"#;
    let file: SceneFile = serde_json::from_str(json).expect("parses");
    assert!(!file.components["x"][0].enabled);
    let again: SceneFile = serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
    assert!(!again.components["x"][0].enabled);
    assert_eq!(again.components["x"][0].data["v"], 1);
}

// ── Blueprint class bindings (#650) ────────────────────────────────────────
//
// Moved verbatim from `pulsar_scene::format`'s own test module when that
// module became a re-export of this crate.

/// The additive guarantee: a pre-#650 file (no `blueprint_bindings` key)
/// deserializes with empty bindings.
#[test]
fn old_files_without_bindings_load_unchanged() {
    let json = r#"{
        "version": "2.1",
        "objects": [
            { "id": "cube", "name": "Cube", "object_type": {"Mesh": "Cube"}, "props": {} }
        ]
    }"#;
    let file: SceneFile = serde_json::from_str(json).expect("old file parses");
    assert!(file.blueprint_bindings.is_empty());
}

/// A bindings section round-trips, keyed by StableId with per-instance
/// overrides; re-serializing drops the key when empty.
#[test]
fn bindings_round_trip_and_skip_serializing_when_empty() {
    let json = r#"{
        "version": "2.1",
        "objects": [],
        "blueprint_bindings": {
            "lever_a": [
                { "class_name": "Lever", "overrides": { "speed": 7.5 } },
                { "class_name": "Alarm" }
            ]
        }
    }"#;
    let file: SceneFile = serde_json::from_str(json).expect("bindings parse");
    let lever = &file.blueprint_bindings["lever_a"];
    assert_eq!(lever.len(), 2, "multiple classes may bind to one object");
    assert_eq!(lever[0].class_name, "Lever");
    assert_eq!(lever[0].overrides.get("speed").and_then(Value::as_f64), Some(7.5));
    assert!(lever[1].overrides.is_empty(), "missing overrides means defaults");

    let rewritten = serde_json::to_string(&file).expect("serialize");
    assert!(rewritten.contains("blueprint_bindings"), "non-empty section is written");
    assert!(rewritten.contains(r#""overrides":{"speed":7.5}"#), "overrides survive");

    let empty: SceneFile = serde_json::from_str(r#"{ "version": 1 }"#).unwrap();
    assert!(
        !serde_json::to_string(&empty).unwrap().contains("blueprint_bindings"),
        "empty section stays out of the file (byte-compat with pre-#650 writers)"
    );
}

/// Bindings reference objects by StableId only — the type has no
/// name field to drift out of sync (#B2 identity rule).
#[test]
fn binding_type_carries_no_object_name() {
    let binding: BlueprintBinding =
        serde_json::from_str(r#"{ "class_name": "Enemy", "overrides": {} }"#).unwrap();
    assert_eq!(binding.class_name, "Enemy");
}

// ── Wire-shape guards ──────────────────────────────────────────────────────

#[test]
fn editor_written_keys_are_all_emitted() {
    // Regression guard for the merge itself: the runtime type used to mark
    // `components`/`metadata`/`editor`/`locked`/`children`/`scene_path` as
    // `skip_serializing`. Now that both consumers share one type, a save from
    // either side must still produce the editor's full document.
    let file: SceneFile = serde_json::from_str(V2_NESTED).expect("parses");
    let text = serde_json::to_string(&file).expect("serialize");
    for key in [
        "version",
        "objects",
        "components",
        "blueprint_bindings",
        "metadata",
        "editor",
        "object_type",
        "transform",
        "visible",
        "locked",
        "parent",
        "children",
        "scene_path",
        "props",
        "component_instances",
    ] {
        assert!(text.contains(&format!("\"{key}\"")), "`{key}` must be written");
    }
}

#[test]
fn component_instances_is_omitted_when_absent() {
    let file: SceneFile = serde_json::from_str(V1_FLAT).expect("parses");
    let text = serde_json::to_string(&file).expect("serialize");
    assert!(
        !text.contains("component_instances"),
        "None must stay out of the file"
    );
}
