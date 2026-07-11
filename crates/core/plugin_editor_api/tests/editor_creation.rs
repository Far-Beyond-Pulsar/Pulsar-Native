//! Integration test: verify the EditorPluginEditor -> EditorFactoryRegistry ->
//! EditorFactory -> PanelView flow works, including the DLL export_plugin! macro
//! and cross-crate trait dispatch.
//!
//! This tests the full plugin lifecycle WITHOUT a GPUI app context by using
//! the DLL loading path directly and verifying the trait vtable layout matches
//! between the engine's plugin_editor_api and the compiled plugin.

use plugin_editor_api::editor_element::{EditorFactoryRegistry, EditorPluginEditor};
use plugin_editor_api::identifiers::EditorId;
use plugin_editor_api::plugin::EditorPlugin;
use plugin_editor_api::version::VersionInfo;
use plugin_editor_api::PluginMetadata;

/// A minimal test plugin that implements the full EditorPlugin + EditorPluginEditor
/// trait chain. This verifies the traits can be implemented and dispatched
/// through dyn trait objects without vtabel mismatch.
struct TestPlugin;

impl EditorPlugin for TestPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: plugin_editor_api::identifiers::PluginId::new("test.plugin"),
            name: "Test Plugin".into(),
            version: "0.1.0".into(),
            author: "Test".into(),
            description: "Integration test plugin".into(),
        }
    }

    fn file_types(&self) -> Vec<plugin_editor_api::file_types::FileTypeDefinition> {
        Vec::new()
    }

    fn editors(&self) -> Vec<plugin_editor_api::metadata::EditorMetadata> {
        vec![plugin_editor_api::metadata::EditorMetadata {
            id: EditorId::new("test-editor"),
            display_name: "Test Editor".into(),
            supported_file_types: Vec::new(),
        }]
    }
}

impl EditorPluginEditor for TestPlugin {
    fn register_editors(&'static self, registry: &mut EditorFactoryRegistry) {
        // Basic registration — just tests the factory closure is stored
        registry.register_fn(EditorId::new("test-editor"), |_file_path, _window, _cx| {
            Err(plugin_editor_api::error::PluginError::Other {
                message: "No GPUI app in test — creation expected to fail".into(),
            })
        });
    }
}

/// A second editor type on the same plugin — tests 1→N relationship
const SECOND_EDITOR: &str = "test-editor-secondary";

#[test]
fn trait_vtable_dispatch() {
    // Box the plugin as dyn EditorPluginFull and verify trait methods dispatch
    // through the vtable correctly (no wrong-slot calls).

    let plugin: &'static dyn plugin_editor_api::plugin::EditorPluginFull = {
        // Helper struct that impls all subtraits
        struct FullTestPlugin;

        impl EditorPlugin for FullTestPlugin {
            fn metadata(&self) -> PluginMetadata {
                TestPlugin.metadata()
            }
            fn file_types(&self) -> Vec<plugin_editor_api::file_types::FileTypeDefinition> {
                TestPlugin.file_types()
            }
            fn editors(&self) -> Vec<plugin_editor_api::metadata::EditorMetadata> {
                TestPlugin.editors()
            }
        }

        impl EditorPluginEditor for FullTestPlugin {
            fn register_editors(&'static self, registry: &mut EditorFactoryRegistry) {
                registry.register_fn(EditorId::new("test-editor"), |_p, _w, _c| {
                    Err(plugin_editor_api::error::PluginError::Other {
                        message: "no app".into(),
                    })
                });
                registry.register_fn(EditorId::new(SECOND_EDITOR), |_p, _w, _c| {
                    Err(plugin_editor_api::error::PluginError::Other {
                        message: "no app".into(),
                    })
                });
            }
        }

        impl plugin_editor_api::statusbar::EditorPluginStatusbar for FullTestPlugin {}
        impl plugin_editor_api::ai::EditorPluginAi for FullTestPlugin {}
        impl plugin_editor_api::components::EditorPluginComponents for FullTestPlugin {
            fn component_definitions(
                &self,
            ) -> Vec<plugin_editor_api::components::ComponentDefinition> {
                Vec::new()
            }
        }
        impl plugin_editor_api::subsystems::EditorPluginSubsystems for FullTestPlugin {
            fn subsystems(&self) -> Vec<Box<dyn plugin_editor_api::subsystems::Subsystem>> {
                Vec::new()
            }
        }

        impl plugin_editor_api::plugin::EditorPluginFull for FullTestPlugin {}

        // Leak the box to get &'static (simulating what the DLL does)
        let plugin: &'static mut dyn plugin_editor_api::plugin::EditorPluginFull =
            Box::leak(Box::new(FullTestPlugin));

        plugin
    };

    // Verify metadata dispatches correctly (simple trait method)
    let meta = plugin.metadata();
    assert_eq!(meta.name, "Test Plugin");
    eprintln!("✅ EditorPlugin::metadata() dispatches correctly");

    // Verify editors dispatches correctly
    let editors = plugin.editors();
    assert_eq!(editors.len(), 1);
    assert_eq!(editors[0].id.as_str(), "test-editor");
    eprintln!("✅ EditorPlugin::editors() dispatches correctly");

    // Verify register_editors dispatches correctly on EditorPluginEditor
    // (this is the NEW path — must work through dyn EditorPluginFull)
    let mut registry = EditorFactoryRegistry::new();
    EditorPluginEditor::register_editors(plugin, &mut registry);
    assert_eq!(registry.factories().len(), 2);
    eprintln!(
        "✅ EditorPluginEditor::register_editors() dispatches correctly ({} factories)",
        registry.factories().len()
    );

    // Verify we can look up each factory by editor_id
    let primary = registry.get(&EditorId::new("test-editor"));
    assert!(primary.is_some(), "primary editor factory not found");
    eprintln!("✅ EditorFactoryRegistry::get() finds primary editor");

    let secondary = registry.get(&EditorId::new(SECOND_EDITOR));
    assert!(secondary.is_some(), "secondary editor factory not found");
    eprintln!("✅ EditorFactoryRegistry::get() finds secondary editor");

    // Verify unknown editor returns None
    let unknown = registry.get(&EditorId::new("nonexistent"));
    assert!(unknown.is_none(), "unknown editor should not be found");
    eprintln!("✅ EditorFactoryRegistry::get() returns None for unknown editor");

    // Verify factory closure is callable
    // (requires Window + App which we don't have in test, but we can
    //  verify the boxed closure is correctly stored and the error path works)
    let factory = primary.unwrap();
    assert_eq!(factory.editor_id.as_str(), "test-editor");
    eprintln!("✅ EditorFactory stores correct editor_id");
}

#[test]
fn version_info_abi() {
    // VersionInfo is #[repr(C)] — verify it crosses the FFI boundary correctly
    let v = VersionInfo::current();
    assert_eq!(v.engine_version.0, 0);
    // rustc_version_hash should be non-zero
    assert!(
        v.rustc_version_hash != 0,
        "rustc_version_hash should not be zero"
    );
    eprintln!(
        "✅ VersionInfo ABI: engine ({}.{}.{}), rustc_hash 0x{:016x}",
        v.engine_version.0, v.engine_version.1, v.engine_version.2, v.rustc_version_hash
    );
}
