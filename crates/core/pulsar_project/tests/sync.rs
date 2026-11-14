//! Sync against a miniature engine checkout: what is generated and where,
//! that nothing holds a machine path, that a second sync does nothing, and
//! how changes to the engine, the project and generated files are handled.

mod common;

use std::path::Path;

use common::{cargo, project, read, tree, write, Engine};
use pulsar_project::sdk::SdkKind;
use pulsar_project::{sync_with, EngineSdk, SyncError, SyncOptions, SyncReport};

/// Every string in `value` that is an absolute path on any platform.
fn absolute_paths(value: &toml::Value) -> Vec<String> {
    match value {
        toml::Value::String(s) => {
            let b = s.as_bytes();
            let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\');
            if drive || s.starts_with('/') || s.starts_with('\\') {
                vec![s.clone()]
            } else {
                Vec::new()
            }
        }
        toml::Value::Array(items) => items.iter().flat_map(absolute_paths).collect(),
        toml::Value::Table(t) => t.values().flat_map(absolute_paths).collect(),
        _ => Vec::new(),
    }
}

fn sync(project: &Path, engine: &Engine) -> SyncReport {
    let sdk = EngineSdk::at(&engine.root).expect("the fixture is an engine checkout");
    sync_with(project, &sdk, SyncOptions::default()).expect("sync")
}

struct Fixture {
    _dir: tempfile::TempDir,
    engine: Engine,
    project: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(dir.path().join("engine"));
    let project_dir = dir.path().join("Star Fall Project");
    project(&project_dir);
    Fixture {
        _dir: dir,
        engine,
        project: project_dir,
    }
}

#[test]
fn a_synced_project_is_a_portable_cargo_workspace() {
    let f = fixture();
    let report = sync(&f.project, &f.engine);
    assert_eq!(report.package, "star_fall");
    assert_eq!(report.bin, "star_fall");
    assert_eq!(report.lib, "star_fall_pie");
    assert!(report.lock_seeded && report.gitignore_updated);
    assert!(report.problems.is_empty(), "{:?}", report.problems);

    let root = read(&f.project.join("Cargo.toml"));
    let game = read(&f.project.join(".pulsar/build/game/Cargo.toml"));
    for (name, text) in [("Cargo.toml", &root), ("game Cargo.toml", &game)] {
        let engine_path = f.engine.root.to_string_lossy().replace('\\', "/");
        assert!(!text.replace('\\', "/").contains(&engine_path), "{name} names the engine");
        let doc = toml::from_str::<toml::Table>(text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let absolute = absolute_paths(&toml::Value::Table(doc));
        assert!(absolute.is_empty(), "{name} has absolute paths: {absolute:?}");
    }
    assert!(root.contains("members = [\".pulsar/build/game\", \"scripts/fall\"]"));
    assert!(root.contains("exclude = [\".pulsar/sdk\"]"));
    assert!(root.contains("pulsar_game = { path = \".pulsar/sdk/crates/pulsar_game\" }"));
    assert!(root.contains("[patch.\"https://example.com/upstream\"]"));
    assert!(!root.contains("unused_editor"), "the editor-only crate is not the game's");
    assert!(root.contains("tracing = { opt-level = 3 }"));
    assert!(!root.contains("not_in_the_game"));
    assert!(game.contains("lights_plugin = { workspace = true }"));
    assert!(!game.contains("tool_plugin"), "an editor tool is not linked");
    assert!(!game.contains("fixture_plugin"), "a plugin the engine does not install is not linked");
    assert!(game.contains("fall_scripts = { path = \"../../../scripts/fall\" }"));
    assert!(game.contains("tracing-subscriber = { features = [\"env-filter\", \"fmt\"], workspace = true }"));
    assert!(!game.contains("engine_class_derive"), "no class is compiled to Rust");

    let setup = read(&f.project.join(".pulsar/build/game/src/setup.rs"));
    assert!(setup.contains("fall_scripts::register_scripts(game)?;"));
    let plugins = read(&f.project.join(".pulsar/build/game/src/plugins.rs"));
    assert!(plugins.contains("use lights_plugin as _;"));

    // The mirror: what the game's graph activates, normalized, and nothing
    // a build does not read.
    let sdk = f.project.join(".pulsar/sdk");
    let mirrored = read(&sdk.join("crates/pulsar_game/Cargo.toml"));
    assert!(!mirrored.contains("workspace = true"));
    let mirrored: toml::Table = toml::from_str(&mirrored).unwrap();
    assert!(!mirrored.contains_key("lints") && !mirrored.contains_key("dev-dependencies"));
    assert_eq!(mirrored["package"]["version"].as_str(), Some("0.4.0"));
    assert!(sdk.join("crates/pulsar_game/shaders/sky.wgsl").is_file());
    assert!(
        sdk.join("crates/engine_class_derive/Cargo.toml").is_file(),
        "a local proc-macro crate is mirrored"
    );
    for absent in [
        "crates/pulsar_game/tests",
        "crates/pulsar_game/target",
        "crates/pulsar_game/.git",
        "plugins/lights/editor",
        "crates/unused_editor",
        "plugins/tool",
    ] {
        assert!(!sdk.join(absent).exists(), "{absent} is mirrored");
    }

    let gitignore = read(&f.project.join(".gitignore"));
    for line in ["/target/", "/.pulsar/sdk/", "/.pulsar/build/"] {
        assert!(gitignore.lines().any(|l| l == line), "{line}");
    }

    // Cargo resolves it, offline, to the expected packages.
    let output = cargo()
        .args(["metadata", "--offline", "--format-version", "1", "--manifest-path"])
        .arg(f.project.join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cargo metadata:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let names: Vec<&str> = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    for expected in ["star_fall", "fall_scripts", "pulsar_game", "lights_plugin", "lights_component"] {
        assert!(names.contains(&expected), "{expected} not in {names:?}");
    }
    assert!(!names.contains(&"lights_editor"));
}

#[test]
fn a_second_sync_changes_nothing() {
    let f = fixture();
    sync(&f.project, &f.engine);
    let before = tree(&f.project);
    let report = sync(&f.project, &f.engine);
    assert!(!report.changed(), "{report:?}");
    assert_eq!(report.sdk.written, 0);
    assert!(!report.gitignore_updated);
    assert_eq!(tree(&f.project), before);
}

#[test]
fn the_same_project_in_two_places_generates_the_same_files() {
    let f = fixture();
    let elsewhere = f.project.parent().unwrap().join("another").join("checkout");
    project(&elsewhere);
    sync(&f.project, &f.engine);
    sync(&elsewhere, &f.engine);
    for file in [
        "Cargo.toml",
        "Cargo.lock",
        ".pulsar/build/game/Cargo.toml",
        ".pulsar/build/game/src/main.rs",
        ".pulsar/build/game/src/lib.rs",
        ".pulsar/build/game/src/setup.rs",
        ".pulsar/build/generated.json",
    ] {
        assert_eq!(
            read(&f.project.join(file)),
            read(&elsewhere.join(file)),
            "{file} differs between the two places"
        );
    }
}

#[test]
fn engine_changes_reach_the_mirror_and_only_them() {
    let f = fixture();
    sync(&f.project, &f.engine);
    let sdk = f.project.join(".pulsar/sdk");

    // A source edit and a removed file.
    std::thread::sleep(std::time::Duration::from_millis(20));
    write(&f.engine.root.join("crates/pulsar_game/src/lib.rs"), "pub fn edited() {}");
    std::fs::remove_file(f.engine.root.join("crates/pulsar_game/shaders/sky.wgsl")).unwrap();
    let report = sync(&f.project, &f.engine);
    assert_eq!((report.sdk.written, report.sdk.deleted), (1, 1), "{report:?}");
    assert_eq!(read(&sdk.join("crates/pulsar_game/src/lib.rs")), "pub fn edited() {}");
    assert!(!sdk.join("crates/pulsar_game/shaders").exists(), "the emptied folder goes too");

    // A new dependency: the index is rebuilt and the package mirrored.
    write(
        &f.engine.root.join("crates/glow/Cargo.toml"),
        "[package]\nname = \"glow\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&f.engine.root.join("crates/glow/src/lib.rs"), "");
    let manifest = f.engine.root.join("plugins/lights/component/Cargo.toml");
    let text = read(&manifest).replace(
        "[lints]",
        "[dependencies]\nglow = { path = \"../../../crates/glow\" }\n\n[lints]",
    );
    write(&manifest, &text);
    f.engine.lock();
    let report = sync(&f.project, &f.engine);
    assert!(report.lock_seeded, "the engine changed");
    assert!(sdk.join("crates/glow/Cargo.toml").is_file());
}

#[test]
fn a_deleted_mirror_is_rebuilt() {
    let f = fixture();
    sync(&f.project, &f.engine);
    let before = tree(&f.project.join(".pulsar/sdk"));
    std::fs::remove_dir_all(f.project.join(".pulsar/sdk")).unwrap();
    let report = sync(&f.project, &f.engine);
    assert!(report.sdk.written > 0);
    assert_eq!(tree(&f.project.join(".pulsar/sdk")), before);
}

#[test]
fn a_hand_edited_generated_file_is_restored_and_reported() {
    let f = fixture();
    sync(&f.project, &f.engine);
    let setup = f.project.join(".pulsar/build/game/src/setup.rs");
    let generated = read(&setup);
    write(&setup, "// mine");
    let report = sync(&f.project, &f.engine);
    assert_eq!(report.files.drifted, [".pulsar/build/game/src/setup.rs"]);
    assert_eq!(read(&setup), generated);
}

#[test]
fn plugins_follow_pulsar_toml() {
    let f = fixture();
    sync(&f.project, &f.engine);
    write(
        &f.project.join("Pulsar.toml"),
        "[project]\nname = \"Star Fall\"\n\n[plugins]\n\"pulsar.lights\" = false\n",
    );
    let report = sync(&f.project, &f.engine);
    assert!(report.changed());
    let game = read(&f.project.join(".pulsar/build/game/Cargo.toml"));
    assert!(!game.contains("lights_plugin"));
    assert!(!read(&f.project.join(".pulsar/build/game/src/plugins.rs")).contains("lights_plugin"));
}

#[test]
fn a_cargo_toml_the_generator_did_not_write_is_never_replaced() {
    let f = fixture();
    let own = "[package]\nname = \"mine\"\nversion = \"0.1.0\"\n";
    write(&f.project.join("Cargo.toml"), own);
    let sdk = EngineSdk::at(&f.engine.root).unwrap();
    let error = sync_with(&f.project, &sdk, SyncOptions::default()).unwrap_err();
    assert!(matches!(error, SyncError::Unowned { ref path } if path == "Cargo.toml"), "{error}");
    assert_eq!(read(&f.project.join("Cargo.toml")), own);
    assert!(!f.project.join(".pulsar").exists(), "nothing is written");
}

#[test]
fn a_scripts_crate_with_a_machine_path_is_reported() {
    let f = fixture();
    write(
        &f.project.join("scripts/old/Cargo.toml"),
        "[package]\nname = \"old_scripts\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\npulsar_game = { path = \"C:/Users/someone/Pulsar-Native/crates/core/pulsar_game\" }\n",
    );
    let report = sync(&f.project, &f.engine);
    assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
    assert!(report.problems[0].contains("scripts/old"));
}

#[test]
fn a_scripts_crate_named_like_an_engine_crate_is_refused() {
    let f = fixture();
    write(
        &f.project.join("scripts/bad/Cargo.toml"),
        "[package]\nname = \"pulsar_game\"\nversion = \"0.1.0\"\n",
    );
    let sdk = EngineSdk::at(&f.engine.root).unwrap();
    let error = sync_with(&f.project, &sdk, SyncOptions::default()).unwrap_err();
    assert!(matches!(error, SyncError::ScriptCrate { .. }), "{error}");
}

#[test]
fn a_class_compiled_to_rust_is_generated_and_removed_with_its_setting() {
    let f = fixture();
    let class = f.project.join("Door.class");
    let module = pulsar_script_vm::Module::new("Door").to_json().unwrap();
    write(&class.join("events/.build/module.json"), &module);
    write(&class.join("graph_save.json"), "{\"compile_target\": \"rust\"}");

    let report = sync(&f.project, &f.engine);
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    let game = f.project.join(".pulsar/build/game");
    assert!(read(&game.join("src/classes/door.rs")).contains("NativeActor"));
    assert!(read(&game.join("src/classes/mod.rs")).contains("pub mod door;"));
    assert!(read(&game.join("Cargo.toml")).contains("engine_class_derive = { workspace = true }"));
    assert!(read(&game.join("src/lib.rs")).contains("mod classes;"));

    write(&class.join("graph_save.json"), "{\"compile_target\": \"vm\"}");
    let report = sync(&f.project, &f.engine);
    assert!(report.files.deleted.iter().any(|d| d.ends_with("classes/door.rs")));
    assert!(!game.join("src/classes").exists());
    assert!(!read(&game.join("Cargo.toml")).contains("engine_class_derive"));
}

#[test]
fn a_class_set_to_rust_but_never_compiled_is_a_problem_not_a_failure() {
    let f = fixture();
    write(
        &f.project.join("Lamp.class/graph_save.json"),
        "{\"compile_target\": \"rust\"}",
    );
    let report = sync(&f.project, &f.engine);
    assert_eq!(report.problems.len(), 1);
    assert!(report.problems[0].contains("Lamp"), "{:?}", report.problems);
}

#[test]
fn an_exported_sdk_generates_the_same_project_as_its_checkout() {
    let f = fixture();
    let exported_dir = f.project.parent().unwrap().join("install/sdk");
    let checkout = EngineSdk::at(&f.engine.root).unwrap();
    checkout.export(&exported_dir).unwrap();
    let exported = EngineSdk::at(&exported_dir).unwrap();
    assert_eq!(exported.kind(), SdkKind::Exported);

    let other = f.project.parent().unwrap().join("from-install");
    project(&other);
    sync_with(&f.project, &checkout, SyncOptions::default()).unwrap();
    sync_with(&other, &exported, SyncOptions::default()).unwrap();
    assert_eq!(read(&f.project.join("Cargo.toml")), read(&other.join("Cargo.toml")));
    assert_eq!(read(&f.project.join("Cargo.lock")), read(&other.join("Cargo.lock")));
    assert_eq!(
        tree(&f.project.join(".pulsar/sdk"))
            .into_iter()
            .filter(|(rel, _)| rel != "sdk.json")
            .collect::<Vec<_>>(),
        tree(&other.join(".pulsar/sdk"))
            .into_iter()
            .filter(|(rel, _)| rel != "sdk.json")
            .collect::<Vec<_>>(),
        "the mirrors match"
    );
}

#[test]
fn a_folder_without_pulsar_toml_is_not_a_project() {
    let f = fixture();
    std::fs::remove_file(f.project.join("Pulsar.toml")).unwrap();
    let sdk = EngineSdk::at(&f.engine.root).unwrap();
    let error = sync_with(&f.project, &sdk, SyncOptions::default()).unwrap_err();
    assert!(matches!(error, SyncError::NotAProject(_)), "{error}");
}
