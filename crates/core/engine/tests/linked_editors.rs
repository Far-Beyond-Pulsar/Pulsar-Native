//! Scripting-language plugins are part of a build exactly when their crates
//! are linked (features `blueprint`, `typescript`), through the plugin API
//! alone: the editor shell (`ui_core`) depends on none of them (#882), and
//! editor and headless tools discover the same languages.

// Linking the crates is what includes them, as `main.rs` does.
#[cfg(feature = "blueprint")]
use blueprint_editor_plugin as _;
#[cfg(feature = "typescript")]
use plugin_typescript as _;

use plugin_editor_api::linked_script_languages;
use plugin_manager::BuiltinEditorRegistry;

const BLUEPRINT: &str = "com.pulsar.blueprint-editor";

fn registry() -> BuiltinEditorRegistry {
    let mut registry = BuiltinEditorRegistry::new();
    registry.register_linked();
    registry
}

#[test]
fn the_blueprint_editor_is_present_exactly_when_the_feature_is_on() {
    assert_eq!(registry().provider_by_id(BLUEPRINT).is_some(), cfg!(feature = "blueprint"));
}

#[test]
fn each_language_comes_with_its_crate_and_only_with_it() {
    let ids: Vec<String> = registry().get_all_script_languages().iter().map(|l| l.id().to_owned()).collect();
    assert_eq!(ids.iter().any(|i| i == "blueprint"), cfg!(feature = "blueprint"), "{ids:?}");
    assert_eq!(ids.iter().any(|i| i == "typescript"), cfg!(feature = "typescript"), "{ids:?}");
}

#[test]
fn the_editor_and_headless_tools_find_the_same_languages() {
    let mut editor: Vec<String> = registry().get_all_script_languages().iter().map(|l| l.id().to_owned()).collect();
    let mut headless: Vec<String> = linked_script_languages().iter().map(|l| l.id().to_owned()).collect();
    editor.sort();
    headless.sort();
    assert_eq!(editor, headless);
}

#[cfg(feature = "blueprint")]
#[test]
fn the_blueprint_editor_offers_its_file_type_and_editor_through_the_ordinary_registry_path() {
    use plugin_manager::{EditorRegistry, FileTypeRegistry};

    let registry = registry();
    let (mut file_types, mut editors) = (FileTypeRegistry::new(), EditorRegistry::new());
    registry.register_all(&mut file_types, &mut editors);
    let provider = registry.provider_by_id(BLUEPRINT).unwrap();
    assert!(provider.file_types().iter().any(|t| t.extension == "class"));
    assert!(provider.editors().iter().any(|e| e.id.as_str() == "blueprint-editor"));
}
