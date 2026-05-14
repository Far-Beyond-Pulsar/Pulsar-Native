/// Bridge between plugin system and AI tool system.
///
/// The PluginToolBridge manages tool discovery and execution across all loaded plugins.
/// It provides a unified interface for the AI agent system to:
/// - Query which tools are available
/// - Get documentation for tools
/// - Execute tools with parameters
use crate::builtin::BuiltinEditorProvider;
use plugin_editor_api::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

type ToolExecFn = Arc<dyn Fn(&Path, serde_json::Value) -> Result<serde_json::Value, PluginError> + Send + Sync>;

/// Represents a tool available from a specific plugin
#[derive(Clone)]
pub struct AvailableTool {
    /// The tool definition
    pub definition: AiToolDefinition,

    /// The plugin ID that provides this tool
    pub plugin_id: PluginId,

    /// File types this tool applies to (empty = applies to all)
    pub file_types: Vec<String>,

    /// Optional direct execution closure captured at bridge build time.
    pub execute: Option<ToolExecFn>,
}

/// Bridges between plugin system and AI tool system
pub struct PluginToolBridge {
    /// Cached tools from all plugins
    tools: HashMap<String, AvailableTool>,

    /// Tool name -> (plugin_id, tool_name) mapping
    tool_to_plugin: HashMap<String, (PluginId, String)>,
}

impl PluginToolBridge {
    /// Create a new, empty tool bridge
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            tool_to_plugin: HashMap::new(),
        }
    }

    /// Discover all tools from a specific plugin
    pub fn discover_plugin_tools(
        &mut self,
        plugin_id: PluginId,
        plugin: &'static dyn EditorPlugin,
    ) {
        let tool_defs = plugin.ai_tools();

        for tool_def in tool_defs {
            let tool_name = tool_def.name.clone();
            let tool_name_for_exec = tool_name.clone();
            let exec: ToolExecFn = Arc::new(move |file_path, tool_args| {
                plugin.execute_ai_tool(file_path, &tool_name_for_exec, tool_args)
            });

            let available_tool = AvailableTool {
                definition: tool_def,
                plugin_id: plugin_id.clone(),
                file_types: Vec::new(),
                execute: Some(exec),
            };

            self.tools.insert(tool_name.clone(), available_tool);
            self.tool_to_plugin
                .insert(tool_name.clone(), (plugin_id.clone(), tool_name));
        }
    }

    /// Discover tools from a built-in provider.
    pub fn discover_builtin_tools(
        &mut self,
        plugin_id: PluginId,
        provider: Arc<dyn BuiltinEditorProvider>,
    ) {
        let tool_defs = provider.ai_tools();

        for tool_def in tool_defs {
            let tool_name = tool_def.name.clone();
            let tool_name_for_exec = tool_name.clone();
            let provider_for_exec = provider.clone();
            let exec: ToolExecFn = Arc::new(move |file_path, tool_args| {
                provider_for_exec.execute_ai_tool(file_path, &tool_name_for_exec, tool_args)
            });

            let available_tool = AvailableTool {
                definition: tool_def,
                plugin_id: plugin_id.clone(),
                file_types: Vec::new(),
                execute: Some(exec),
            };

            self.tools.insert(tool_name.clone(), available_tool);
            self.tool_to_plugin
                .insert(tool_name.clone(), (plugin_id.clone(), tool_name));
        }
    }

    /// Discover tools from a built-in provider for a specific file.
    pub fn discover_builtin_tools_for_file(
        &mut self,
        plugin_id: PluginId,
        provider: Arc<dyn BuiltinEditorProvider>,
        file_path: &Path,
    ) {
        let capabilities = provider.capabilities_for_file(file_path);
        if capabilities.is_empty() {
            return;
        }

        let tool_defs = provider.ai_tools();
        for tool_def in tool_defs {
            if capabilities.contains(&tool_def.name) {
                let tool_name = tool_def.name.clone();
                let tool_name_for_exec = tool_name.clone();
                let provider_for_exec = provider.clone();
                let exec: ToolExecFn = Arc::new(move |file_path, tool_args| {
                    provider_for_exec.execute_ai_tool(file_path, &tool_name_for_exec, tool_args)
                });

                self.tools
                    .entry(tool_name.clone())
                    .or_insert_with(|| AvailableTool {
                        definition: tool_def,
                        plugin_id: plugin_id.clone(),
                        file_types: vec![],
                        execute: Some(exec),
                    });

                self.tool_to_plugin
                    .insert(tool_name.clone(), (plugin_id.clone(), tool_name));
            }
        }
    }

    /// Discover tools from a plugin for a specific file type
    pub fn discover_plugin_tools_for_file(
        &mut self,
        plugin_id: PluginId,
        plugin: &'static dyn EditorPlugin,
        file_path: &Path,
    ) {
        // Get tools available for this file from the plugin
        let capabilities = plugin.capabilities_for_file(file_path);

        if !capabilities.is_empty() {
            let tool_defs = plugin.ai_tools();

            for tool_def in tool_defs {
                if capabilities.contains(&tool_def.name) {
                    let tool_name = tool_def.name.clone();
                    let tool_name_for_exec = tool_name.clone();
                    let exec: ToolExecFn = Arc::new(move |file_path, tool_args| {
                        plugin.execute_ai_tool(file_path, &tool_name_for_exec, tool_args)
                    });

                    self.tools
                        .entry(tool_name.clone())
                        .or_insert_with(|| AvailableTool {
                            definition: tool_def,
                            plugin_id: plugin_id.clone(),
                            file_types: vec![],
                            execute: Some(exec),
                        });

                    self.tool_to_plugin
                        .insert(tool_name.clone(), (plugin_id.clone(), tool_name));
                }
            }
        }
    }

    /// Get all available tools
    pub fn all_tools(&self) -> Vec<AvailableTool> {
        self.tools.values().cloned().collect()
    }

    /// Get tools for a specific file
    pub fn tools_for_file(&self, file_path: &Path) -> Vec<AvailableTool> {
        self.tools
            .values()
            .filter(|tool| {
                // If no file types specified, tool applies to all files
                if tool.file_types.is_empty() {
                    return true;
                }

                // Check if file extension matches
                if let Some(ext) = file_path.extension() {
                    if let Some(ext_str) = ext.to_str() {
                        return tool.file_types.contains(&ext_str.to_string());
                    }
                }

                false
            })
            .cloned()
            .collect()
    }

    /// Get a specific tool by name
    pub fn tool(&self, tool_name: &str) -> Option<&AvailableTool> {
        self.tools.get(tool_name)
    }

    /// Get all tool names
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Get the plugin ID for a specific tool
    pub fn plugin_for_tool(&self, tool_name: &str) -> Option<PluginId> {
        self.tool_to_plugin
            .get(tool_name)
            .map(|(plugin_id, _)| plugin_id.clone())
    }

    /// Execute a tool (requires having a reference to the plugin)
    ///
    /// This method is low-level. In practice, call through PluginManager which
    /// has access to the actual plugin instances.
    pub fn execute_tool_with_plugin(
        &self,
        tool_name: &str,
        tool_args: serde_json::Value,
        plugin: &dyn EditorPlugin,
        file_path: &Path,
    ) -> Result<serde_json::Value, PluginError> {
        plugin.execute_ai_tool(file_path, tool_name, tool_args)
    }

    /// Execute using the closure captured during discovery, if available.
    pub fn execute_tool_direct(
        &self,
        tool_name: &str,
        file_path: &Path,
        tool_args: serde_json::Value,
    ) -> Option<Result<serde_json::Value, PluginError>> {
        let tool = self.tools.get(tool_name)?;
        let exec = tool.execute.as_ref()?;
        Some(exec(file_path, tool_args))
    }

    /// Clear all cached tools
    pub fn clear(&mut self) {
        self.tools.clear();
        self.tool_to_plugin.clear();
    }

    /// Refresh tools from a plugin
    pub fn refresh_plugin(&mut self, plugin_id: &PluginId, plugin: &'static dyn EditorPlugin) {
        // Remove old tools from this plugin
        self.tools.retain(|_, tool| tool.plugin_id != *plugin_id);
        self.tool_to_plugin.retain(|_, (pid, _)| pid != plugin_id);

        // Rediscover tools
        self.discover_plugin_tools(plugin_id.clone(), plugin);
    }
}

impl Default for PluginToolBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_creation() {
        let bridge = PluginToolBridge::new();
        assert_eq!(bridge.tool_names().len(), 0);
    }

    #[test]
    fn test_tool_storage() {
        let mut bridge = PluginToolBridge::new();

        let tool = AvailableTool {
            definition: AiToolDefinition::new(
                "test_tool",
                "A test tool",
                serde_json::json!({"type": "object", "properties": {}}),
            ),
            plugin_id: PluginId::new("com.example.test"),
            file_types: vec![],
        };

        bridge.tools.insert("test_tool".to_string(), tool.clone());

        assert_eq!(bridge.tool_names().len(), 1);
        assert!(bridge.tool("test_tool").is_some());
    }
}
