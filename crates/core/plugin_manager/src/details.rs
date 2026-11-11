//! Read-only descriptions of the plugin libraries loaded at runtime, joined
//! into the plugin catalog for the Extensions viewer.
//!
//! Entry points: [`PluginManager::catalog`], [`PluginManager::library_details`]
//! and [`PluginManager::get_plugins`].

use crate::catalog::CatalogSnapshot;
use crate::{DependencyState, DependencyStatus, LoadedPlugin, PluginDetails, PluginManager};
use plugin_editor_api::*;

impl PluginManager {
    /// The plugin catalog ([`CatalogSnapshot::snapshot`]: every installed
    /// plugin, its contributions and contracts, the activation) with the
    /// plugin libraries this manager loaded: what the Extensions viewer
    /// shows.
    pub fn catalog(&self) -> CatalogSnapshot {
        let mut catalog = CatalogSnapshot::snapshot();
        catalog.libraries = self.library_details();
        catalog
    }

    /// Every loaded plugin library: identity, declared dependencies with
    /// their current state, the libraries that depend on it, and what it
    /// contributes. (Linked plugins' editor providers are contributions of
    /// their plugin in the catalog.)
    pub fn library_details(&self) -> Vec<PluginDetails> {
        let manifests: Vec<(PluginManifest, &LoadedPlugin)> = self
            .plugins
            .values()
            .map(|loaded| (loaded.plugin.manifest(), loaded))
            .collect();

        let status = |dependency: &PluginDependency, optional: bool| {
            let state = match manifests.iter().find(|(m, _)| m.id == dependency.id) {
                None => DependencyState::Missing,
                Some((installed, _)) => {
                    if plugin_editor_api::manifest::satisfies(installed, dependency) {
                        DependencyState::Satisfied {
                            version: installed.version.clone(),
                        }
                    } else {
                        DependencyState::VersionMismatch {
                            found: installed.version.clone(),
                        }
                    }
                }
            };
            DependencyStatus {
                dependency: dependency.clone(),
                optional,
                state,
            }
        };

        manifests
            .iter()
            .map(|(manifest, loaded)| {
                let plugin = loaded.plugin;
                let mut dependencies: Vec<DependencyStatus> =
                    manifest.requires.iter().map(|d| status(d, false)).collect();
                dependencies.extend(manifest.optional.iter().map(|d| status(d, true)));
                let mut required_by: Vec<PluginId> = manifests
                    .iter()
                    .filter(|(other, _)| {
                        other
                            .requires
                            .iter()
                            .chain(&other.optional)
                            .any(|d| d.id == manifest.id)
                    })
                    .map(|(other, _)| other.id.clone())
                    .collect();
                required_by.sort_by(|a, b| a.as_str().cmp(b.as_str()));
                PluginDetails {
                    metadata: manifest.metadata(),
                    library_path: loaded.library.path().to_path_buf(),
                    dependencies,
                    required_by,
                    file_types: plugin
                        .file_types()
                        .into_iter()
                        .map(|t| format!("{} (.{})", t.display_name, t.extension))
                        .collect(),
                    editors: plugin
                        .editors()
                        .into_iter()
                        .map(|e| e.display_name)
                        .collect(),
                    components: plugin
                        .component_definitions()
                        .into_iter()
                        .map(|c| c.display_name)
                        .collect(),
                    statusbar_buttons: plugin
                        .statusbar_buttons()
                        .into_iter()
                        .map(|button| button.tooltip)
                        .collect(),
                    ai_tools: plugin
                        .ai_tools()
                        .into_iter()
                        .map(|tool| tool.name)
                        .collect(),
                    subsystems: loaded.subsystem_ids.clone(),
                }
            })
            .collect()
    }

    /// Changes whenever the set of plugins [`Self::catalog`] reports from
    /// this manager changes: a plugin library installed, a built-in editor
    /// provider registered.
    /// Plugins are never unloaded, so the count of both is that revision. A
    /// view of the details (the Plugin Manager window) reloads when it
    /// differs from the one it drew.
    pub fn revision(&self) -> usize {
        self.plugins.len() + self.builtin_registry.providers().len()
    }

    /// Get all loaded plugins.
    pub fn get_plugins(&self) -> Vec<&PluginMetadata> {
        self.plugins.values().map(|p| &p.metadata).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use gpui::{App, Window};
    use plugin_editor_api::{
        AiToolDefinition, EditorId, EditorMetadata, FileStructure, FileTypeDefinition, FileTypeId,
        PluginError,
    };

    use crate::builtin::EditorContext;
    use ui::dock::PanelView;

    use crate::{BuiltinEditorProvider, PluginEditorProvider, PluginManager};

    /// A built-in provider with one file type, one editor and one AI tool.
    struct Notes;

    impl BuiltinEditorProvider for Notes {
        fn provider_id(&self) -> &str {
            "test.notes"
        }
        fn file_types(&self) -> Vec<FileTypeDefinition> {
            vec![FileTypeDefinition {
                id: FileTypeId::new("notes"),
                extension: "notes".into(),
                display_name: "Notes".into(),
                icon: ui::IconName::Folder,
                color: gpui::Hsla::default(),
                structure: FileStructure::Standalone,
                default_content: serde_json::Value::Null,
                categories: Vec::new(),
                creation_directory: None,
            }]
        }
        fn editors(&self) -> Vec<EditorMetadata> {
            vec![EditorMetadata {
                id: EditorId::new("notes-editor"),
                display_name: "Notes Editor".into(),
                supported_file_types: vec![FileTypeId::new("notes")],
            }]
        }
        fn ai_tools(&self) -> Vec<AiToolDefinition> {
            vec![AiToolDefinition {
                name: "notes.append".into(),
                description: String::new(),
                parameters_json_schema: serde_json::json!({}),
                category: None,
            }]
        }
        fn can_handle(&self, _: &EditorId) -> bool {
            false
        }
        fn create_editor(
            &self,
            _: PathBuf,
            _: &EditorContext,
            _: &mut Window,
            _: &mut App,
        ) -> Result<Arc<dyn PanelView>, PluginError> {
            Err(PluginError::Other {
                message: "not in a test".into(),
            })
        }
    }

    inventory::submit! {
        PluginEditorProvider {
            origin: pulsar_plugins::engine_origin!(),
            id: "test.notes",
            create: || Arc::new(Notes),
        }
    }

    /// The Extensions viewer reads this: a linked provider is listed in the
    /// catalog under its owner (here the engine) with the file types,
    /// editors and AI tools it brings, and registering it changes the
    /// revision the viewer watches.
    #[test]
    fn the_catalog_lists_a_linked_provider_and_what_it_brings() {
        let mut manager = PluginManager::new();
        let before = manager.revision();
        manager.builtin_registry_mut().register_linked();
        manager.register_builtin_editors();
        assert_ne!(
            manager.revision(),
            before,
            "a new provider is a new revision"
        );

        let catalog = manager.catalog();
        assert!(catalog.libraries.is_empty(), "no library is loaded");
        let notes = catalog
            .engine
            .iter()
            .find(|summary| summary.category == "editor.provider" && summary.id == "test.notes")
            .expect("the provider is listed as the engine's");
        assert_eq!(notes.label, "Notes Editor");
        let children: Vec<(&str, &str)> = notes
            .children
            .iter()
            .map(|child| (child.category, child.label.as_str()))
            .collect();
        assert_eq!(
            children,
            [
                ("editor.file_type", "Notes (.notes)"),
                ("editor.editor", "Notes Editor"),
                ("editor.ai_tool", "notes.append"),
            ]
        );
    }
}
