//! Headless Blueprint compile (#879): `pulsar build-scripts` compiles every
//! class's saved graph through `ScriptLanguage::compile_project`, writes its
//! module, and fails on classes that do not compile.
#![cfg(feature = "scripting")]

use pulsar_content::BuildProfile;

const EMPTY_GRAPH: &str = r#"{
  "format_version": 1,
  "main_graph": {
    "nodes": {},
    "connections": [],
    "metadata": { "name": "EventGraph", "description": "", "version": "1", "created_at": "", "modified_at": "" }
  },
  "local_macros": [],
  "variables": []
}"#;

#[test]
fn blueprint_classes_compile_headlessly() {
    let project = tempfile::tempdir().unwrap();
    let classes = project.path().join("src/classes");
    std::fs::create_dir_all(classes.join("Door")).unwrap();
    std::fs::write(classes.join("Door/graph_save.json"), EMPTY_GRAPH).unwrap();

    let output =
        pulsar_package::build_scripts(project.path(), BuildProfile::Dev).expect("Door compiles");
    assert!(
        output.languages.iter().any(|l| l == "blueprint"),
        "{:?}",
        output.languages
    );
    let door_module = pulsar_class::modules::class_module_path(&classes.join("Door")).unwrap();
    let module = std::fs::read(&door_module).expect("module written");
    assert_eq!(
        pulsar_script_vm::Module::decode(&module).unwrap().name,
        "Door"
    );

    // A class that does not parse fails the build, and loses its stale module.
    std::fs::create_dir_all(classes.join("Broken")).unwrap();
    std::fs::write(classes.join("Broken/graph_save.json"), "{ not a graph").unwrap();
    let broken_module = pulsar_class::modules::class_module_path(&classes.join("Broken")).unwrap();
    std::fs::create_dir_all(broken_module.parent().unwrap()).unwrap();
    std::fs::write(&broken_module, "{}").unwrap();
    let Err(pulsar_package::PackageError::Scripts(problems)) =
        pulsar_package::build_scripts(project.path(), BuildProfile::Dev)
    else {
        panic!("a broken class must fail the build");
    };
    assert!(
        problems
            .iter()
            .any(|p| p.error && p.class.as_deref() == Some("Broken")),
        "{problems:?}"
    );
    assert!(!broken_module.exists(), "stale module removed");
}
