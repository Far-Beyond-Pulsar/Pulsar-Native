# Plugin AI Tools - Practical Examples & Best Practices

## Table of Contents

1. [Simple Example: Text Formatter Plugin](#simple-example-text-formatter-plugin)
2. [Complex Example: Blueprint Editor Plugin](#complex-example-blueprint-editor-plugin)
3. [Cross-Plugin Workflow Example](#cross-plugin-workflow-example)
4. [Best Practices](#best-practices)
5. [Common Patterns](#common-patterns)
6. [Troubleshooting](#troubleshooting)

---

## Simple Example: Text Formatter Plugin

### Plugin Implementation

```rust
// plugins/text_formatter_plugin/src/lib.rs

use plugin_editor_api::*;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Default)]
pub struct TextFormatterPlugin;

impl EditorPlugin for TextFormatterPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new("com.example.text-formatter"),
            name: "Text Formatter".into(),
            version: "1.0.0".into(),
            author: "Example Corp".into(),
            description: "Format text files with various styles".into(),
        }
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![
            standalone_file_type(
                "text-file",
                "txt",
                "Text File",
                ui::IconName::FileText,
                gpui::rgb(0x9CA3AF),
                json!({"content": ""}),
            )
        ]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![
            EditorMetadata {
                id: EditorId::new("text-formatter-editor"),
                display_name: "Text Formatter".into(),
                supported_file_types: vec![FileTypeId::new("text-file")],
            }
        ]
    }

    fn create_editor(
        &self,
        _editor_id: EditorId,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        // Editor implementation...
        todo!()
    }

    // NEW: AI Tools Implementation
    fn ai_tools(&self) -> Vec<AiToolDefinition> {
        vec![
            AiToolDefinition {
                name: "format_as_markdown".to_string(),
                description: "Convert text file to well-formatted Markdown".to_string(),
                parameters_json_schema: json!({
                    "type": "object",
                    "properties": {
                        "heading_style": {
                            "type": "string",
                            "enum": ["atx", "setext"],
                            "description": "Markdown heading style (# or underline)"
                        },
                        "bullet_style": {
                            "type": "string",
                            "enum": ["dash", "asterisk", "plus"],
                            "description": "List bullet character"
                        }
                    },
                    "required": ["heading_style", "bullet_style"]
                }),
                category: Some("formatting".to_string()),
            },
            AiToolDefinition {
                name: "wrap_columns".to_string(),
                description: "Wrap text to specified column width".to_string(),
                parameters_json_schema: json!({
                    "type": "object",
                    "properties": {
                        "width": {
                            "type": "integer",
                            "minimum": 20,
                            "maximum": 200,
                            "description": "Column width in characters"
                        }
                    },
                    "required": ["width"]
                }),
                category: Some("formatting".to_string()),
            },
            AiToolDefinition {
                name: "remove_trailing_whitespace".to_string(),
                description: "Remove trailing whitespace from all lines".to_string(),
                parameters_json_schema: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                category: Some("cleanup".to_string()),
            },
            AiToolDefinition {
                name: "normalize_line_endings".to_string(),
                description: "Normalize line endings to CRLF, LF, or CR".to_string(),
                parameters_json_schema: json!({
                    "type": "object",
                    "properties": {
                        "ending": {
                            "type": "string",
                            "enum": ["lf", "crlf", "cr"],
                            "description": "Line ending style"
                        }
                    },
                    "required": ["ending"]
                }),
                category: Some("cleanup".to_string()),
            },
        ]
    }

    fn execute_ai_tool(
        &self,
        file_path: &Path,
        tool_name: &str,
        tool_args: Value,
        fs_context: &FsContext,
    ) -> Result<Value, PluginError> {
        // Read file
        let mut content = fs_context.engine_fs
            .read_file(file_path)
            .map_err(|e| PluginError::FileLoadError {
                path: file_path.to_path_buf(),
                message: e.to_string(),
            })?;

        let result = match tool_name {
            "format_as_markdown" => {
                let heading_style = tool_args["heading_style"].as_str().unwrap_or("atx");
                let bullet_style = tool_args["bullet_style"].as_str().unwrap_or("dash");
                
                content = self.convert_to_markdown(&content, heading_style, bullet_style);
                json!({"status": "formatted", "lines": content.lines().count()})
            }
            
            "wrap_columns" => {
                let width = tool_args["width"].as_u64()
                    .ok_or_else(|| PluginError::InvalidFormat {
                        expected: "integer width".into(),
                        message: "width must be a number".into(),
                    })? as usize;
                
                content = self.wrap_lines(&content, width);
                json!({"status": "wrapped", "width": width})
            }
            
            "remove_trailing_whitespace" => {
                content = content.lines()
                    .map(|line| line.trim_end())
                    .collect::<Vec<_>>()
                    .join("\n");
                json!({"status": "cleaned"})
            }
            
            "normalize_line_endings" => {
                let ending = tool_args["ending"].as_str().unwrap_or("lf");
                content = match ending {
                    "crlf" => content.replace('\n', "\r\n"),
                    "cr" => content.replace('\n', "\r"),
                    _ => content.replace("\r\n", "\n"),
                };
                json!({"status": "normalized", "ending": ending})
            }
            
            _ => return Err(PluginError::Other {
                message: format!("Unknown tool: {}", tool_name),
            })
        };

        // Write back
        fs_context.engine_fs
            .write_file(file_path, &content)
            .map_err(|e| PluginError::FileSaveError {
                path: file_path.to_path_buf(),
                message: e.to_string(),
            })?;

        Ok(result)
    }

    fn capabilities_for_file(&self, file_path: &Path) -> Vec<String> {
        if file_path.extension().map_or(false, |ext| ext == "txt") {
            vec![
                "format".into(),
                "cleanup".into(),
                "wrap".into(),
                "normalize".into(),
            ]
        } else {
            vec![]
        }
    }
}

impl TextFormatterPlugin {
    fn convert_to_markdown(&self, text: &str, heading_style: &str, bullet_style: &str) -> String {
        // Simple implementation
        text.to_string()
    }

    fn wrap_lines(&self, text: &str, width: usize) -> String {
        // Simple word-wrap implementation
        text.lines()
            .flat_map(|line| {
                if line.len() <= width {
                    vec![line.to_string()]
                } else {
                    let mut result = Vec::new();
                    let mut current_line = String::new();
                    
                    for word in line.split_whitespace() {
                        if current_line.is_empty() {
                            current_line = word.to_string();
                        } else if current_line.len() + 1 + word.len() <= width {
                            current_line.push(' ');
                            current_line.push_str(word);
                        } else {
                            result.push(current_line);
                            current_line = word.to_string();
                        }
                    }
                    
                    if !current_line.is_empty() {
                        result.push(current_line);
                    }
                    
                    result
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

export_plugin!(TextFormatterPlugin);
```

### AI Usage Example

```
User: "Fix the formatting of my README.txt - make it markdown-style with dashes for bullets"

AI Workflow:
1. Query: query_file_tools("README.txt")
   Response: {
     "available_tools": [
       {
         "name": "format_as_markdown",
         "plugin_id": "com.example.text-formatter",
         "description": "Convert text file to well-formatted Markdown"
       },
       ...
     ]
   }

2. Execute: execute_plugin_tool(
     file_path="README.txt",
     plugin_id="com.example.text-formatter",
     tool_name="format_as_markdown",
     tool_args={"heading_style": "atx", "bullet_style": "dash"}
   )
   Response: {"status": "formatted", "lines": 42}

3. Report: "Formatted README.txt as Markdown with 42 lines"
```

---

## Complex Example: Blueprint Editor Plugin

### File Type Definition with Structure

```rust
fn file_types(&self) -> Vec<FileTypeDefinition> {
    vec![
        folder_file_type(
            "blueprint-graph",
            "blueprint",
            "Blueprint Class",
            ui::IconName::NetworkNode,
            gpui::rgb(0x3B82F6),
            "graph_save.json",  // Marker file
            vec![
                PathTemplate::File {
                    path: "graph_save.json".into(),
                    content: r#"{
  "version": 1,
  "name": "NewBlueprint",
  "nodes": [],
  "connections": []
}"#.into(),
                },
                PathTemplate::Folder {
                    path: "assets".into(),
                },
            ],
            json!({"version": 1, "name": "", "nodes": [], "connections": []}),
        )
    ]
}
```

### Blueprint-Specific Tools

```rust
fn ai_tools(&self) -> Vec<AiToolDefinition> {
    vec![
        AiToolDefinition {
            name: "refactor_blueprint_nodes".to_string(),
            description: "Refactor blueprint by renaming nodes and updating connections".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "operations": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "type": {"type": "string", "enum": ["rename", "delete", "reroute"]},
                                "old_name": {"type": "string"},
                                "new_name": {"type": "string"},
                                "node_id": {"type": "string"},
                                "from_connection": {"type": "string"},
                                "to_connection": {"type": "string"}
                            }
                        }
                    }
                },
                "required": ["operations"]
            }),
            category: Some("refactoring".to_string()),
        },
        
        AiToolDefinition {
            name: "validate_blueprint".to_string(),
            description: "Validate blueprint for errors and issues".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "check_cycles": {"type": "boolean"},
                    "check_unused": {"type": "boolean"},
                    "check_invalid_connections": {"type": "boolean"}
                },
                "required": []
            }),
            category: Some("validation".to_string()),
        },
        
        AiToolDefinition {
            name: "optimize_blueprint".to_string(),
            description: "Optimize blueprint by removing dead code and consolidating nodes".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "strategy": {
                        "type": "string",
                        "enum": ["remove_unused", "consolidate_small_branches", "merge_linear_chains"],
                        "description": "Optimization strategy to apply"
                    }
                },
                "required": ["strategy"]
            }),
            category: Some("optimization".to_string()),
        },
        
        AiToolDefinition {
            name: "generate_blueprint_from_template".to_string(),
            description: "Generate new blueprint from a template pattern".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "template": {
                        "type": "string",
                        "enum": ["state_machine", "event_dispatcher", "sequential_executor"],
                    },
                    "parameters": {
                        "type": "object",
                        "description": "Template-specific parameters"
                    }
                },
                "required": ["template"]
            }),
            category: Some("generation".to_string()),
        },
        
        AiToolDefinition {
            name: "analyze_blueprint_performance".to_string(),
            description: "Analyze blueprint execution performance characteristics".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "target_framerate": {"type": "integer", "minimum": 30, "maximum": 240},
                    "budget_ms": {"type": "number", "description": "Time budget in milliseconds"}
                },
                "required": []
            }),
            category: Some("analysis".to_string()),
        },
    ]
}
```

### Tool Execution Implementation

```rust
fn execute_ai_tool(
    &self,
    file_path: &Path,
    tool_name: &str,
    tool_args: Value,
    fs_context: &FsContext,
) -> Result<Value, PluginError> {
    // Get the blueprint folder path
    let bp_path = if file_path.is_dir() {
        file_path.to_path_buf()
    } else {
        file_path.parent().unwrap().to_path_buf()
    };

    // Read blueprint data
    let graph_file = bp_path.join("graph_save.json");
    let content = fs_context.engine_fs
        .read_file(&graph_file)
        .map_err(|e| PluginError::FileLoadError {
            path: graph_file.clone(),
            message: e.to_string(),
        })?;

    let mut blueprint: BlueprintGraph = serde_json::from_str(&content)
        .map_err(|e| PluginError::InvalidFormat {
            expected: "valid blueprint JSON".into(),
            message: e.to_string(),
        })?;

    let result = match tool_name {
        "refactor_blueprint_nodes" => {
            let operations = tool_args["operations"].as_array()
                .ok_or_else(|| PluginError::InvalidFormat {
                    expected: "array".into(),
                    message: "operations must be an array".into(),
                })?;

            for op in operations {
                match op["type"].as_str().unwrap_or("") {
                    "rename" => {
                        let old = op["old_name"].as_str().unwrap();
                        let new = op["new_name"].as_str().unwrap();
                        blueprint.rename_node(old, new)?;
                    }
                    "delete" => {
                        let id = op["node_id"].as_str().unwrap();
                        blueprint.delete_node(id)?;
                    }
                    _ => {}
                }
            }

            // Save back
            let updated = serde_json::to_string_pretty(&blueprint)?;
            fs_context.engine_fs.write_file(&graph_file, &updated)?;

            json!({
                "status": "refactored",
                "node_count": blueprint.nodes.len(),
                "connection_count": blueprint.connections.len()
            })
        }

        "validate_blueprint" => {
            let check_cycles = tool_args["check_cycles"].as_bool().unwrap_or(true);
            let check_unused = tool_args["check_unused"].as_bool().unwrap_or(true);
            let check_invalid = tool_args["check_invalid_connections"].as_bool().unwrap_or(true);

            let mut issues = Vec::new();

            if check_cycles {
                if let Some(cycle) = blueprint.detect_cycle() {
                    issues.push(format!("Cycle detected: {}", cycle));
                }
            }

            if check_unused {
                for node in &blueprint.nodes {
                    if blueprint.is_node_unused(&node.id) {
                        issues.push(format!("Unused node: {}", node.id));
                    }
                }
            }

            if check_invalid {
                for conn in &blueprint.connections {
                    if !blueprint.is_connection_valid(conn) {
                        issues.push(format!("Invalid connection: {:?}", conn));
                    }
                }
            }

            json!({
                "status": "validated",
                "valid": issues.is_empty(),
                "issue_count": issues.len(),
                "issues": issues
            })
        }

        "optimize_blueprint" => {
            let strategy = tool_args["strategy"].as_str().unwrap_or("remove_unused");
            let changes = match strategy {
                "remove_unused" => blueprint.remove_unused_nodes(),
                "consolidate_small_branches" => blueprint.consolidate_branches(),
                "merge_linear_chains" => blueprint.merge_linear_chains(),
                _ => 0,
            };

            let updated = serde_json::to_string_pretty(&blueprint)?;
            fs_context.engine_fs.write_file(&graph_file, &updated)?;

            json!({
                "status": "optimized",
                "strategy": strategy,
                "changes_made": changes,
                "node_count": blueprint.nodes.len()
            })
        }

        _ => return Err(PluginError::Other {
            message: format!("Unknown tool: {}", tool_name),
        })
    };

    Ok(result)
}

fn capabilities_for_file(&self, file_path: &Path) -> Vec<String> {
    if self.is_blueprint_folder(file_path) {
        vec![
            "refactor".into(),
            "validate".into(),
            "optimize".into(),
            "analyze".into(),
            "generate".into(),
        ]
    } else {
        vec![]
    }
}
```

---

## Cross-Plugin Workflow Example

### Scenario: AI Optimizes Multiple File Types

```
User: "Optimize the entire project - fix code, shaders, and blueprints"

AI Workflow:

Phase 1: Discovery
==================
1. Query available file types
2. Search workspace for all files
3. Group by type:
   - .rs files (handled by rust-analyzer plugin)
   - .shader files (handled by shader-compiler plugin)
   - .blueprint folders (handled by blueprint-editor plugin)
   - .json files (handled by json-validator plugin)

Phase 2: Planning
=================
For each file, query available tools
- Rust files:    ["format", "lint", "suggest_optimizations"]
- Shaders:       ["compile_check", "optimize_performance", "validate"]
- Blueprints:    ["optimize_blueprint", "validate_blueprint"]
- JSON files:    ["validate", "format", "minify"]

Create execution plan prioritizing:
1. Validation (catch errors first)
2. Optimization
3. Formatting (cosmetic changes last)

Phase 3: Execution
==================
For Rust files:
  execute_plugin_tool("main.rs", "rust-analyzer", "lint")
  → Returns errors to fix
  
  execute_plugin_tool("main.rs", "rust-analyzer", "suggest_optimizations")
  → Returns optimization suggestions applied

For Shaders:
  execute_plugin_tool("main.shader", "shader-compiler", "compile_check")
  → Validates syntax
  
  execute_plugin_tool("main.shader", "shader-compiler", "optimize_performance")
  → Applies shader optimizations

For Blueprints:
  execute_plugin_tool("GameLogic.blueprint", "blueprint-editor", "validate_blueprint")
  → Reports issues
  
  execute_plugin_tool("GameLogic.blueprint", "blueprint-editor", "optimize_blueprint", 
                      {strategy: "remove_unused"})
  → Cleans up unused nodes

For JSON:
  execute_plugin_tool("config.json", "json-validator", "validate")
  → Ensures valid JSON

Phase 4: Reporting
==================
Summary:
  ✓ Fixed 3 linting errors in Rust
  ✓ Optimized 2 shader programs
  ✓ Cleaned up blueprint (removed 5 unused nodes)
  ✓ Validated config JSON
  
  Total execution time: 2.3s
  All files optimized successfully
```

---

## Best Practices

### 1. **Keep Tools Focused**

**Good:**
```rust
AiToolDefinition {
    name: "format_json".to_string(),
    description: "Format JSON file with specified indentation".to_string(),
    parameters_json_schema: json!({
        "indent": {"type": "integer"}
    }),
    category: Some("formatting".to_string()),
}
```

**Bad:**
```rust
AiToolDefinition {
    name: "do_everything".to_string(),
    description: "Do formatting, validation, refactoring, and more...".to_string(),
    parameters_json_schema: json!({
        "operations": {...}  // Too many complex nested parameters
    }),
}
```

### 2. **Provide Clear Descriptions**

```rust
// Good: Actionable, specific
"Rename all occurrences of a node in the blueprint, updating all connected edges"

// Bad: Vague
"Process blueprint"
```

### 3. **Use Categories for Organization**

```rust
pub enum ToolCategory {
    Formatting,
    Validation,
    Refactoring,
    Optimization,
    CodeGeneration,
    Analysis,
}

// In tool definition:
category: Some("refactoring".to_string())
```

### 4. **Validate Arguments Thoroughly**

```rust
fn execute_ai_tool(...) -> Result<Value, PluginError> {
    // Check for required fields
    let width = tool_args["width"].as_u64()
        .ok_or_else(|| PluginError::InvalidFormat {
            expected: "integer width".into(),
            message: "width must be a positive integer".into(),
        })?;
    
    // Check ranges
    if width < 20 || width > 200 {
        return Err(PluginError::InvalidFormat {
            expected: "width between 20-200".into(),
            message: format!("got {}", width),
        });
    }
    
    // Proceed...
}
```

### 5. **Return Informative Results**

```rust
// Good: Clear status and metrics
Ok(json!({
    "status": "optimized",
    "lines_removed": 42,
    "dead_code_eliminated": 5,
    "final_size_bytes": 8192,
    "time_ms": 245
}))

// Bad: Unclear
Ok(json!(true))
```

### 6. **Handle Errors Gracefully**

```rust
fn execute_ai_tool(...) -> Result<Value, PluginError> {
    match self.perform_operation(&file_path) {
        Ok(result) => Ok(json!({"status": "success", "result": result})),
        Err(e) => {
            // Return detailed error, not just generic message
            Ok(json!({
                "status": "error",
                "error_type": "validation_failed",
                "message": e.to_string(),
                "suggestions": [
                    "Check file format",
                    "Ensure all required fields are present"
                ]
            }))
            // OR return Err for truly exceptional cases
            // Err(PluginError::Other { message: e.to_string() })
        }
    }
}
```

### 7. **Avoid Long-Running Operations**

```rust
// Bad: Might timeout
execute_ai_tool(.., "generate_entire_game_ai", ..)  // Takes 5 minutes

// Good: Break into steps
execute_ai_tool(.., "generate_ai_module_1", ..)  // 30 seconds
execute_ai_tool(.., "generate_ai_module_2", ..)  // 30 seconds
execute_ai_tool(.., "connect_ai_modules", ..)    // 10 seconds
```

### 8. **Document Parameter Ranges**

```rust
parameters_json_schema: json!({
    "type": "object",
    "properties": {
        "thread_count": {
            "type": "integer",
            "minimum": 1,
            "maximum": 16,
            "description": "Number of worker threads (1-16, default: CPU count)"
        },
        "timeout_seconds": {
            "type": "number",
            "minimum": 0.1,
            "maximum": 300,
            "description": "Operation timeout in seconds"
        }
    }
})
```

---

## Common Patterns

### Pattern 1: Batch Operations

```rust
AiToolDefinition {
    name: "batch_format".to_string(),
    description: "Format multiple files at once".to_string(),
    parameters_json_schema: json!({
        "files": {
            "type": "array",
            "items": {"type": "string"},
            "description": "Paths to files to format"
        },
        "options": {"type": "object"}
    }),
}

fn execute_ai_tool(...) -> Result<Value, PluginError> {
    let files = tool_args["files"].as_array().unwrap();
    let mut results = Vec::new();
    
    for file_path in files {
        match self.format_file(file_path) {
            Ok(result) => results.push(json!({
                "file": file_path,
                "status": "success",
                "result": result
            })),
            Err(e) => results.push(json!({
                "file": file_path,
                "status": "error",
                "error": e.to_string()
            })),
        }
    }
    
    Ok(json!({
        "batch_size": results.len(),
        "results": results
    }))
}
```

### Pattern 2: Progressive Enhancement

```rust
fn ai_tools(&self) -> Vec<AiToolDefinition> {
    vec![
        // Basic operation (always available)
        AiToolDefinition {
            name: "validate".to_string(),
            description: "Basic validation".to_string(),
            ..
        },
        
        // Advanced operation (requires configuration)
        AiToolDefinition {
            name: "advanced_validate".to_string(),
            description: "Advanced validation with deep analysis".to_string(),
            parameters_json_schema: json!({
                "depth": {"type": "string", "enum": ["basic", "deep", "exhaustive"]}
            }),
            ..
        },
        
        // Experimental (might be unstable)
        AiToolDefinition {
            name: "experimental_refactor".to_string(),
            description: "[EXPERIMENTAL] Refactor using new algorithm".to_string(),
            ..
        },
    ]
}
```

### Pattern 3: Conditional Availability

```rust
fn ai_tools(&self) -> Vec<AiToolDefinition> {
    let mut tools = vec![/* standard tools */];
    
    // Add optimized tools only if requested
    if std::env::var("ENABLE_ADVANCED_TOOLS").is_ok() {
        tools.push(AiToolDefinition {
            name: "advanced_optimization".to_string(),
            description: "Advanced optimization (may be slower)".to_string(),
            ..
        });
    }
    
    tools
}
```

---

## Troubleshooting

### Issue 1: Tool Not Appearing in AI

**Symptoms**: Tool is defined but AI doesn't see it

**Debugging**:
```rust
// 1. Check plugin loads without errors
tracing::debug!("Plugin loaded: {}", metadata.name);

// 2. Verify ai_tools() returns non-empty vector
let tools = self.ai_tools();
tracing::debug!("Exporting {} tools", tools.len());

// 3. Check tool registry was updated
// (AI should call query_available_file_types first)
```

**Solutions**:
- Ensure `ai_tools()` is implemented (not using default)
- Verify plugin version matches engine version
- Check file type is registered
- Ensure editor is registered for file type

### Issue 2: Tool Execution Fails

**Symptoms**: Tool call returns error JSON

**Debugging**:
```rust
fn execute_ai_tool(...) {
    tracing::error!("Executing tool: {}", tool_name);
    tracing::error!("Args: {}", serde_json::to_string_pretty(&tool_args)?);
    
    match self.actual_operation() {
        Ok(result) => {
            tracing::error!("Success: {}", serde_json::to_string_pretty(&result)?);
            Ok(result)
        }
        Err(e) => {
            tracing::error!("Error: {}", e);
            // ... return detailed error
        }
    }
}
```

**Solutions**:
- Validate all arguments first
- Check file exists and is readable
- Provide detailed error messages
- Log intermediate steps

### Issue 3: Performance Issues

**Symptoms**: Tool takes too long to execute

**Solutions**:
- Cache parsed data across tool calls
- Break large operations into smaller tools
- Provide progress indicators (future enhancement)
- Optimize file I/O operations

### Issue 4: File Locks / Concurrency

**Symptoms**: Multiple tools try to modify same file

**Solutions**:
- Use engine_fs which handles locking
- Implement atomic operations
- Document concurrency expectations
- Provide tool serialization if needed

---

## Migration Guide: Adding AI Tools to Existing Plugin

### Step 1: Add Tool Definitions

```rust
impl EditorPlugin for MyExistingPlugin {
    // ... existing implementation ...
    
    // ADD THIS:
    fn ai_tools(&self) -> Vec<AiToolDefinition> {
        vec![
            AiToolDefinition {
                name: "my_first_tool".to_string(),
                description: "Do something useful".to_string(),
                parameters_json_schema: json!({/* ... */}),
                category: None,
            }
        ]
    }
}
```

### Step 2: Implement Tool Execution

```rust
impl EditorPlugin for MyExistingPlugin {
    // ... existing implementation ...
    
    // ADD THIS:
    fn execute_ai_tool(
        &self,
        file_path: &Path,
        tool_name: &str,
        tool_args: serde_json::Value,
        fs_context: &FsContext,
    ) -> Result<serde_json::Value, PluginError> {
        match tool_name {
            "my_first_tool" => {
                // Implementation...
                Ok(json!({"status": "done"}))
            }
            _ => Err(PluginError::Other {
                message: format!("Unknown tool: {}", tool_name),
            })
        }
    }
}
```

### Step 3: Test with AI

Use `query_file_tools()` to verify tools appear, then `execute_plugin_tool()` to test.

---

## References & Further Reading

- [AI Plugins Integration Plan](./AI_PLUGINS_INTEGRATION_PLAN.md)
- [Plugin Development Guide](./PLUGIN_DEVELOPMENT.md)
- [EditorPlugin Trait](../crates/plugin_editor_api/src/lib.rs)
- [Agent Chat Tools](../agent-providers/agent_chat_tools/src/lib.rs)
