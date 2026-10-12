//! The plugin API's context hands every editor the engine's language server.
//!
//! Editors used to find the analyzer through a process global, which a plugin
//! library does not share with the host, and one creation path attached
//! nothing at all -- so an editor could open, look fine, and never get hover,
//! completion or diagnostics. These tests pin the contract from the host side:
//! whatever the manager is given is what the editor factory receives.

use gpui::{div, AppContext as _, Context, IntoElement, Render, TestAppContext, Window};
use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext, PluginManager};
use pulsar_lsp::RustAnalyzerManager;
use std::path::PathBuf;
use std::sync::Arc;
use ui::dock::PanelView;

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A built-in editor that reports, in its error message, whether the context
/// it was created with carried a language server.
struct Probe;

impl BuiltinEditorProvider for Probe {
    fn provider_id(&self) -> &str {
        "test.services-probe"
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![FileTypeDefinition {
            id: FileTypeId::new("services_probe"),
            extension: "svcprobe".to_string(),
            display_name: "Probe".to_string(),
            icon: ui::IconName::Page,
            color: gpui::rgb(0x808080).into(),
            structure: FileStructure::Standalone,
            default_content: serde_json::json!(""),
            creation_directory: None,
            categories: vec!["Test".to_string()],
        }]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![EditorMetadata {
            id: EditorId::new("services-probe"),
            display_name: "Probe".into(),
            supported_file_types: vec![FileTypeId::new("services_probe")],
        }]
    }

    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "services-probe"
    }

    fn create_editor(
        &self,
        _file_path: PathBuf,
        editor_context: &EditorContext,
        _window: &mut Window,
        _cx: &mut gpui::App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        Err(PluginError::Other {
            message: format!(
                "language_server={}",
                editor_context.services.language_server.is_some()
            ),
        })
    }
}

fn manager_with_probe() -> PluginManager {
    let mut manager = PluginManager::new();
    manager
        .builtin_registry_mut()
        .register_provider(Arc::new(Probe));
    manager.register_builtin_editors();
    manager
}

fn created_with(manager: &mut PluginManager, cx: &mut gpui::VisualTestContext) -> String {
    let outcome = cx.update(|window, app| {
        manager.create_editor_for_file(&PathBuf::from("thing.svcprobe"), window, app)
    });
    match outcome {
        Ok(_) => panic!("the probe always refuses, to report what it was given"),
        Err(error) => format!("{error:?}"),
    }
}

#[gpui::test]
fn a_builtin_editor_is_created_with_the_language_server(cx: &mut TestAppContext) {
    let (_root, cx) = cx.add_window_view(|_, _| Blank);
    let analyzer = cx.update(|window, app| app.new(|cx| RustAnalyzerManager::new(window, cx)));

    let mut manager = manager_with_probe();
    assert!(
        created_with(&mut manager, cx).contains("language_server=false"),
        "nothing was provided yet"
    );

    manager.set_editor_services(EditorServices {
        language_server: Some(analyzer.clone()),
    });
    assert!(
        created_with(&mut manager, cx).contains("language_server=true"),
        "the editor context must carry the handle the host was given"
    );
}

#[gpui::test]
fn plugin_libraries_receive_the_same_handle_through_the_settings_snapshot(cx: &mut TestAppContext) {
    let (_root, cx) = cx.add_window_view(|_, _| Blank);
    let analyzer = cx.update(|window, app| app.new(|cx| RustAnalyzerManager::new(window, cx)));

    let mut manager = PluginManager::new();
    assert!(manager.snapshot_for_editor().services().language_server.is_none());

    manager.set_editor_services(EditorServices {
        language_server: Some(analyzer.clone()),
    });
    // The host replaces the snapshot wholesale whenever a setting changes;
    // that must not drop the services.
    manager.set_editor_settings_snapshot(EditorSettingsSnapshot::default());
    let snapshot = manager.snapshot_for_editor();
    assert_eq!(
        snapshot.services().language_server.as_ref().map(|a| a.entity_id()),
        Some(analyzer.entity_id())
    );
}

/// The manager is a process-wide static: it must not keep the host's
/// language server alive once the host lets it go (Pulsar-Native CI: the
/// editor's tests failed with a leaked `RustAnalyzerManager`).
#[gpui::test]
fn the_manager_does_not_keep_the_language_server_alive(cx: &mut TestAppContext) {
    let (_root, cx) = cx.add_window_view(|_, _| Blank);
    let analyzer = cx.update(|window, app| app.new(|cx| RustAnalyzerManager::new(window, cx)));

    let mut manager = PluginManager::new();
    manager.set_editor_services(EditorServices {
        language_server: Some(analyzer.clone()),
    });
    assert!(manager.editor_services().language_server.is_some());

    drop(analyzer);
    cx.run_until_parked();
    assert!(manager.editor_services().language_server.is_none());
    assert!(manager.snapshot_for_editor().services().language_server.is_none());
}
