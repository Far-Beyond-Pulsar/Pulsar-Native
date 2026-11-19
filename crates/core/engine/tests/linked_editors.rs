//! Scripting-language plugins are part of a build exactly when their crates
//! are linked (the installed set's Blueprints and TypeScript plugins),
//! through the plugin API alone: the editor shell (`ui_core`) depends on none
//! of them (#882), and the editor and headless tools discover the same
//! languages, because a language is one `pulsar_language::Language`
//! registration that both read through the language registry.

// Linking the installed set is what includes them, as `main.rs` does.
use installed_plugins as _;

use plugin_manager::BuiltinEditorRegistry;
use pulsar_language::LanguageRegistry;

const BLUEPRINT: &str = "com.pulsar.blueprint-editor";

fn registry() -> BuiltinEditorRegistry {
    let mut registry = BuiltinEditorRegistry::new();
    registry.register_linked();
    registry
}

#[test]
fn the_blueprint_editor_is_installed() {
    assert!(registry().provider_by_id(BLUEPRINT).is_some());
}

#[test]
fn each_language_comes_with_its_crate_and_only_with_it() {
    let languages = LanguageRegistry::current();
    let compilers: Vec<&str> = languages
        .all()
        .iter()
        .filter(|language| language.compiler.is_some())
        .map(|language| language.id)
        .collect();
    assert!(compilers.contains(&"blueprint"), "{compilers:?}");
    assert!(compilers.contains(&"typescript"), "{compilers:?}");
}

#[test]
fn the_editor_and_headless_tools_find_the_same_compilers() {
    // One registration: the packager's list and the editor's pre-Play
    // validation read `LanguageRegistry::compilers`, so there is nothing to
    // keep equal; this pins that each compiled language has its compiler.
    let languages = LanguageRegistry::current();
    let mut ids: Vec<&str> = languages
        .compilers()
        .iter()
        .map(|(language, _)| language.id)
        .collect();
    ids.sort_unstable();
    let mut expected: Vec<&str> = languages
        .all()
        .iter()
        .filter(|language| language.compiler.is_some())
        .map(|language| language.id)
        .collect();
    expected.sort_unstable();
    assert_eq!(ids, expected);
}

#[test]
fn the_blueprint_editor_offers_its_file_type_and_editor_through_the_ordinary_registry_path() {
    use plugin_manager::{EditorRegistry, FileTypeRegistry};

    let registry = registry();
    let (mut file_types, mut editors) = (FileTypeRegistry::new(), EditorRegistry::new());
    registry.register_all(&mut file_types, &mut editors);
    let provider = registry.provider_by_id(BLUEPRINT).unwrap();
    assert!(provider.file_types().iter().any(|t| t.extension == "class"));
    assert!(provider
        .editors()
        .iter()
        .any(|e| e.id.as_str() == "blueprint-editor"));
}
