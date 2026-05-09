/// Plugin AI Tools Runtime Support
///
/// This module provides runtime helpers for plugins to automatically
/// collect, register, and execute AI tools defined with #[ai_tool] macro.

use serde_json::{json, Value};
use plugin_editor_api::{AiToolDefinition, PluginError};
use std::collections::HashMap;
use std::path::Path;

/// A registered AI tool with its metadata and handler
pub struct RegisteredTool {
    pub definition: AiToolDefinition,
    pub handler: Box<dyn Fn(Value) -> Result<Value, PluginError> + Send + Sync>,
    pub documentation: String,
}

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
    )
    where
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

/// Macro to register a tool from an #[ai_tool] function
/// 
/// # Usage
/// 
/// ```rust,ignore
/// #[ai_tool(category = "formatting")]
/// /// Format the file
/// pub fn format_file(width: u32) -> Result<Value, PluginError> {
///     // ...
/// }
/// 
/// let mut tools = ToolRegistry::new();
/// tools.register_tool!(format_file);
/// ```
#[macro_export]
macro_rules! register_ai_tool {
    ($registry:expr, $tool_fn:path) => {{
        let tool_name = stringify!($tool_fn);
        // In actual implementation, this would use the generated wrapper
        // For now, it's a placeholder that would be replaced by macro expansion
    }};
}

/// Helper to collect all tools for a plugin
/// 
/// Plugins implement this trait to provide their tools
pub trait AiToolProvider {
    /// Create a registry with all this plugin's tools
    fn create_tool_registry() -> ToolRegistry;

    /// Get capabilities for a specific file
    fn capabilities_for_file(&self, _file_path: &Path) -> Vec<String> {
        Vec::new()
    }
}

/// Example trait implementation helper
pub struct SimpleToolProvider;

impl SimpleToolProvider {
    /// Create a registry and register a single tool
    pub fn register_single_tool(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
        handler: impl Fn(Value) -> Result<Value, PluginError> + Send + Sync + 'static,
    ) -> ToolRegistry {
        let mut registry = ToolRegistry::new();

        let definition = AiToolDefinition {
            name: name.into(),
            description: description.into(),
            parameters_json_schema: parameters,
            category: None,
        };

        let doc = format!("Tool documentation for {}", definition.name);
        registry.register(definition.name.clone(), definition, doc, handler);

        registry
    }
}

/// Builder for creating AiToolDefinition more easily
pub struct AiToolBuilder {
    name: String,
    description: String,
    parameters: serde_json::Value,
    category: Option<String>,
}

impl AiToolBuilder {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            category: None,
        }
    }

    pub fn parameter(
        mut self,
        param_name: &str,
        param_type: &str,
        description: Option<&str>,
    ) -> Self {
        if let Some(props) = self.parameters.get_mut("properties") {
            if let Some(obj) = props.as_object_mut() {
                let mut param_def = json!({
                    "type": param_type,
                });
                if let Some(desc) = description {
                    param_def["description"] = json!(desc);
                }
                obj.insert(param_name.to_string(), param_def);
            }
        }

        if let Some(required) = self.parameters.get_mut("required") {
            if let Some(arr) = required.as_array_mut() {
                arr.push(json!(param_name));
            }
        }

        self
    }

    pub fn optional_parameter(
        mut self,
        param_name: &str,
        param_type: &str,
        description: Option<&str>,
    ) -> Self {
        if let Some(props) = self.parameters.get_mut("properties") {
            if let Some(obj) = props.as_object_mut() {
                let mut param_def = json!({
                    "type": param_type,
                });
                if let Some(desc) = description {
                    param_def["description"] = json!(desc);
                }
                obj.insert(param_name.to_string(), param_def);
            }
        }

        self
    }

    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    pub fn build(self) -> AiToolDefinition {
        AiToolDefinition {
            name: self.name,
            description: self.description,
            parameters_json_schema: self.parameters,
            category: self.category,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_builder() {
        let definition = AiToolBuilder::new("test_tool", "A test tool")
            .parameter("input", "string", Some("The input string"))
            .optional_parameter("width", "integer", None)
            .category("testing")
            .build();

        assert_eq!(definition.name, "test_tool");
        assert_eq!(definition.category, Some("testing".to_string()));
    }

    #[test]
    fn test_tool_registry() {
        let mut registry = ToolRegistry::new();

        let definition = AiToolBuilder::new("format", "Format a file").build();

        registry.register(
            "format",
            definition,
            "Format documentation",
            |_args| Ok(json!({"status": "formatted"})),
        );

        let result = registry.execute("format", json!({}));
        assert!(result.is_ok());
    }
}
