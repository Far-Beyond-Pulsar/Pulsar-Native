//! Headless TypeScript compile: `pulsar build-scripts` compiles every
//! `class.ts` through `ScriptLanguage::compile_project`, with no editor.
#![cfg(feature = "typescript")]

use pulsar_content::BuildProfile;

const DOOR: &str = r#"
export default class Door extends ScriptClass {
    opened: int = 0;
    open(): void { this.opened += 1; }
}
"#;

#[test]
fn typescript_classes_compile_headlessly_and_link_against_the_engine() {
    let project = tempfile::tempdir().unwrap();
    let classes = project.path().join("src/classes");
    std::fs::create_dir_all(classes.join("Door")).unwrap();
    std::fs::write(classes.join("Door/class.ts"), DOOR).unwrap();

    let output =
        pulsar_package::build_scripts(project.path(), BuildProfile::Dev).expect("Door compiles");
    assert!(
        output.languages.iter().any(|l| l == "typescript"),
        "{:?}",
        output.languages
    );
    let module =
        std::fs::read(classes.join("Door/events/.build/module.json")).expect("module written");
    assert_eq!(
        pulsar_script_vm::Module::decode(&module).unwrap().name,
        "Door"
    );
    assert!(
        classes.join("Door/class.schema.json").is_file(),
        "field identity is written for the class to keep"
    );

    // A class with a type error fails the build and loses its stale module.
    std::fs::write(
        classes.join("Door/class.ts"),
        "export default class Door { f(): int { return \"x\"; } }",
    )
    .unwrap();
    let Err(pulsar_package::PackageError::Scripts(problems)) =
        pulsar_package::build_scripts(project.path(), BuildProfile::Dev)
    else {
        panic!("a broken class must fail the build");
    };
    assert!(
        problems.iter().any(|p| p.error
            && p.class.as_deref() == Some("Door")
            && p.message.contains("expected `int`")),
        "{problems:?}"
    );
    assert!(
        !classes.join("Door/events/.build/module.json").exists(),
        "stale module removed"
    );
}
