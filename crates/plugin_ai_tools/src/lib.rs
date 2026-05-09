use plugin_editor_api::{AiToolDefinition, PluginError};
/// Plugin AI Tools Runtime Support
///
/// This module provides runtime helpers for plugins to automatically
/// collect, register, and execute AI tools defined with #[ai_tool] macro.
use serde_json::{json, Value};
use std::collections::HashMap;

pub use inventory;

/// A registered AI tool with its metadata and handler
pub struct RegisteredTool {
    pub definition: AiToolDefinition,
    pub handler: Box<dyn Fn(Value) -> Result<Value, PluginError> + Send + Sync>,
    pub documentation: String,
}

pub struct GeneratedToolEntry {
    pub namespace: &'static str,
    pub definition: &'static str,
    pub documentation: &'static str,
    pub handler: fn(Value) -> Result<Value, PluginError>,
}

inventory::collect!(GeneratedToolEntry);

/// Tool registry for plugins
///
/// Collects all #[ai_tool] marked functions in a plugin
pub struct ToolRegistry {
    tools: HashMap<String, RegisteredTool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool
    pub fn register<F>(
        &mut self,
        name: impl Into<String>,
        definition: AiToolDefinition,
        documentation: impl Into<String>,
        handler: F,
    ) where
        F: Fn(Value) -> Result<Value, PluginError> + Send + Sync + 'static,
    {
        let name_str = name.into();
        self.tools.insert(
            name_str.clone(),
            RegisteredTool {
                definition,
                handler: Box::new(handler),
                documentation: documentation.into(),
            },
        );
    }

    /// Get all tool definitions
    pub fn definitions(&self) -> Vec<AiToolDefinition> {
        self.tools.values().map(|t| t.definition.clone()).collect()
    }

    /// Execute a tool by name
    pub fn execute(&self, tool_name: &str, args: Value) -> Result<Value, PluginError> {
        let tool = self
            .tools
            .get(tool_name)
            .ok_or_else(|| PluginError::Other {
                message: format!("Unknown tool: {}", tool_name),
            })?;

        (tool.handler)(args)
    }

    /// Get documentation for a tool
    pub fn documentation(&self, tool_name: &str) -> Option<&str> {
        self.tools.get(tool_name).map(|t| t.documentation.as_str())
    }

    /// List all tool names
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn parse_generated_tool_definition(definition: &str) -> Result<AiToolDefinition, PluginError> {
    let value: Value = serde_json::from_str(definition).map_err(|err| PluginError::Other {
        message: format!("Invalid generated tool definition: {err}"),
    })?;

    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PluginError::Other {
            message: "Tool definition missing name".to_string(),
        })?
        .to_string();
    let description = value
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let parameters_json_schema = value
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));

    let mut def = AiToolDefinition::new(name, description, parameters_json_schema);
    if let Some(category) = value.get("category").and_then(|v| v.as_str()) {
        def = def.with_category(category.to_string());
    }

    Ok(def)
}

pub fn register_generated_tool<F>(
    registry: &mut ToolRegistry,
    definition: &str,
    documentation: impl Into<String>,
    handler: F,
) -> Result<(), PluginError>
where
    F: Fn(Value) -> Result<Value, PluginError> + Send + Sync + 'static,
{
    let parsed = parse_generated_tool_definition(definition)?;
    let name = parsed.name.clone();
    registry.register(name, parsed, documentation, handler);
    Ok(())
}

pub fn registry_from_inventory(namespace_prefix: &str) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    for entry in inventory::iter::<GeneratedToolEntry> {
        if !entry.namespace.starts_with(namespace_prefix) {
            continue;
        }

        register_generated_tool(
            &mut registry,
            entry.definition,
            entry.documentation,
            entry.handler,
        )
        .expect("invalid generated AI tool metadata from inventory");
    }

    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_registry() {
        let mut registry = ToolRegistry::new();
        let definition = AiToolDefinition::new(
            "format",
            "Format a file",
            json!({"type": "object", "properties": {}}),
        );

        registry.register("format", definition, "Format documentation", |_args| {
            Ok(json!({"status": "formatted"}))
        });

        let result = registry.execute("format", json!({}));
        assert!(result.is_ok());
    }
}
