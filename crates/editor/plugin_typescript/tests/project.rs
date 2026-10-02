//! Compiling a project's TypeScript classes headlessly.

use std::path::Path;

use plugin_editor_api::{linked_script_languages, NativeRegistry, ScriptLanguage};
use plugin_typescript::{compile_project_classes, validate_project_classes, TypeScriptLanguage, CLASS_FILE, SCHEMA_FILE};
use pulsar_script_vm::Module;

use plugin_typescript as _;
use pulsar_script_math as _;

const DOOR: &str = r#"
export default class Door extends ScriptClass {
    opened: int = 0;
    openings(): int { return this.opened; }
    open(): void { this.opened += 1; }
}
"#;

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    dir
}

fn natives() -> NativeRegistry {
    NativeRegistry::with_engine_natives()
}

fn module(root: &Path, class: &str) -> Module {
    let json = std::fs::read_to_string(root.join(format!("src/classes/{class}/events/.build/module.json"))).expect("module.json");
    Module::from_json(&json).expect("a module")
}

#[test]
fn a_class_compiles_to_the_modules_the_engine_loads() {
    let dir = project(&[("src/classes/Door/class.ts", DOOR)]);
    let problems = compile_project_classes(dir.path(), &natives());
    assert!(problems.is_empty(), "{problems:#?}");

    let door = module(dir.path(), "Door");
    assert_eq!(door.name, "Door");
    assert!(door.function("open").is_some());
    let build = dir.path().join("src/classes/Door/events/.build");
    assert_eq!(std::fs::read_to_string(build.join("language")).unwrap(), "typescript");
    assert!(dir.path().join("src/classes/Door").join(SCHEMA_FILE).is_file(), "the schema is written next to the source");
    assert!(std::fs::read_to_string(dir.path().join("Pulsar/types/pulsar.d.ts")).unwrap().contains("declare function wait"));
}

#[test]
fn field_identity_is_stable_across_compiles() {
    let dir = project(&[("src/classes/Door/class.ts", DOOR)]);
    assert!(compile_project_classes(dir.path(), &natives()).is_empty());
    let first = module(dir.path(), "Door").variables[0].id.clone();
    assert!(first.is_some());
    let schema = std::fs::read_to_string(dir.path().join("src/classes/Door").join(SCHEMA_FILE)).unwrap();

    // Recompiling unchanged source changes nothing, including the schema file.
    assert!(compile_project_classes(dir.path(), &natives()).is_empty());
    assert_eq!(module(dir.path(), "Door").variables[0].id, first);
    assert_eq!(std::fs::read_to_string(dir.path().join("src/classes/Door").join(SCHEMA_FILE)).unwrap(), schema);

    // Editing the source (a new field) keeps the old field's id and raises the version.
    std::fs::write(
        dir.path().join("src/classes/Door").join(CLASS_FILE),
        DOOR.replace("opened: int = 0;", "opened: int = 0;\n    locked: boolean = false;"),
    )
    .unwrap();
    assert!(compile_project_classes(dir.path(), &natives()).is_empty());
    let door = module(dir.path(), "Door");
    assert_eq!(door.variables[0].id, first);
    assert_eq!(door.class_version, 2);
}

#[test]
fn errors_carry_the_class_file_and_position_and_leave_no_stale_module() {
    let dir = project(&[("src/classes/Door/class.ts", DOOR)]);
    assert!(compile_project_classes(dir.path(), &natives()).is_empty());
    assert!(dir.path().join("src/classes/Door/events/.build/module.json").is_file());

    std::fs::write(
        dir.path().join("src/classes/Door").join(CLASS_FILE),
        "export default class Door {\n    open(): int {\n        return 1.5;\n    }\n}\n",
    )
    .unwrap();
    let problems = compile_project_classes(dir.path(), &natives());
    assert_eq!(problems.len(), 1, "{problems:#?}");
    let problem = &problems[0];
    assert!(problem.is_error());
    assert_eq!(problem.class.as_deref(), Some("Door"));
    assert!(problem.file.as_ref().unwrap().ends_with("class.ts"));
    assert_eq!(problem.location.as_deref(), Some("3:16"));
    assert!(problem.message.contains("expected `int`, found `number`"));
    assert!(!dir.path().join("src/classes/Door/events/.build/module.json").exists(), "never leave a module from an older source");
}

#[test]
fn a_class_has_one_language() {
    // Both sources present.
    let dir = project(&[("src/classes/Door/class.ts", DOOR), ("src/classes/Door/graph_save.json", "{}")]);
    let problems = compile_project_classes(dir.path(), &natives());
    assert!(problems.iter().any(|p| p.message.contains("a class has one language")), "{problems:#?}");
    assert!(!dir.path().join("src/classes/Door/events/.build/module.json").exists());

    // A module another language wrote is not overwritten.
    let dir = project(&[
        ("src/classes/Door/class.ts", DOOR),
        ("src/classes/Door/events/.build/language", "blueprint"),
        ("src/classes/Door/events/.build/module.json", "{\"from\":\"blueprint\"}"),
    ]);
    let problems = compile_project_classes(dir.path(), &natives());
    assert!(problems.iter().any(|p| p.message.contains("`blueprint` language")), "{problems:#?}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("src/classes/Door/events/.build/module.json")).unwrap(),
        "{\"from\":\"blueprint\"}",
        "the other language's artifact survives the refusal"
    );
}

#[test]
fn validation_checks_without_writing_and_blocks_play_on_errors() {
    let dir = project(&[("src/classes/Door/class.ts", DOOR)]);
    assert!(validate_project_classes(dir.path(), &natives()).is_ok());
    assert!(!dir.path().join("src/classes/Door/events").exists(), "validation writes nothing");

    std::fs::write(dir.path().join("src/classes/Door").join(CLASS_FILE), "export default class Door { f(): int { return \"x\"; } }").unwrap();
    let message = validate_project_classes(dir.path(), &natives()).unwrap_err();
    assert!(message.contains("Door") && message.contains("expected `int`, found `string`"), "{message}");
    assert_eq!(TypeScriptLanguage.validate_project(dir.path()).is_err(), true);
}

#[test]
fn projects_without_typescript_compile_nothing_and_write_nothing() {
    let dir = project(&[("src/classes/Lamp/graph_save.json", "{}")]);
    assert!(compile_project_classes(dir.path(), &natives()).is_empty());
    assert!(!dir.path().join("Pulsar").exists(), "no declarations without TypeScript classes");
}

#[test]
fn the_language_is_discoverable_without_naming_the_crate() {
    let languages = linked_script_languages();
    let typescript = languages.iter().find(|l| l.id() == "typescript").expect("registered at link time");
    assert_eq!(typescript.display_name(), "TypeScript");
}
