//! Read-only descriptions of installed plugins, for the plugin manager window.
//!
//! Entry points: [`PluginManager::plugin_details`], [`PluginManager::get_plugins`]
//! and [`PluginManager::get_builtin_plugins`].

use crate::{DependencyState, DependencyStatus, LoadedPlugin, PluginDetails, PluginManager};
use plugin_editor_api::*;

impl PluginManager {
    /// What the plugin manager window shows for every plugin, DLL and
    /// built-in: identity, declared dependencies with their current state,
    /// the plugins that depend on it, and what it contributes.
    pub fn plugin_details(&self) -> Vec<PluginDetails> {
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

        let mut details: Vec<PluginDetails> = manifests
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
                    builtin: false,
                    library_path: Some(loaded.library.path().to_path_buf()),
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
            .collect();

        details.extend(self.builtin_registry.providers().iter().map(|provider| {
            let metadata = self
                .get_builtin_plugins()
                .into_iter()
                .find(|m| m.id.as_str() == provider.provider_id())
                .unwrap_or_else(|| PluginMetadata {
                    id: PluginId::new(provider.provider_id()),
                    name: provider.provider_id().to_string(),
                    version: String::new(),
                    author: "Pulsar Team".to_string(),
                    description: String::new(),
                });
            PluginDetails {
                metadata,
                builtin: true,
                library_path: None,
                dependencies: Vec::new(),
                required_by: Vec::new(),
                file_types: provider
                    .file_types()
                    .into_iter()
                    .map(|t| format!("{} (.{})", t.display_name, t.extension))
                    .collect(),
                editors: provider
                    .editors()
                    .into_iter()
                    .map(|e| e.display_name)
                    .collect(),
                components: Vec::new(),
                statusbar_buttons: Vec::new(),
                ai_tools: Vec::new(),
                subsystems: Vec::new(),
            }
        }));
        details
    }

    /// Get all loaded plugins.
    pub fn get_plugins(&self) -> Vec<&PluginMetadata> {
        self.plugins.values().map(|p| &p.metadata).collect()
    }

    /// Get metadata entries for all registered built-in editor providers.
    ///
    /// Built-in providers are compiled into the engine and do not have plugin
    /// manifests, so their display metadata is derived from their editor and
    /// file-type registrations.
    pub fn get_builtin_plugins(&self) -> Vec<PluginMetadata> {
        self.builtin_registry
            .providers()
            .iter()
            .map(|provider| {
                let editors = provider.editors();
                let file_types = provider.file_types();
                let name = editors
                    .iter()
                    .map(|editor| editor.display_name.as_str())
                    .collect::<Vec<_>>()
                    .join(" · ");
                let description = if file_types.is_empty() {
                    "Built-in editor provider".to_string()
                } else {
                    format!(
                        "Built-in editor for {}",
                        file_types
                            .iter()
                            .map(|file_type| file_type.display_name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };

                PluginMetadata {
                    id: PluginId::new(provider.provider_id()),
                    name: if name.is_empty() {
                        provider.provider_id().to_string()
                    } else {
                        name
                    },
                    version: String::new(),
                    author: "Pulsar Team".to_string(),
                    description,
                }
            })
            .collect()
    }
}
