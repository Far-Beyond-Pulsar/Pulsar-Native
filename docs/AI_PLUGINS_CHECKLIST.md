# AI Plugin Tools - Quick Implementation Checklist

## Overview

This document provides a concise checklist for implementing the AI plugin tools system. Use alongside:
- [AI_PLUGINS_INTEGRATION_PLAN.md](./AI_PLUGINS_INTEGRATION_PLAN.md) - Full architecture
- [AI_PLUGINS_PRACTICAL_EXAMPLES.md](./AI_PLUGINS_PRACTICAL_EXAMPLES.md) - Code examples

---

## Phase 1: Extend Plugin Editor API

### crates/plugin_editor_api/src/lib.rs

- [ ] Add `AiToolDefinition` struct
  ```rust
  pub struct AiToolDefinition {
      pub name: String,
      pub description: String,
      pub parameters_json_schema: serde_json::Value,
      pub category: Option<String>,
  }
  ```

- [ ] Add `FsContext` struct
  ```rust
  pub struct FsContext {
      pub project_root: PathBuf,
      pub plugin_manager: Arc<RwLock<PluginManager>>,
      pub engine_fs: Arc<EngineFs>,
  }
  ```

- [ ] Extend `EditorPlugin` trait
  ```rust
  fn ai_tools(&self) -> Vec<AiToolDefinition> { Vec::new() }
  
  fn execute_ai_tool(
      &self,
      file_path: &Path,
      tool_name: &str,
      tool_args: serde_json::Value,
      fs_context: &FsContext,
  ) -> Result<serde_json::Value, PluginError> { Err(...) }
  
  fn capabilities_for_file(&self, file_path: &Path) -> Vec<String> { Vec::new() }
  ```

- [ ] Update `PluginManager` to support tool queries and execution
  ```rust
  pub fn get_plugin_ai_tools(&self, plugin_id: &PluginId) -> Result<Vec<AiToolDefinition>>
  pub fn execute_plugin_tool(&self, ...) -> Result<serde_json::Value>
  ```

- [ ] Update `export_plugin!` macro if needed for new trait methods

- [ ] Bump version in Cargo.toml (plugins must match engine version)

### Test
```bash
cd crates/plugin_editor_api
cargo test
cargo doc --open
```

---

## Phase 2: Create Plugin Tool Bridge

### crates/plugin_manager/src/plugin_tool_bridge.rs (NEW)

- [ ] Create `PluginToolBridge` struct
  ```rust
  pub struct PluginToolBridge {
      plugin_tools: HashMap<PluginId, Vec<(FileTypeId, Vec<AiToolDefinition>)>>,
      tool_lookup: HashMap<(FileTypeId, String), (PluginId, String)>,
      plugin_manager: Arc<RwLock<PluginManager>>,
  }
  ```

- [ ] Implement core methods
  ```rust
  pub fn new(plugin_manager: Arc<RwLock<PluginManager>>) -> Self
  pub fn available_file_types(&self) -> Vec<FileTypeInfo>
  pub fn editors_for_file_type(&self, file_type_id: &FileTypeId) -> Vec<EditorInfo>
  pub fn tools_for_file(&self, file_path: &Path) -> Vec<AvailableTool>
  pub fn execute_tool(&self, file_path: &Path, plugin_id: &PluginId, ...) -> Result<Value>
  pub fn refresh_all_tools(&mut self)
  ```

- [ ] Add helper types
  ```rust
  pub struct FileTypeInfo { /* ... */ }
  pub struct EditorInfo { /* ... */ }
  pub struct AvailableTool { /* ... */ }
  ```

- [ ] Update crates/plugin_manager/src/lib.rs to export bridge
  ```rust
  pub use plugin_tool_bridge::PluginToolBridge;
  ```

- [ ] Create global bridge instance
  ```rust
  static GLOBAL_PLUGIN_TOOL_BRIDGE: OnceCell<RwLock<PluginToolBridge>> = OnceCell::new();
  
  pub fn initialize_global_tool_bridge(bridge: PluginToolBridge)
  pub fn global_tool_bridge() -> Option<&'static RwLock<PluginToolBridge>>
  ```

### Test
```bash
cd crates/plugin_manager
cargo test plugin_tool_bridge
```

---

## Phase 3: Extend Tool Registry

### agent-providers/agent_chat_tools/src/lib.rs

- [ ] Extend `ToolContext`
  ```rust
  pub struct ToolContext {
      pub workspace_root: PathBuf,
      pub plugin_bridge: Option<Arc<RwLock<PluginToolBridge>>>,
  }
  ```

- [ ] Add discovery tools as `ChatTool` implementations
  ```rust
  struct QueryAvailableFileTypesTool;
  struct QueryFileToolsTool;
  struct ExecutePluginToolTool;
  ```

- [ ] Extend `ToolRegistry`
  ```rust
  pub struct ToolRegistry {
      tools: HashMap<String, Arc<dyn ChatTool>>,
      plugin_bridge: Option<Arc<RwLock<PluginToolBridge>>>,
  }
  
  pub fn with_plugins(bridge: Arc<RwLock<PluginToolBridge>>) -> Self
  ```

- [ ] Update `available_tools_schema()`
  - Include discovery tools
  - Include execution tool
  - Maintain backward compatibility with existing tools

### Tool Signatures

```rust
// Query available file types
pub fn query_available_file_types() -> FileTypeInfo[]

// Query tools for a specific file
pub fn query_file_tools(file_path: String) -> AvailableTool[]

// Execute a plugin tool
pub fn execute_plugin_tool(
    file_path: String,
    plugin_id: String,
    tool_name: String,
    tool_args: Object
) -> Object
```

### Test
```bash
cd agent-providers/agent_chat_tools
cargo test
```

---

## Phase 4: Integrate with Agent Chat

### ui-crates/ui_core/src/app/agent_chat_panel/mod.rs

- [ ] Import bridge in `new()` method
  ```rust
  use plugin_manager::PluginToolBridge;
  ```

- [ ] Create plugin tool bridge during initialization
  ```rust
  let plugin_bridge = if let Some(pm_lock) = plugin_manager::global() {
      if let Ok(pm) = pm_lock.read() {
          Some(Arc::new(RwLock::new(
              PluginToolBridge::new(pm_lock.clone())
          )))
      } else { None }
  } else { None };
  ```

- [ ] Pass bridge to ToolRegistry
  ```rust
  let tool_registry = if let Some(bridge) = plugin_bridge.clone() {
      ToolRegistry::with_plugins(bridge)
  } else {
      ToolRegistry::with_default_tools()
  };
  ```

- [ ] Store bridge in panel state (for refresh/updates)
  ```rust
  pub struct AgentChatPanel {
      // ... existing fields ...
      plugin_bridge: Option<Arc<RwLock<PluginToolBridge>>>,
  }
  ```

### Test
- [ ] Start chat panel without errors
- [ ] Verify tools appear in dropdown/schema
- [ ] Query file types works
- [ ] Execute generic tools still works

---

## Phase 5: Built-in Editor Tools

### For Each Built-in Editor

- [ ] Implement `ai_tools()` method
  ```rust
  impl EditorPlugin for MyBuiltinEditor {
      fn ai_tools(&self) -> Vec<AiToolDefinition> {
          vec![
              AiToolDefinition { /* ... */ }
          ]
      }
  }
  ```

- [ ] Implement `execute_ai_tool()` method
  ```rust
  fn execute_ai_tool(
      &self,
      file_path: &Path,
      tool_name: &str,
      tool_args: Value,
      fs_context: &FsContext,
  ) -> Result<Value, PluginError> {
      match tool_name {
          "my_tool" => { /* implementation */ }
          _ => Err(PluginError::Other { message: "Unknown tool" })
      }
  }
  ```

- [ ] Optionally implement `capabilities_for_file()`

### Examples to Implement
- [ ] Text/JSON Editor: find_replace, format, validate
- [ ] Code Editor: lint, suggest_optimizations, refactor
- [ ] Image Editor: resize, compress, convert_format
- [ ] Config Editor: validate, upgrade, diff

---

## Phase 6: Documentation

### PLUGIN_DEVELOPMENT.md

- [ ] Add "AI Tools" section
  - Overview of tool system
  - When to implement tools
  - Best practices

- [ ] Add example: Simple tool plugin
  ```rust
  // Code showing format_file tool
  ```

- [ ] Add example: Complex tool plugin
  ```rust
  // Code showing multi-tool plugin
  ```

- [ ] Add migration guide for existing plugins
  - Steps to add tools to existing plugin
  - Backward compatibility notes

### Create AI_TOOLS_BEST_PRACTICES.md

- [ ] Clear descriptions matter
- [ ] Validate all arguments
- [ ] Return informative results
- [ ] Handle errors gracefully
- [ ] Keep tools focused
- [ ] Avoid long-running operations

### API Reference

- [ ] Document `AiToolDefinition`
  - All fields
  - JSON schema requirements
  - Category guidelines

- [ ] Document `FsContext`
  - Available operations
  - Permissions/limitations
  - Error handling

- [ ] Document tool execution flow
  - How AI discovers tools
  - How tools are called
  - Result format

---

## Phase 7: Testing

### Unit Tests

- [ ] Plugin tool bridge
  ```bash
  cargo test -p plugin_manager plugin_tool_bridge
  ```

- [ ] Tool registry
  ```bash
  cargo test -p agent_chat_tools
  ```

- [ ] Built-in editor tools
  ```bash
  cargo test -p ui_core -- ai_tools
  ```

### Integration Tests

- [ ] Plugin loads and registers tools
- [ ] Tools appear in schema
- [ ] Tool execution works end-to-end
- [ ] Multiple plugins work together
- [ ] Error handling works

### Chat Tests

- [ ] Query available file types returns results
- [ ] Query file tools returns plugins' tools
- [ ] Execute plugin tool works
- [ ] Tool results propagate back to AI
- [ ] Multi-tool workflow completes successfully

### Performance Tests

- [ ] 100+ plugins load quickly
- [ ] Tool discovery O(1) lookup
- [ ] No memory leaks in tool execution

---

## Verification Checklist

### Before Merging

- [ ] All new code compiles without warnings
- [ ] All tests pass (unit + integration + chat)
- [ ] Plugin API version bumped (if breaking changes)
- [ ] Documentation updated
- [ ] Example plugin created
- [ ] Backward compatibility verified
- [ ] Performance benchmarks show no degradation
- [ ] Error messages are helpful
- [ ] Memory leaks checked (valgrind/miri)
- [ ] Cross-plugin interactions work correctly

### Post-Merge

- [ ] Built-in editors updated with tools
- [ ] Example plugins migrated
- [ ] Release notes updated
- [ ] Migration guide publicized
- [ ] Community feedback gathered

---

## File Modification Summary

### New Files
- `crates/plugin_manager/src/plugin_tool_bridge.rs`
- `docs/AI_PLUGINS_INTEGRATION_PLAN.md`
- `docs/AI_PLUGINS_PRACTICAL_EXAMPLES.md`
- `docs/AI_TOOLS_BEST_PRACTICES.md` (optional)
- Example plugin (optional)

### Modified Files
- `crates/plugin_editor_api/src/lib.rs`
- `crates/plugin_editor_api/Cargo.toml`
- `crates/plugin_manager/src/lib.rs`
- `crates/plugin_manager/src/registry.rs`
- `agent-providers/agent_chat_tools/src/lib.rs`
- `ui-crates/ui_core/src/app/agent_chat_panel/mod.rs`
- `ui-crates/ui_core/src/app/constructors.rs`
- `docs/PLUGIN_DEVELOPMENT.md`

---

## Implementation Order (Recommended)

1. **Week 1**: Phase 1 (Plugin API extension)
   - Extend EditorPlugin trait
   - Add new types
   - Test locally

2. **Week 2**: Phase 2 (Plugin tool bridge)
   - Create PluginToolBridge
   - Implement discovery/execution
   - Unit tests

3. **Week 3**: Phase 3 (Tool registry)
   - Add discovery tools
   - Extend ToolRegistry
   - Integration tests

4. **Week 4**: Phase 4 (Chat integration)
   - Initialize bridge in AgentChatPanel
   - End-to-end tests
   - Basic chat tests

5. **Week 5-6**: Phase 5 (Built-in editors)
   - Add tools to each editor
   - Real-world testing
   - Performance optimization

6. **Week 7**: Phase 6-7 (Documentation + Testing)
   - Complete documentation
   - Final tests
   - Example plugins

---

## Rollback Plan

If major issues discovered:

1. **Disable plugin tools** (environment variable)
   ```rust
   if std::env::var("ENABLE_PLUGIN_TOOLS").is_err() {
       tool_registry = ToolRegistry::with_default_tools();
   }
   ```

2. **Keep old API available** (no breaking changes to existing traits)

3. **Feature flag for gradual rollout**
   ```toml
   [features]
   plugin-ai-tools = []
   ```

---

## Key Points to Remember

1. **Backward Compatibility**: All changes are additions, no breaking changes required
2. **Plugin Version Check**: Plugins must match engine version (already enforced)
3. **Safe FFI**: Permanent library loading ensures safety
4. **Tool Discovery**: Lazy initialization, cached for performance
5. **Error Handling**: All errors returned as JSON to AI, not exceptions
6. **Modularity**: Each tool should do one thing well
7. **Scalability**: Handles 100+ plugins without performance issues

---

## Contact & Questions

For clarification on any part:
- Review [AI_PLUGINS_INTEGRATION_PLAN.md](./AI_PLUGINS_INTEGRATION_PLAN.md)
- Check [AI_PLUGINS_PRACTICAL_EXAMPLES.md](./AI_PLUGINS_PRACTICAL_EXAMPLES.md)
- Look at reference implementation in codebase

---

## Appendix: Copy-Paste Templates

### Empty AI Tool Implementation
```rust
fn ai_tools(&self) -> Vec<AiToolDefinition> {
    vec![
        AiToolDefinition {
            name: "my_tool".to_string(),
            description: "TODO: Add description".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            category: None,
        }
    ]
}

fn execute_ai_tool(
    &self,
    file_path: &Path,
    tool_name: &str,
    tool_args: Value,
    fs_context: &FsContext,
) -> Result<Value, PluginError> {
    match tool_name {
        "my_tool" => {
            // TODO: Implement
            Ok(json!({"status": "done"}))
        }
        _ => Err(PluginError::Other {
            message: format!("Unknown tool: {}", tool_name),
        })
    }
}
```

### Tool with Parameters Template
```rust
AiToolDefinition {
    name: "tool_name".to_string(),
    description: "What this tool does".to_string(),
    parameters_json_schema: json!({
        "type": "object",
        "properties": {
            "param1": {
                "type": "string",
                "description": "Parameter description"
            },
            "param2": {
                "type": "integer",
                "minimum": 1,
                "maximum": 100
            }
        },
        "required": ["param1"]
    }),
    category: Some("category_name".to_string()),
}
```

---

**Document Version**: 1.0  
**Last Updated**: May 8, 2026  
**Status**: Ready for implementation
