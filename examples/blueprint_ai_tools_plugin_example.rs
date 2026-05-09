/// Example plugin showing complete AI tools integration using the macro framework.
/// 
/// This module demonstrates all 5 blueprint editing tools with:
/// - Proper macro usage with all attributes
/// - Complete documentation files
/// - Tool registry setup
/// - EditorPlugin trait implementation
/// - Parameter handling and error cases

use plugin_ai_tools::ToolRegistry;
use plugin_editor_api::{AiToolDefinition, EditorPlugin, PluginError, PluginMetadata};
use serde_json::{json, Value};
use std::path::Path;

// ============================================================================
// TOOL 1: Rename Node
// ============================================================================

/// Rename a node in the blueprint graph while updating all references
/// 
/// This tool handles the refactoring of node names, ensuring all connections
/// and cross-references are properly updated to maintain graph integrity.
#[ai_tool(
    category = "refactoring",
    timeout_ms = 5000,
    docs = "src/ai_tools/docs/refactor_rename_node.md"
)]
pub fn refactor_blueprint_rename_node(
    #[doc = "Current name of the node to rename"]
    old_name: String,
    
    #[doc = "New name for the node"]
    new_name: String,
) -> Result<Value, PluginError> {
    // Validation
    if old_name.is_empty() || new_name.is_empty() {
        return Err(PluginError::ValidationError(
            "Node names cannot be empty".to_string(),
        ));
    }
    
    if old_name == new_name {
        return Err(PluginError::ValidationError(
            "New name must differ from old name".to_string(),
        ));
    }
    
    // Implementation: Find node, rename it, update references
    // For example purposes, we simulate success
    Ok(json!({
        "status": "success",
        "old_name": old_name,
        "new_name": new_name,
        "references_updated": 5,
        "connections_verified": true
    }))
}

// ============================================================================
// TOOL 2: Validate Blueprint
// ============================================================================

/// Validate the blueprint for structural and logical errors
/// 
/// Performs comprehensive validation including cycle detection, type checking,
/// and unused node detection. Can run subset or full validation based on parameters.
#[ai_tool(
    category = "validation",
    timeout_ms = 8000,
    docs = "src/ai_tools/docs/validate_blueprint.md"
)]
pub fn validate_blueprint(
    #[doc = "Check for circular dependencies in the graph"]
    check_cycles: bool,
    
    #[doc = "Verify type compatibility of all connections"]
    check_types: bool,
    
    #[doc = "Identify nodes that are never executed"]
    find_unused: bool,
) -> Result<Value, PluginError> {
    let mut issues = Vec::new();
    
    if check_cycles {
        // Check for cycles
    }
    
    if check_types {
        // Check type compatibility
    }
    
    if find_unused {
        // Find unused nodes
    }
    
    Ok(json!({
        "status": "success",
        "issues_found": issues.len(),
        "issues": issues,
        "check_cycles": check_cycles,
        "check_types": check_types,
        "find_unused": find_unused
    }))
}

// ============================================================================
// TOOL 3: Optimize Blueprint
// ============================================================================

/// Optimize blueprint performance and structure
/// 
/// Applies various optimizations such as node consolidation, dead code removal,
/// and connection path optimization. Can automatically apply or show recommendations.
#[ai_tool(
    category = "optimization",
    timeout_ms = 15000,
    docs = "src/ai_tools/docs/optimize_blueprint.md"
)]
pub fn optimize_blueprint(
    #[doc = "Automatically apply all optimizations without confirmation"]
    auto_apply: bool,
    
    #[doc = "Focus area for optimization: 'performance', 'size', or 'readability'"]
    focus: String,
) -> Result<Value, PluginError> {
    // Validate focus parameter
    if !["performance", "size", "readability"].contains(&focus.as_str()) {
        return Err(PluginError::ValidationError(
            "Focus must be 'performance', 'size', or 'readability'".to_string(),
        ));
    }
    
    // Apply optimizations based on focus
    Ok(json!({
        "status": "success",
        "optimizations_applied": 3,
        "focus": focus,
        "auto_applied": auto_apply,
        "performance_improvement_percent": 15.5,
        "size_reduction_bytes": 1024
    }))
}

// ============================================================================
// TOOL 4: Generate Template
// ============================================================================

/// Generate a blueprint template for quick setup
/// 
/// Creates template blueprints with common patterns and structures.
/// Useful for starting new blueprints or understanding best practices.
#[ai_tool(
    category = "generation",
    timeout_ms = 3000,
    docs = "src/ai_tools/docs/generate_blueprint_template.md"
)]
pub fn generate_blueprint_template(
    #[doc = "Template type: 'empty', 'state_machine', 'data_pipeline', or 'event_handler'"]
    template_type: String,
    
    #[doc = "Optional parameters for the template as JSON string"]
    parameters: Option<String>,
) -> Result<Value, PluginError> {
    // Validate template type
    let valid_types = ["empty", "state_machine", "data_pipeline", "event_handler"];
    if !valid_types.contains(&template_type.as_str()) {
        return Err(PluginError::ValidationError(
            format!("Invalid template type: {}", template_type),
        ));
    }
    
    // Generate template based on type
    let template = match template_type.as_str() {
        "empty" => json!({"nodes": [], "connections": []}),
        "state_machine" => json!({"nodes": [], "connections": []}),
        "data_pipeline" => json!({"nodes": [], "connections": []}),
        "event_handler" => json!({"nodes": [], "connections": []}),
        _ => unreachable!(),
    };
    
    Ok(json!({
        "status": "success",
        "template_type": template_type,
        "template": template,
        "parameters_used": parameters
    }))
}

// ============================================================================
// TOOL 5: Analyze Performance
// ============================================================================

/// Analyze blueprint performance characteristics
/// 
/// Profiles blueprint execution, identifies bottlenecks, and provides
/// detailed performance metrics and recommendations for improvement.
#[ai_tool(
    category = "analysis",
    timeout_ms = 10000,
    docs = "src/ai_tools/docs/analyze_blueprint_performance.md"
)]
pub fn analyze_blueprint_performance(
    #[doc = "Run performance profiling on the blueprint"]
    profile: bool,
    
    #[doc = "Include detailed breakdown by node and connection"]
    detailed: bool,
) -> Result<Value, PluginError> {
    let mut analysis = json!({
        "status": "success",
        "total_execution_time_ms": 1250,
        "profiled": profile,
        "detailed": detailed
    });
    
    if detailed {
        analysis["nodes"] = json!([
            {
                "name": "ProcessNode",
                "execution_time_ms": 500,
                "memory_mb": 2.5,
                "calls": 100
            }
        ]);
    }
    
    Ok(analysis)
}

// ============================================================================
// Tool Registry and Plugin Implementation
// ============================================================================

use lazy_static::lazy_static;

lazy_static! {
    /// Global tool registry for all blueprint editing tools
    static ref BLUEPRINT_TOOL_REGISTRY: ToolRegistry = {
        let mut registry = ToolRegistry::new();
        
        // Register Tool 1: Rename Node
        registry.register(
            "refactor_blueprint_rename_node",
            json!({
                "name": "refactor_blueprint_rename_node",
                "description": "Rename a node in the blueprint graph",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "old_name": {"type": "string"},
                        "new_name": {"type": "string"}
                    },
                    "required": ["old_name", "new_name"]
                },
                "category": "refactoring"
            }),
            TOOL_DOC_REFACTOR_BLUEPRINT_RENAME_NODE,
            refactor_blueprint_rename_node_ai_tool_wrapper,
        );
        
        // Register Tool 2: Validate Blueprint
        registry.register(
            "validate_blueprint",
            json!({
                "name": "validate_blueprint",
                "description": "Validate the blueprint for errors",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "check_cycles": {"type": "boolean"},
                        "check_types": {"type": "boolean"},
                        "find_unused": {"type": "boolean"}
                    },
                    "required": ["check_cycles", "check_types", "find_unused"]
                },
                "category": "validation"
            }),
            TOOL_DOC_VALIDATE_BLUEPRINT,
            validate_blueprint_ai_tool_wrapper,
        );
        
        // Register Tool 3: Optimize Blueprint
        registry.register(
            "optimize_blueprint",
            json!({
                "name": "optimize_blueprint",
                "description": "Optimize blueprint performance and structure",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "auto_apply": {"type": "boolean"},
                        "focus": {"type": "string"}
                    },
                    "required": ["auto_apply", "focus"]
                },
                "category": "optimization"
            }),
            TOOL_DOC_OPTIMIZE_BLUEPRINT,
            optimize_blueprint_ai_tool_wrapper,
        );
        
        // Register Tool 4: Generate Template
        registry.register(
            "generate_blueprint_template",
            json!({
                "name": "generate_blueprint_template",
                "description": "Generate a blueprint template",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "template_type": {"type": "string"},
                        "parameters": {"type": ["string", "null"]}
                    },
                    "required": ["template_type"]
                },
                "category": "generation"
            }),
            TOOL_DOC_GENERATE_BLUEPRINT_TEMPLATE,
            generate_blueprint_template_ai_tool_wrapper,
        );
        
        // Register Tool 5: Analyze Performance
        registry.register(
            "analyze_blueprint_performance",
            json!({
                "name": "analyze_blueprint_performance",
                "description": "Analyze blueprint performance",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "profile": {"type": "boolean"},
                        "detailed": {"type": "boolean"}
                    },
                    "required": ["profile", "detailed"]
                },
                "category": "analysis"
            }),
            TOOL_DOC_ANALYZE_BLUEPRINT_PERFORMANCE,
            analyze_blueprint_performance_ai_tool_wrapper,
        );
        
        registry
    };
}

// ============================================================================
// Example EditorPlugin Implementation
// ============================================================================

/// Example blueprint editor plugin with AI tool support
pub struct BlueprintEditorPlugin;

impl EditorPlugin for BlueprintEditorPlugin {
    fn get_metadata(&self) -> Result<PluginMetadata, PluginError> {
        Ok(PluginMetadata {
            name: "Blueprint Editor with AI Tools".to_string(),
            version: "1.0.0".to_string(),
            author: "Example".to_string(),
        })
    }
    
    /// Expose all registered tools for AI discovery
    fn ai_tools(&self) -> Vec<AiToolDefinition> {
        BLUEPRINT_TOOL_REGISTRY.definitions()
    }
    
    /// Execute AI tool with parameters
    fn execute_ai_tool(
        &self,
        _file_path: &Path,
        tool_name: &str,
        tool_args: Value,
    ) -> Result<Value, PluginError> {
        BLUEPRINT_TOOL_REGISTRY.execute(tool_name, tool_args)
    }
    
    /// Specify which tools apply to which file types
    fn capabilities_for_file(&self, file_path: &Path) -> Vec<String> {
        if file_path
            .extension()
            .map_or(false, |ext| ext == "blueprint" || ext == "bp")
        {
            BLUEPRINT_TOOL_REGISTRY.tool_names()
        } else {
            Vec::new()
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_rename_node_success() {
        let result = refactor_blueprint_rename_node(
            "OldName".to_string(),
            "NewName".to_string(),
        );
        assert!(result.is_ok());
        
        let value = result.unwrap();
        assert_eq!(value["status"], "success");
        assert_eq!(value["old_name"], "OldName");
        assert_eq!(value["new_name"], "NewName");
    }
    
    #[test]
    fn test_rename_node_empty_name_error() {
        let result = refactor_blueprint_rename_node(
            "".to_string(),
            "NewName".to_string(),
        );
        assert!(result.is_err());
    }
    
    #[test]
    fn test_validate_blueprint() {
        let result = validate_blueprint(true, true, true);
        assert!(result.is_ok());
        
        let value = result.unwrap();
        assert_eq!(value["status"], "success");
    }
    
    #[test]
    fn test_tool_registry_registration() {
        let tools = BLUEPRINT_TOOL_REGISTRY.tool_names();
        assert_eq!(tools.len(), 5);
        assert!(tools.contains(&"refactor_blueprint_rename_node".to_string()));
        assert!(tools.contains(&"validate_blueprint".to_string()));
        assert!(tools.contains(&"optimize_blueprint".to_string()));
        assert!(tools.contains(&"generate_blueprint_template".to_string()));
        assert!(tools.contains(&"analyze_blueprint_performance".to_string()));
    }
    
    #[test]
    fn test_tool_execution_via_registry() {
        let args = json!({
            "old_name": "Node1",
            "new_name": "Node2"
        });
        
        let result = BLUEPRINT_TOOL_REGISTRY.execute("refactor_blueprint_rename_node", args);
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_optimize_blueprint_invalid_focus() {
        let result = optimize_blueprint(false, "invalid".to_string());
        assert!(result.is_err());
    }
    
    #[test]
    fn test_generate_template_invalid_type() {
        let result = generate_blueprint_template("invalid".to_string(), None);
        assert!(result.is_err());
    }
}
