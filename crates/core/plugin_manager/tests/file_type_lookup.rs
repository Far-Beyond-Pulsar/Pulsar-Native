//! The file-type lookup is published by the registry and read without a lock.

use std::sync::mpsc;
use std::time::Duration;

use plugin_editor_api::{FileStructure, FileTypeDefinition, FileTypeId, PluginId};
use plugin_manager::FileTypeRegistry;

fn definition(id: &str, extension: &str, structure: FileStructure) -> FileTypeDefinition {
    FileTypeDefinition {
        id: FileTypeId::new(id),
        extension: extension.to_string(),
        display_name: id.to_string(),
        icon: ui::IconName::Page,
        color: gpui::rgb(0x808080).into(),
        structure,
        default_content: serde_json::json!(""),
        creation_directory: None,
        categories: Vec::new(),
    }
}

fn plugin() -> PluginId {
    PluginId::new("test.plugin")
}

#[test]
fn a_handle_taken_before_registration_sees_later_changes() {
    let mut registry = FileTypeRegistry::new();
    let lookup = registry.lookup_handle();
    let path = std::path::Path::new("scene/player.skeleton");
    assert_eq!(lookup.load().file_type_for_path(path), None);

    registry.register(definition("skeleton", "skeleton", FileStructure::Standalone), plugin());
    assert_eq!(
        lookup.load().file_type_for_path(path),
        Some(FileTypeId::new("skeleton"))
    );
    assert_eq!(registry.get_file_type_for_path(path), Some(FileTypeId::new("skeleton")));

    registry.unregister_by_plugin(&plugin());
    assert_eq!(lookup.load().file_type_for_path(path), None);
}

#[test]
fn compound_extensions_resolve() {
    let mut registry = FileTypeRegistry::new();
    registry.register(definition("struct", "struct.json", FileStructure::Standalone), plugin());
    assert_eq!(
        registry.get_file_type_for_path(std::path::Path::new("a/b/Player.struct.json")),
        Some(FileTypeId::new("struct"))
    );
}

#[test]
fn folder_based_types_resolve_by_extension_and_by_marker() {
    let mut registry = FileTypeRegistry::new();
    registry.register(
        definition(
            "class",
            "class",
            FileStructure::FolderBased {
                marker_file: "graph_save.json".to_string(),
                template_structure: Vec::new(),
            },
        ),
        plugin(),
    );
    let dir = tempfile::tempdir().unwrap();

    let by_extension = dir.path().join("Enemy.class");
    std::fs::create_dir(&by_extension).unwrap();
    assert_eq!(registry.get_file_type_for_path(&by_extension), Some(FileTypeId::new("class")));

    let by_marker = dir.path().join("Plain");
    std::fs::create_dir(&by_marker).unwrap();
    assert_eq!(registry.get_file_type_for_path(&by_marker), None);
    std::fs::write(by_marker.join("graph_save.json"), "{}").unwrap();
    assert_eq!(registry.get_file_type_for_path(&by_marker), Some(FileTypeId::new("class")));
}

/// The reason the lookup exists: reading it must work while the same thread
/// holds the lock that normally guards the registry (a write lock, as `ui_core`
/// does while it builds an editor).
#[test]
fn reading_the_lookup_never_waits_on_the_registry_lock() {
    let mut registry = FileTypeRegistry::new();
    registry.register(definition("skeleton", "skeleton", FileStructure::Standalone), plugin());
    let lookup = registry.lookup_handle();
    let guarded = parking_lot::RwLock::new(registry);

    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let _write = guarded.write();
        let found = lookup.load().file_type_for_path(std::path::Path::new("x.skeleton"));
        done.send(found).unwrap();
    });
    let found = finished
        .recv_timeout(Duration::from_secs(30))
        .expect("the lookup waited on the registry lock");
    assert_eq!(found, Some(FileTypeId::new("skeleton")));
}
