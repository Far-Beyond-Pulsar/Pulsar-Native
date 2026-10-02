//! The Blueprint editor is part of a build exactly when its crate is linked
//! (`blueprint` feature), through the plugin API alone: the editor shell
//! (`ui_core`) does not depend on it (#882).

// Linking the crate is what includes it, as `main.rs` does.
#[cfg(feature = "blueprint")]
use blueprint_editor_plugin as _;

use plugin_manager::BuiltinEditorRegistry;

const BLUEPRINT: &str = "com.pulsar.blueprint-editor";

#[test]
fn the_blueprint_editor_is_present_exactly_when_the_feature_is_on() {
    let mut registry = BuiltinEditorRegistry::new();
    registry.register_linked();
    assert_eq!(registry.provider_by_id(BLUEPRINT).is_some(), cfg!(feature = "blueprint"));
}

#[test]
fn its_scripting_language_comes_with_it_and_only_with_it() {
    let mut registry = BuiltinEditorRegistry::new();
    registry.register_linked();
    let languages = registry.get_all_script_languages();
    assert_eq!(!languages.is_empty(), cfg!(feature = "blueprint"), "{} languages", languages.len());
}

#[cfg(feature = "blueprint")]
#[test]
fn it_offers_its_file_type_and_editor_through_the_ordinary_registry_path() {
    use plugin_manager::{EditorRegistry, FileTypeRegistry};

    let mut registry = BuiltinEditorRegistry::new();
    registry.register_linked();
    let (mut file_types, mut editors) = (FileTypeRegistry::new(), EditorRegistry::new());
    registry.register_all(&mut file_types, &mut editors);
    let provider = registry.provider_by_id(BLUEPRINT).unwrap();
    assert!(provider.file_types().iter().any(|t| t.extension == "class"));
    assert!(provider.editors().iter().any(|e| e.id.as_str() == "blueprint-editor"));
}
