//! Plugin AI tools: building tool bridges and running a plugin's tool.
//!
//! Entry points: [`PluginManager::build_tool_bridge`],
//! [`PluginManager::build_tool_bridge_for_file`],
//! [`PluginManager::get_plugin_ai_tools`] and
//! [`PluginManager::execute_plugin_ai_tool`]. The bridge type itself lives in
//! `tool_bridge.rs`.

use crate::{PluginManager, PluginManagerError, PluginToolBridge};
use plugin_editor_api::*;
use std::path::Path;

impl PluginManager {
    /// Build a snapshot bridge containing AI tools exposed by all loaded plugins.
    pub fn build_tool_bridge(&self) -> PluginToolBridge {
        tracing::debug!(
            plugin_count = self.plugins.len(),
            builtin_count = self.builtin_registry.providers().len(),
            "build_tool_bridge start"
        );
        let mut bridge = PluginToolBridge::new();
        for (plugin_id, loaded_plugin) in &self.plugins {
            bridge.discover_plugin_tools(plugin_id.clone(), loaded_plugin.plugin);
        }

        for provider in self.builtin_registry.providers() {
            bridge.discover_builtin_tools(PluginId::new(provider.provider_id()), provider.clone());
        }

        // Set filesystem context from project root, if available.
        if let Some(root) = &self.project_root {
            bridge.set_fs_context(FsContext::unrestricted(root.clone()));
        }

        tracing::debug!(
            tool_count = bridge.tool_names().len(),
            "build_tool_bridge end"
        );
        bridge
    }

    /// Build a snapshot bridge containing AI tools applicable to a specific file.
    pub fn build_tool_bridge_for_file(&self, file_path: &Path) -> PluginToolBridge {
        tracing::debug!(file = %file_path.display(), plugin_count = self.plugins.len(), builtin_count = self.builtin_registry.providers().len(), "build_tool_bridge_for_file start");
        let mut bridge = PluginToolBridge::new();
        for (plugin_id, loaded_plugin) in &self.plugins {
            bridge.discover_plugin_tools_for_file(
                plugin_id.clone(),
                loaded_plugin.plugin,
                file_path,
            );
        }

        for provider in self.builtin_registry.providers() {
            bridge.discover_builtin_tools_for_file(
                PluginId::new(provider.provider_id()),
                provider.clone(),
                file_path,
            );
        }

        // Set filesystem context from project root, if available.
        if let Some(root) = &self.project_root {
            bridge.set_fs_context(FsContext::unrestricted(root.clone()));
        }

        tracing::debug!(file = %file_path.display(), tool_count = bridge.tool_names().len(), "build_tool_bridge_for_file end");
        bridge
    }

    /// Get AI tools exposed by a specific plugin or built-in provider.
    pub fn get_plugin_ai_tools(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Vec<AiToolDefinition>, PluginManagerError> {
        if let Some(loaded_plugin) = self.plugins.get(plugin_id) {
            return Ok(loaded_plugin.plugin.ai_tools());
        }

        if let Some(provider) = self.builtin_registry.provider_by_id(plugin_id.as_str()) {
            return Ok(provider.ai_tools());
        }

        Err(PluginManagerError::PluginNotFound {
            plugin_id: plugin_id.clone(),
        })
    }

    /// Execute an AI tool against a specific plugin or built-in provider.
    pub fn execute_plugin_ai_tool(
        &self,
        plugin_id: &PluginId,
        file_path: &Path,
        tool_name: &str,
        tool_args: JsonValue,
    ) -> Result<JsonValue, PluginManagerError> {
        if let Some(loaded_plugin) = self.plugins.get(plugin_id) {
            return loaded_plugin
                .plugin
                .execute_ai_tool(file_path, tool_name, tool_args)
                .map_err(|error| PluginManagerError::PluginError {
                    plugin_id: plugin_id.clone(),
                    error,
                });
        }

        if let Some(provider) = self.builtin_registry.provider_by_id(plugin_id.as_str()) {
            return provider
                .execute_ai_tool(file_path, tool_name, tool_args)
                .map_err(|error| PluginManagerError::PluginError {
                    plugin_id: plugin_id.clone(),
                    error,
                });
        }

        Err(PluginManagerError::PluginNotFound {
            plugin_id: plugin_id.clone(),
        })
    }
}
