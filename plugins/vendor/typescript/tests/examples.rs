//! Every embedded example class compiles with the plugin's own compiler.

use plugin_editor_api::NativeRegistry;
use plugin_typescript::{compile_project_classes, EXAMPLES};

use plugin_typescript as _;
use pulsar_script_math as _;

#[test]
fn every_example_class_compiles() {
    let dir = tempfile::tempdir().unwrap();
    let mut classes = 0;
    for file in EXAMPLES.iter().filter(|f| f.path.ends_with("class.ts")) {
        // examples/<Class>/class.ts becomes src/classes/<Class>/class.ts.
        let class_path = file.path.strip_prefix("examples/").unwrap();
        let path = dir.path().join("src/classes").join(class_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, file.bytes).unwrap();
        classes += 1;
    }
    assert!(classes >= 3);

    // Hero is the second version of a class: `@renamedFrom` needs the schema
    // of the first, so compile that first.
    let hero = dir.path().join("src/classes/Hero/class.ts");
    let version_two = std::fs::read(&hero).unwrap();
    std::fs::write(
        &hero,
        "export default class Hero extends ScriptClass {\n    hp: int = 10;\n    position: Vec3 = Vec3.new_(1, 2, 3);\n    name: string = \"hero\";\n}\n",
    )
    .unwrap();
    let natives = NativeRegistry::with_engine_natives();
    let problems = compile_project_classes(dir.path(), &natives);
    assert!(problems.is_empty(), "{problems:#?}");

    std::fs::write(&hero, version_two).unwrap();
    let problems = compile_project_classes(dir.path(), &natives);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn the_readme_is_embedded() {
    assert!(EXAMPLES
        .iter()
        .any(|f| f.path == "examples/README.md" && !f.bytes.is_empty()));
}
