use super::*;
use crate::builtin::{GAME_MODE_TRAIT_ID, GAME_MODE_TRAIT_PATH};
use crate::sources::{TraitImplementerSource, NATIVE_KIND};

// ── Fixtures ─────────────────────────────────────────────────────────────────

/// A stand-in implementer kind: `*.impl.json` files declaring traits the way
/// every source reports them.
fn claims_impl(relative: &str) -> bool {
    relative.ends_with(".impl.json")
}

fn scan_impl(relative: &str, bytes: &[u8]) -> Option<Scanned> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let list = |key: &str| -> Vec<String> {
        value[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    Some(Scanned {
        id: relative.to_owned(),
        declared: Declared {
            ids: list("ids"),
            paths: list("paths"),
        },
        binding: Some(Binding::ScriptClass {
            dir: relative.rsplit_once('/').map_or("", |(dir, _)| dir).to_owned(),
        }),
    })
}

inventory::submit! {
    TraitImplementerSource {
        origin: pulsar_plugins::engine_origin!(),
        kind: "test.impl",
        label: "Test implementers",
        claims: Some(claims_impl),
        scan: Some(scan_impl),
        list: None,
    }
}

/// Engine-mounted fixtures: a trait a native component implements, and two
/// the index must refuse.
const TEST_FILES: &[engine_fs::MountedFile] = &[
    engine_fs::MountedFile {
        path: "test/greeter.trait.json",
        bytes: br#"{"schemaVersion":1,"typeKind":"trait","id":"pulsar.TestGreeter","name":"TestGreeter","displayName":"Test Greeter","methods":[{"name":"add_charges","signature":{"params":[{"name":"amount","type":{"kind":"Primitive","name":"i32"}}],"return":{"kind":"Primitive","name":"i32"}}}],"meta":{"contractVersion":1}}"#,
    },
    engine_fs::MountedFile {
        path: "test/outside.trait.json",
        bytes: br#"{"schemaVersion":1,"typeKind":"trait","id":"elsewhere.Outside","name":"Outside","displayName":"Outside","methods":[],"meta":{"contractVersion":1}}"#,
    },
    engine_fs::MountedFile {
        path: "test/unversioned.trait.json",
        bytes: br#"{"schemaVersion":1,"typeKind":"trait","id":"pulsar.Unversioned","name":"Unversioned","displayName":"Unversioned","methods":[],"meta":{}}"#,
    },
];

inventory::submit! {
    engine_fs::MountedContent {
        origin: pulsar_plugins::engine_origin!(),
        id: "pulsar.traits.test",
        files: TEST_FILES,
    }
}

fn charges_methods() -> Vec<pulsar_reflection::MethodMetadata> {
    let i32_info = pulsar_reflection::RUNTIME_TYPE_REGISTRY
        .get::<i32>()
        .expect("i32 registered");
    vec![pulsar_reflection::MethodMetadata {
        name: "add_charges",
        display_name: "Add Charges".into(),
        category: None,
        params: vec![pulsar_reflection::MethodParameter {
            name: "amount",
            type_info: i32_info,
        }],
        return_type: Some(pulsar_reflection::MethodReturnType { type_info: i32_info }),
        flags: pulsar_reflection::MethodFlags::NONE,
        caller: Box::new(|_, _| None),
    }]
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::ComponentMethodRegistration {
        class_name: "TestCharger",
        methods: charges_methods,
    }
}

inventory::submit! {
    crate::NativeTraitImpl {
        origin: pulsar_plugins::engine_origin!(),
        class: "TestCharger",
        trait_id: "pulsar.TestGreeter",
        methods: &[],
    }
}

// A component that claims the trait without the method.
inventory::submit! {
    crate::NativeTraitImpl {
        origin: pulsar_plugins::engine_origin!(),
        class: "TestSilent",
        trait_id: "pulsar.TestGreeter",
        methods: &[],
    }
}

fn trait_json(name: &str, id: Option<&str>) -> String {
    let mut value = json!({
        "schemaVersion": 1, "typeKind": "trait", "name": name, "displayName": name,
        "methods": [], "meta": {}
    });
    if let Some(id) = id {
        value["id"] = json!(id);
    }
    value.to_string()
}

fn impl_json(ids: &[&str], paths: &[&str]) -> String {
    json!({ "ids": ids, "paths": paths }).to_string()
}

fn put(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    virtual_fs::create_dir_all(path.parent().unwrap()).unwrap();
    virtual_fs::write_file(&path, text.as_bytes()).unwrap();
}

fn id_in_file(root: &Path, relative: &str) -> String {
    let value: Value =
        serde_json::from_slice(&virtual_fs::read_file(&root.join(relative)).unwrap()).unwrap();
    value["id"].as_str().unwrap().to_owned()
}

// ── Identity and references ──────────────────────────────────────────────────

#[test]
fn legacy_path_references_resolve_and_traits_get_ids() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "types/traits/movable.trait.json", &trait_json("Movable", None));
    put(
        dir.path(),
        "classes/Player/player.impl.json",
        &impl_json(&[], &["types\\traits\\movable.trait.json"]),
    );
    let index = TraitIndex::rebuild(dir.path()).unwrap();
    let id = id_in_file(dir.path(), "types/traits/movable.trait.json");
    let entry = index.entry("classes/Player/player.impl.json").unwrap();
    assert_eq!(entry.implemented_trait_ids, [id.clone()]);
    assert_eq!(entry.kind, "test.impl");
    assert!(entry.path_only);
    assert_eq!(index.implementers_of(&id).len(), 1);
    assert_eq!(
        index.db().kind_of("classes/Player/player.impl.json"),
        Some("test.impl")
    );
}

#[test]
fn a_renamed_trait_keeps_its_implementers_and_a_second_rebuild_is_stable() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "types/traits/a.trait.json", &trait_json("A", Some("ta")));
    put(dir.path(), "x/one.impl.json", &impl_json(&["ta"], &[]));
    let first = TraitIndex::rebuild(dir.path()).unwrap();
    virtual_fs::rename(
        &dir.path().join("types/traits/a.trait.json"),
        &dir.path().join("types/traits/renamed.trait.json"),
    )
    .unwrap();
    let second = TraitIndex::rebuild(dir.path()).unwrap();
    assert_eq!(second.implementers_of("ta").len(), 1);
    assert_eq!(
        second.entry("x/one.impl.json").unwrap().implemented_traits,
        ["types/traits/renamed.trait.json"]
    );
    assert_ne!(first, second);
    assert_eq!(second, TraitIndex::rebuild(dir.path()).unwrap());
}

#[test]
fn copied_trait_files_get_distinct_identities() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.trait.json", &trait_json("A", Some("same")));
    put(dir.path(), "b.trait.json", &trait_json("A2", Some("same")));
    TraitIndex::rebuild(dir.path()).unwrap();
    assert_eq!(id_in_file(dir.path(), "a.trait.json"), "same");
    assert_ne!(id_in_file(dir.path(), "b.trait.json"), "same");
}

#[test]
fn missing_references_are_reported_not_resolved() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "x/one.impl.json",
        &impl_json(&["gone-id"], &["types/traits/gone.trait.json", "../escape.trait.json"]),
    );
    let index = TraitIndex::rebuild(dir.path()).unwrap();
    let entry = index.entry("x/one.impl.json").unwrap();
    assert!(entry.implemented_trait_ids.is_empty());
    assert_eq!(entry.unresolved, ["gone-id", "types/traits/gone.trait.json"]);
}

#[test]
fn supertraits_make_subtrait_implementers_implementers_of_the_base() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "base.trait.json", &trait_json("Base", Some("base")));
    let mut sub: Value = serde_json::from_str(&trait_json("Sub", Some("sub"))).unwrap();
    sub["supertraits"] = json!(["base"]);
    put(dir.path(), "sub.trait.json", &sub.to_string());
    put(dir.path(), "x/one.impl.json", &impl_json(&["sub"], &[]));
    let index = TraitIndex::rebuild(dir.path()).unwrap();
    assert_eq!(index.implementers_of("base").len(), 1);
    assert_eq!(
        index.db().implementers("base")[0].via,
        Some("sub"),
        "inherited through the subtrait"
    );
}

#[test]
fn update_path_matches_a_full_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "types/traits/a.trait.json", &trait_json("A", Some("ta")));
    put(dir.path(), "x/one.impl.json", &impl_json(&["ta"], &[]));
    TraitIndex::rebuild(dir.path()).unwrap();

    put(dir.path(), "types/traits/b.trait.json", &trait_json("B", Some("tb")));
    TraitIndex::update_path(dir.path(), Path::new("types/traits/b.trait.json")).unwrap();
    put(dir.path(), "x/two.impl.json", &impl_json(&["tb", "ta"], &[]));
    let updated =
        TraitIndex::update_path(dir.path(), &dir.path().join("x/two.impl.json")).unwrap();
    assert_eq!(updated, TraitIndex::rebuild(dir.path()).unwrap());

    virtual_fs::delete_path(&dir.path().join("types/traits/b.trait.json")).unwrap();
    let updated =
        TraitIndex::update_path(dir.path(), Path::new("types/traits/b.trait.json")).unwrap();
    assert_eq!(
        updated.entry("x/two.impl.json").unwrap().unresolved,
        ["tb"]
    );
    assert_eq!(updated, TraitIndex::rebuild(dir.path()).unwrap());
}

#[test]
fn lookups_accept_either_slash_style_and_reject_unsafe_paths() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "classes/One/one.impl.json", &impl_json(&[], &[]));
    let index = TraitIndex::rebuild(dir.path()).unwrap();
    assert!(index.traits_for("classes\\One\\one.impl.json").is_some());
    assert!(index.traits_for("../classes/One/one.impl.json").is_none());
    assert!(index.implementers_of("../x.trait.json").is_empty());
}

#[test]
fn load_rejects_an_older_schema() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        TraitIndex::RELATIVE_PATH,
        r#"{"schema_version":2,"traits":[],"implementers":[]}"#,
    );
    assert!(TraitIndex::load(dir.path()).is_err());
}

#[test]
fn the_type_index_is_cross_referenced() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "types/traits/a.trait.json", &trait_json("Named", Some("ta")));
    put(dir.path(), "type-index/index.json", r#"{"types":{"traits":{"Named":{}}}}"#);
    let index = TraitIndex::rebuild(dir.path()).unwrap();
    assert_eq!(
        index.resolve_trait("ta").unwrap().type_index_id.as_deref(),
        Some("trait:Named")
    );
}

// ── Mounts and namespaces ────────────────────────────────────────────────────

#[test]
fn the_engines_traits_come_from_its_read_only_mount() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "classes/Mode/mode.impl.json", &impl_json(&[], &[GAME_MODE_TRAIT_PATH]));
    let index = TraitIndex::rebuild(dir.path()).unwrap();
    let record = index.resolve_trait(GAME_MODE_TRAIT_ID).unwrap();
    assert_eq!(record.path, GAME_MODE_TRAIT_PATH);
    assert!(record.builtin);
    assert_eq!(index.implementers_of(GAME_MODE_TRAIT_ID).len(), 1);

    // Read-only: the engine's file cannot be changed, and it is not stored as
    // the project's (the stored index is the project's part only).
    let error = virtual_fs::write_file(&dir.path().join(GAME_MODE_TRAIT_PATH), b"{}").unwrap_err();
    assert!(virtual_fs::is_read_only(&error), "{error:#}");
    let stored: Value = serde_json::from_str(
        &virtual_fs::read_to_string(&dir.path().join(TraitIndex::RELATIVE_PATH)).unwrap(),
    )
    .unwrap();
    assert!(stored["traits"].as_array().unwrap().is_empty());
    assert_eq!(
        TraitIndex::load(dir.path()).unwrap().resolve_trait(GAME_MODE_TRAIT_ID).unwrap().path,
        GAME_MODE_TRAIT_PATH
    );
}

#[test]
fn an_old_projects_copy_of_a_built_in_is_refused_not_resynced() {
    let dir = tempfile::tempdir().unwrap();
    let copy = String::from_utf8(
        crate::builtin::TRAIT_FILES
            .iter()
            .find(|file| file.path.ends_with("game_mode.trait.json"))
            .unwrap()
            .bytes
            .to_vec(),
    )
    .unwrap();
    put(dir.path(), "types/traits/game_mode.trait.json", &copy);
    let index = TraitIndex::rebuild(dir.path()).unwrap();
    assert!(index
        .problems()
        .iter()
        .any(|p| p.starts_with("types/traits/game_mode.trait.json: ") && p.contains("reserved")));
    assert_eq!(index.resolve_trait(GAME_MODE_TRAIT_ID).unwrap().path, GAME_MODE_TRAIT_PATH);
    // The file is left as it was.
    assert_eq!(
        virtual_fs::read_to_string(&dir.path().join("types/traits/game_mode.trait.json")).unwrap(),
        copy
    );
    // Stored, so a load reports it too.
    assert!(TraitIndex::load(dir.path())
        .unwrap()
        .problems()
        .iter()
        .any(|p| p.contains("reserved")));
}

#[test]
fn mounted_traits_outside_their_namespace_or_unversioned_are_refused() {
    let index = TraitIndex::live(tempfile::tempdir().unwrap().path());
    assert!(index.resolve_trait("elsewhere.Outside").is_none());
    assert!(index.resolve_trait("pulsar.Unversioned").is_none());
    let problems = index.problems().join("\n");
    assert!(problems.contains("outside its owner's namespace"), "{problems}");
    assert!(problems.contains("contractVersion"), "{problems}");
}

// ── Native implementers ──────────────────────────────────────────────────────

#[test]
fn native_components_implement_traits_through_their_registered_methods() {
    let index = TraitIndex::live(tempfile::tempdir().unwrap().path());
    let entry = index.entry("component:TestCharger").unwrap();
    assert_eq!(entry.kind, NATIVE_KIND);
    assert_eq!(entry.implemented_trait_ids, ["pulsar.TestGreeter"]);
    assert_eq!(
        entry.binding,
        Some(Binding::Component {
            class: "TestCharger".into()
        })
    );
    // Conforms: no problem names it. The one without the method is reported.
    let problems = index.problems().join("\n");
    assert!(!problems.contains("component:TestCharger"), "{problems}");
    assert!(problems.contains("component:TestSilent"), "{problems}");
    assert!(problems.contains("add_charges"), "{problems}");
    assert_eq!(index.implementers_of("pulsar.TestGreeter").len(), 2);
}
