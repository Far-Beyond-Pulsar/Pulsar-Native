# AI Plugin Tools Integration Plan

## Executive Summary

This plan describes how to enable plugins to provide tools to AI agents, allowing the AI to intelligently choose which files to edit and use plugin-specific editing capabilities via tool calls. The system must handle potentially hundreds of plugins efficiently while maintaining clean separation between the plugin manager, tool registry, and agent chat system.

**Core Principle**: AI agents will use dynamic tool discovery to ask "what can I do with this file?" and "what files are available?" rather than relying on a static pre-compiled tool list.

---

## Problem Statement

Currently:
1. **Plugins exist in isolation** - AI agents don't know what capabilities plugins provide
2. **Tools are static** - AI sees only generic file operations (read, list, search)
3. **No plugin-AI bridge** - when AI wants to edit a file, it uses generic tools, not plugin-specific ones
4. **AI can't discover capabilities** - no way to ask "what editors handle .myformat files?"
5. **Scalability concern** - with many plugins, static tool lists become unmaintainable

Goal:
- Let AI agents dynamically discover file types, available editors, and plugin-specific tools
- Enable AI to make informed decisions about which files to edit first
- Allow plugins to provide custom editing tools (not just generic read/write)
- Support potentially thousands of plugins without performance degradation

---

## Architecture Overview

### Three-Layer Tool System

```
┌─────────────────────────────────────────────────────────────┐
│              Agent Chat Panel / AI Provider                 │
│                                                              │
│  - Maintains ToolRegistry & PluginToolRegistry             │
│  - Passes tools to AI via tool_definitions                 │
│  - Handles tool execution callbacks                         │
└─────────────────────────────────────────────────────────────┘
                           ▲
                           │
                    Tool Discovery & Execution
                           │
┌─────────────────────────────────────────────────────────────┐
│          Plugin Tool Bridge (NEW)                            │
│                                                              │
│  - Query available file types & editors                    │
│  - Register/unregister plugin tools                        │
│  - Map file paths to plugins & available tools             │
│  - Execute plugin-specific tool calls                      │
└─────────────────────────────────────────────────────────────┘
                           ▲
                           │
                    Query & Execution
                           │
┌─────────────────────────────────────────────────────────────┐
│           Plugin Manager (ENHANCED)                         │
│                                                              │
│  - EditorPlugin trait extended with tool support           │
│  - File type registry (unchanged)                          │
│  - Editor registry (unchanged)                             │
│  - NEW: Tool provider interface                            │
│  - NEW: Context/capability export                          │
└─────────────────────────────────────────────────────────────┘
                           ▲
                           │
                    Trait Implementation
                           │
┌─────────────────────────────────────────────────────────────┐
│                   Plugins (DLLs)                             │
│                                                              │
│  - Implement tool methods                                  │
│  - Execute actual file editing/operations                  │
│  - Access engine filesystem via context                    │
└─────────────────────────────────────────────────────────────┘
```

---

## Design Phase 1: Extend EditorPlugin Trait

### New Methods on EditorPlugin

```rust
pub trait EditorPlugin: Send + Sync {
    // Existing methods...
    
    /// Declare what tools this plugin provides for AI agents
    /// 
    /// Called once at plugin load time
    /// Returns JSON schemas for tools the AI can call
    fn ai_tools(&self) -> Vec<AiToolDefinition> {
        Vec::new()  // Optional - default no custom tools
    }
    
    /// Execute an AI tool call for a specific file
    /// 
    /// Args:
    /// - file_path: Path to the file being edited
    /// - tool_name: Name of the tool to execute
    /// - tool_args: Tool arguments as JSON
    /// - fs_context: Access to engine filesystem for operations
    /// 
    /// Returns result JSON that gets sent back to AI
    fn execute_ai_tool(
        &self,
        file_path: &Path,
        tool_name: &str,
        tool_args: serde_json::Value,
        fs_context: &FsContext,
    ) -> Result<serde_json::Value, PluginError> {
        Err(PluginError::Other {
            message: format!("Tool '{}' not implemented", tool_name),
        })
    }
    
    /// Query what capabilities this plugin has for a given file
    /// 
    /// Called by AI when deciding how to edit a file
    /// Allows plugin to declare "I can refactor this", "I can format this", etc.
    fn capabilities_for_file(&self, file_path: &Path) -> Vec<String> {
        Vec::new()  // Optional - default no special capabilities
    }
}

/// Describes a single tool provided by a plugin to AI
#[derive(Clone, Debug)]
pub struct AiToolDefinition {
    /// Unique tool name (scoped to plugin internally)
    pub name: String,
    
    /// Human description of what the tool does
    pub description: String,
    
    /// JSON Schema for the tool's input parameters
    pub parameters_json_schema: serde_json::Value,
    
    /// Category for grouping related tools (e.g., "refactoring", "formatting")
    pub category: Option<String>,
}

/// Context passed to plugins for file operations
pub struct FsContext {
    /// Current project root
    pub project_root: PathBuf,
    
    /// Access to plugin manager for cross-plugin queries
    pub plugin_manager: Arc<RwLock<PluginManager>>,
    
    /// Access to engine filesystem
    pub engine_fs: Arc<EngineFs>,
}
```

### Example Plugin Implementation

```rust
impl EditorPlugin for MyPlugin {
    fn ai_tools(&self) -> Vec<AiToolDefinition> {
        vec![
            AiToolDefinition {
                name: "refactor_blueprint".to_string(),
                description: "Refactor blueprint by renaming nodes".to_string(),
                parameters_json_schema: json!({
                    "type": "object",
                    "properties": {
                        "old_name": { "type": "string" },
                        "new_name": { "type": "string" }
                    },
                    "required": ["old_name", "new_name"]
                }),
                category: Some("refactoring".to_string()),
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
            "refactor_blueprint" => {
                let old_name = tool_args["old_name"].as_str().ok_or(...)?;
                let new_name = tool_args["new_name"].as_str().ok_or(...)?;
                
                // Load file from engine FS
                let content = fs_context.engine_fs.read_file(file_path)?;
                
                // Perform refactoring
                let mut graph: BlueprintGraph = serde_json::from_str(&content)?;
                graph.rename_node(old_name, new_name)?;
                
                // Save back
                let updated = serde_json::to_string_pretty(&graph)?;
                fs_context.engine_fs.write_file(file_path, &updated)?;
                
                Ok(json!({
                    "success": true,
                    "message": format!("Renamed '{}' to '{}'", old_name, new_name)
                }))
            }
            _ => Err(PluginError::Other {
                message: format!("Unknown tool: {}", tool_name),
            })
        }
    }
    
    fn capabilities_for_file(&self, file_path: &Path) -> Vec<String> {
        if file_path.extension().map_or(false, |ext| ext == "blueprint") {
            vec!["refactor".into(), "validate".into(), "analyze".into()]
        } else {
            vec![]
        }
    }
}
```

---

## Design Phase 2: Plugin Tool Registry

### New Module: PluginToolBridge

```rust
// Location: crates/plugin_manager/src/plugin_tool_bridge.rs

/// Bridge between plugin system and AI tool system
/// Manages discovery and execution of plugin-provided tools
pub struct PluginToolBridge {
    /// Cached tool definitions from all plugins
    /// Structure: plugin_id -> (file_type_id -> [tool_defs])
    plugin_tools: HashMap<PluginId, Vec<(FileTypeId, Vec<AiToolDefinition>)>>,
    
    /// Maps (file_type_id, tool_name) -> (plugin_id, original_tool_name)
    /// Enables AI to reference tools by their exposed names
    tool_lookup: HashMap<(FileTypeId, String), (PluginId, String)>,
    
    /// Reference to plugin manager
    plugin_manager: Arc<RwLock<PluginManager>>,
}

impl PluginToolBridge {
    pub fn new(plugin_manager: Arc<RwLock<PluginManager>>) -> Self {
        let mut bridge = Self {
            plugin_tools: HashMap::new(),
            tool_lookup: HashMap::new(),
            plugin_manager,
        };
        bridge.refresh_all_tools();
        bridge
    }
    
    /// Query all available file types
    pub fn available_file_types(&self) -> Vec<FileTypeInfo> {
        let pm = self.plugin_manager.read().unwrap();
        pm.file_type_registry()
            .get_all_file_types()
            .into_iter()
            .map(|ft| FileTypeInfo {
                id: ft.id.clone(),
                name: ft.display_name.clone(),
                extension: ft.extension.clone(),
                icon: ft.icon,
            })
            .collect()
    }
    
    /// Query what editors handle a specific file type
    pub fn editors_for_file_type(&self, file_type_id: &FileTypeId) 
        -> Vec<EditorInfo> 
    {
        let pm = self.plugin_manager.read().unwrap();
        pm.editor_registry()
            .get_editors_for_file_type(file_type_id)
            .into_iter()
            .map(|editor| EditorInfo {
                id: editor.id.clone(),
                name: editor.display_name.clone(),
            })
            .collect()
    }
    
    /// Query what tools are available for a specific file
    pub fn tools_for_file(&self, file_path: &Path) -> Vec<AvailableTool> {
        let pm = self.plugin_manager.read().unwrap();
        
        // Find file type
        let Some(file_type_id) = pm.file_type_registry()
            .get_file_type_for_path(file_path) else {
            return vec![];
        };
        
        // Find editors that handle this type
        let editor_ids = pm.editor_registry()
            .get_editors_for_file_type(&file_type_id);
        
        let mut tools = Vec::new();
        
        for editor_id in editor_ids {
            if let Some(plugin_id) = pm.editor_registry()
                .get_plugin_for_editor(&editor_id) 
            {
                // Get this plugin's tools for this file type
                if let Some(plugin_tools) = self.plugin_tools.get(plugin_id) {
                    for (ft_id, tool_defs) in plugin_tools {
                        if ft_id == &file_type_id {
                            for tool_def in tool_defs {
                                tools.push(AvailableTool {
                                    name: tool_def.name.clone(),
                                    description: tool_def.description.clone(),
                                    plugin_id: plugin_id.clone(),
                                    editor_id: editor_id.clone(),
                                    parameters: tool_def.parameters_json_schema.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
        
        tools
    }
    
    /// Execute a plugin tool
    pub fn execute_tool(
        &self,
        file_path: &Path,
        plugin_id: &PluginId,
        tool_name: &str,
        tool_args: serde_json::Value,
    ) -> Result<serde_json::Value, PluginError> {
        let mut pm = self.plugin_manager.write().unwrap();
        
        let fs_context = FsContext {
            project_root: pm.project_root().cloned().unwrap_or_default(),
            plugin_manager: self.plugin_manager.clone(),
            engine_fs: Arc::new(EngineFs::default()),
        };
        
        pm.execute_plugin_tool(file_path, plugin_id, tool_name, tool_args, &fs_context)
    }
    
    /// Refresh tool cache from plugins
    pub fn refresh_all_tools(&mut self) {
        self.plugin_tools.clear();
        self.tool_lookup.clear();
        
        let pm = self.plugin_manager.read().unwrap();
        
        for plugin in pm.get_plugins() {
            let tools = pm.get_plugin_ai_tools(&plugin.id).unwrap_or_default();
            
            // Group tools by file type (for now, tools apply to all types)
            let file_types = pm.file_type_registry()
                .get_all_file_types()
                .into_iter()
                .map(|ft| ft.id.clone())
                .collect::<Vec<_>>();
            
            for file_type_id in file_types {
                for tool in &tools {
                    let key = (file_type_id.clone(), tool.name.clone());
                    self.tool_lookup.insert(
                        key, 
                        (plugin.id.clone(), tool.name.clone())
                    );
                }
            }
            
            if !tools.is_empty() {
                let entries = file_types.into_iter()
                    .map(|ft_id| (ft_id, tools.clone()))
                    .collect();
                self.plugin_tools.insert(plugin.id.clone(), entries);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileTypeInfo {
    pub id: FileTypeId,
    pub name: String,
    pub extension: String,
    pub icon: ui::IconName,
}

#[derive(Clone, Debug)]
pub struct EditorInfo {
    pub id: EditorId,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct AvailableTool {
    pub name: String,
    pub description: String,
    pub plugin_id: PluginId,
    pub editor_id: EditorId,
    pub parameters: serde_json::Value,
}
```

---

## Design Phase 3: AI Agent Integration

### Extend ToolRegistry to Include Plugin Tools

```rust
// Location: agent-providers/agent_chat_tools/src/lib.rs

#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ChatTool>>,
    
    /// NEW: Plugin tool bridge for dynamic tool discovery
    plugin_bridge: Option<Arc<RwLock<PluginToolBridge>>>,
}

impl ToolRegistry {
    /// Create with plugin tool bridge
    pub fn with_plugins(
        bridge: Arc<RwLock<PluginToolBridge>>
    ) -> Self {
        let mut this = Self::with_default_tools();
        this.plugin_bridge = Some(bridge);
        
        // Add helper tools for plugin discovery
        this.register(Arc::new(QueryAvailableFileTypesTool));
        this.register(Arc::new(QueryFileToolsTool));
        this.register(Arc::new(ExecutePluginToolTool));
        
        this
    }
    
    pub fn available_tools_schema(&self) -> Vec<serde_json::Value> {
        let mut schemas = vec![
            // Existing tools...
            json!({
                "name": "read_file",
                "description": "Read a UTF-8 text file from workspace.",
                "parameters": { /* ... */ }
            }),
            // NEW: Discovery tools
            json!({
                "name": "query_available_file_types",
                "description": "List all available file types and their editors",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }),
            json!({
                "name": "query_file_tools",
                "description": "Get available tools for editing a specific file",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" }
                    },
                    "required": ["file_path"]
                }
            }),
            json!({
                "name": "execute_plugin_tool",
                "description": "Execute a plugin-provided tool on a file",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" },
                        "plugin_id": { "type": "string" },
                        "tool_name": { "type": "string" },
                        "tool_args": { "type": "object" }
                    },
                    "required": ["file_path", "plugin_id", "tool_name", "tool_args"]
                }
            }),
        ];
        
        // If plugin bridge exists, add plugin-specific tools
        if let Some(bridge) = &self.plugin_bridge {
            if let Ok(b) = bridge.read() {
                // Add tools for each plugin's offerings
                // (could be generated dynamically per workspace)
            }
        }
        
        schemas
    }
}

// Tool implementations

struct QueryAvailableFileTypesTool;
impl ChatTool for QueryAvailableFileTypesTool {
    fn name(&self) -> &'static str {
        "query_available_file_types"
    }
    
    fn execute(&self, _args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let bridge = ctx.plugin_bridge.as_ref()
            .ok_or_else(|| anyhow!("Plugin bridge not available"))?;
        let b = bridge.read().unwrap();
        let types = b.available_file_types();
        Ok(json!({ "file_types": types }))
    }
}

struct QueryFileToolsTool;
impl ChatTool for QueryFileToolsTool {
    fn name(&self) -> &'static str {
        "query_file_tools"
    }
    
    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path = args["file_path"].as_str()
            .ok_or_else(|| anyhow!("file_path required"))?;
        
        let bridge = ctx.plugin_bridge.as_ref()
            .ok_or_else(|| anyhow!("Plugin bridge not available"))?;
        let b = bridge.read().unwrap();
        let tools = b.tools_for_file(Path::new(file_path));
        
        Ok(json!({
            "file_path": file_path,
            "available_tools": tools
        }))
    }
}

struct ExecutePluginToolTool;
impl ChatTool for ExecutePluginToolTool {
    fn name(&self) -> &'static str {
        "execute_plugin_tool"
    }
    
    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path = args["file_path"].as_str()?;
        let plugin_id = args["plugin_id"].as_str()?;
        let tool_name = args["tool_name"].as_str()?;
        let tool_args = &args["tool_args"];
        
        let bridge = ctx.plugin_bridge.as_ref()
            .ok_or_else(|| anyhow!("Plugin bridge not available"))?;
        let b = bridge.read().unwrap();
        
        let result = b.execute_tool(
            Path::new(file_path),
            &PluginId::new(plugin_id),
            tool_name,
            tool_args.clone(),
        )?;
        
        Ok(result)
    }
}
```

### ToolContext Enhancement

```rust
pub struct ToolContext {
    pub workspace_root: PathBuf,
    
    // NEW: Plugin bridge for AI tool discovery
    pub plugin_bridge: Option<Arc<RwLock<PluginToolBridge>>>,
}
```

---

## Design Phase 4: Agent Chat Panel Integration

### Initialize Plugin Tools

```rust
// In AgentChatPanel::new

pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
    // ... existing code ...
    
    // Initialize plugin tool bridge
    let plugin_bridge = if let Some(pm_lock) = plugin_manager::global() {
        if let Ok(pm) = pm_lock.read() {
            // Create bridge with reference to plugin manager
            Some(Arc::new(RwLock::new(
                PluginToolBridge::new(pm_lock.clone())
            )))
        } else {
            None
        }
    } else {
        None
    };
    
    // Create tool registry with plugin bridge
    let mut tool_registry = ToolRegistry::with_default_tools();
    if let Some(bridge) = plugin_bridge.clone() {
        tool_registry = ToolRegistry::with_plugins(bridge);
    }
    
    let mut this = Self {
        // ... existing fields ...
        tool_registry,
        plugin_bridge,
        // ... rest of init ...
    };
    
    this
}
```

---

## AI Workflow Example

### Scenario: AI Edits Multiple File Types

```
User: "Optimize the blueprint and validate the shader script"

AI Reasoning:
1. Query: "What file types are available?"
   → Gets list: [blueprint, shader, script, ...]
   
2. Query: "What files exist in project?"
   → Uses generic search_workspace tool
   → Finds: "level.blueprint", "main.shader", etc.
   
3. For each file, Query: "What tools do I have?"
   → For level.blueprint: ["refactor_nodes", "optimize_graph", "validate"]
   → For main.shader: ["compile_check", "optimize_performance", "validate"]
   
4. For blueprint:
   Execute: execute_plugin_tool(
     file_path="level.blueprint",
     plugin_id="com.pulsar.blueprint-editor",
     tool_name="optimize_graph",
     tool_args={strategy: "merge_nodes"}
   )
   
5. For shader:
   Execute: execute_plugin_tool(
     file_path="main.shader",
     plugin_id="com.pulsar.shader-editor",
     tool_name="compile_check",
     tool_args={target: "directx12"}
   )

Result: AI gets JSON responses with operation results
```

---

## Implementation Phases

### Phase 1: Core Plugin Extension (1-2 weeks)
- Extend `EditorPlugin` trait with `ai_tools()`, `execute_ai_tool()`, `capabilities_for_file()`
- Add `AiToolDefinition` struct
- Add `FsContext` for plugin operations
- Update `export_plugin!` macro if needed
- Update plugin_editor_api version

### Phase 2: Plugin Tool Bridge (1 week)
- Implement `PluginToolBridge` in plugin_manager
- Add tool discovery methods
- Implement tool execution dispatch
- Add global PluginToolBridge instance similar to global plugin manager

### Phase 3: Tool Registry Enhancement (1 week)
- Extend `ToolContext` with `plugin_bridge`
- Add discovery tools: `query_file_types`, `query_file_tools`
- Add execution tool: `execute_plugin_tool`
- Update `available_tools_schema()`

### Phase 4: Agent Chat Integration (1 week)
- Update `AgentChatPanel` to initialize plugin bridge
- Pass `plugin_bridge` through `ToolContext`
- Test tool discovery and execution in chat

### Phase 5: Built-in Editor Tools (1-2 weeks)
- Update built-in editors to implement AI tools
- Examples: text editor (find/replace), json editor (validate/format), etc.
- Test with real AI agent scenarios

### Phase 6: Documentation & Examples (1 week)
- Update PLUGIN_DEVELOPMENT.md with tool examples
- Create reference plugin with custom tools
- Document best practices

---

## Key Design Decisions

### 1. **Plugin-Scoped Tool Names**
Tools are namespaced by plugin internally but exposed to AI cleanly:
- Plugin provides: `["refactor", "validate"]`
- AI sees: `"blueprint-editor:refactor"`, `"blueprint-editor:validate"`
- Prevents naming collisions across plugins

### 2. **Lazy Tool Discovery**
- Tool list refreshed when: plugins load/unload, or on explicit refresh
- Cached to avoid repeated plugin queries
- Bridges can be event-subscribed for invalidation

### 3. **File Type → Tool Mapping**
- Tools are associated with file types they handle
- Determined by editor registration (editor → plugin → tools)
- Allows AI to query "what can I do with .blueprint files?"

### 4. **Optional Plugin Tool Implementation**
- `ai_tools()` defaults to empty vector
- Plugins gradually adopt tool API
- Backward compatible with existing plugins

### 5. **Context Isolation**
- Plugins receive `FsContext` with controlled access
- No direct plugin manager access (via Arc<RwLock>)
- All filesystem ops go through engine_fs abstraction

### 6. **Error Handling**
- Tool execution errors returned as JSON (not exceptions)
- Allows AI to handle failures gracefully
- Error format: `{"error": "message", "details": {...}}`

---

## Potential Challenges & Mitigations

### Challenge 1: Too Many Tools Overwhelming AI
**Mitigation**: 
- Provide `capabilities_for_file()` to filter relevant tools
- Group tools by category
- AI can use `query_file_tools()` to get precise matches
- Smart tool prioritization in tool schema

### Challenge 2: Plugin Tool Stability
**Mitigation**:
- Version plugin tools independently
- Tool definitions must be Serialize/stable
- Breaking changes require major version bump
- Backward compatibility guidelines in docs

### Challenge 3: Execution Errors in Plugins
**Mitigation**:
- Catch all panics in FFI boundary
- Return detailed error messages
- Log failures to plugin logger
- AI can retry with different tool or approach

### Challenge 4: Performance with Many Plugins
**Mitigation**:
- Tool cache updated at plugin load time, not per-query
- HashMap lookups O(1) for common operations
- Batch queries possible (e.g., get all tools at once)
- Lazy evaluation of file type scanning

### Challenge 5: Security of Plugin Tool Execution
**Mitigation**:
- Plugins only get `FsContext` (immutable + controlled operations)
- All file operations go through engine_fs (can add permissions)
- Tool arguments validated against JSON schema first
- Optional plugin sandboxing/capability system later

---

## Future Enhancements

### 1. **Hierarchical Tool Organization**
```rust
pub enum ToolCategory {
    FileEditing,
    Validation,
    Refactoring,
    Analysis,
    CodeGeneration,
    Custom(String),
}
```

### 2. **Tool Dependencies**
- Allow tools to declare prerequisites
- AI can understand "must run check before optimize"

### 3. **Streaming Tool Results**
- For long-running operations
- Provide progress updates to AI

### 4. **Plugin-to-Plugin Tools**
- Plugins can call tools from other plugins
- Enables cross-plugin workflows

### 5. **Tool Permissions System**
- Declare what files/operations a tool can access
- Support trusted/untrusted plugin modes

### 6. **Undo/Redo for Tool Operations**
- Plugins track changes made by tools
- Integrate with editor undo system

### 7. **Concurrent Tool Execution**
- Multiple plugins executing tools in parallel
- Coordinated file access

---

## API Compatibility

### Breaking Changes
- None initially; all new APIs optional
- Existing plugins continue to work

### Versioning
- Plugin version used for tool compatibility
- Engine major version must match for tools
- Tool schema changes tracked separately

---

## Testing Strategy

### Unit Tests
- Tool registry operations
- Tool lookup and dispatch
- Error handling

### Integration Tests
- Plugin with custom tools loads correctly
- Tools appear in schema
- Tool execution works end-to-end
- Multiple plugins don't interfere

### Chat Tests
- AI can discover file types
- AI can query available tools
- AI can execute tools and get results
- Multi-tool workflows

### Performance Tests
- Large number of plugins (100+)
- Many file types
- Rapid tool queries/execution

---

## Documentation

### For Plugin Developers
- Updated PLUGIN_DEVELOPMENT.md
- AiToolDefinition schema
- FsContext capabilities
- Error handling patterns
- Example: Simple tool plugin
- Example: Complex multi-tool plugin
- Best practices guide

### For Engine Developers
- PluginToolBridge architecture
- Tool registry integration points
- Agent chat integration points
- Extension hooks for future work

### For Users/AI Agents
- What plugins provide tools
- How to ask for capabilities
- Tool usage examples
- Troubleshooting

---

## Success Criteria

1. ✅ Plugins can declare and provide custom AI tools
2. ✅ AI can discover available tools dynamically
3. ✅ AI can execute plugin tools on files
4. ✅ System handles 10+ plugins without performance issues
5. ✅ Error handling allows graceful fallback
6. ✅ Existing plugins continue to work unchanged
7. ✅ Built-in editors provide basic tools
8. ✅ Example plugins demonstrate capabilities
9. ✅ Documentation is comprehensive
10. ✅ Real-world AI workflows work end-to-end

---

## Repository Organization

### New/Modified Files

**Crates/plugin_manager:**
- `src/plugin_tool_bridge.rs` (NEW)
- `src/lib.rs` (extend PluginManager)
- `src/registry.rs` (minimal changes)

**Crates/plugin_editor_api:**
- `src/lib.rs` (extend EditorPlugin trait)
- Add `AiToolDefinition`
- Add `FsContext`
- Add `export_plugin!` macro updates

**Agent-providers/agent_chat_tools:**
- `src/lib.rs` (extend ToolRegistry)
- Add discovery tools
- Add execution tool
- Update ToolContext

**UI-crates/ui_core:**
- `src/app/agent_chat_panel/mod.rs` (extend initialization)
- Pass plugin_bridge to ToolRegistry

**Docs:**
- `PLUGIN_DEVELOPMENT.md` (extend with AI tools section)
- `AI_PLUGINS_INTEGRATION_PLAN.md` (this file)
- `AI_TOOLS_BEST_PRACTICES.md` (NEW)

---

## Timeline Estimate

**Total: 6-8 weeks**
- Phase 1: 1-2 weeks
- Phase 2: 1 week
- Phase 3: 1 week
- Phase 4: 1 week
- Phase 5: 1-2 weeks
- Phase 6: 1 week
- Buffer: 1 week

Could be parallelized to reduce to 4-5 weeks.

---

## References

- [Plugin Manager](../crates/plugin_manager/src/lib.rs)
- [Plugin API](../crates/plugin_editor_api/src/lib.rs)
- [Agent Chat Tools](../agent-providers/agent_chat_tools/src/lib.rs)
- [Plugin Development Guide](PLUGIN_DEVELOPMENT.md)
- [Agent Chat Panel](../ui-crates/ui_core/src/app/agent_chat_panel/mod.rs)
