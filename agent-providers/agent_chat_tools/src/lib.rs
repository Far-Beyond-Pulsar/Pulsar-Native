use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

#[derive(Clone)]
pub struct ToolContext {
    pub workspace_root: PathBuf,
    /// Optional plugin tool bridge for accessing plugin tools
    pub plugin_bridge: Option<Arc<RwLock<plugin_manager::PluginToolBridge>>>,
    /// Current file being edited (if any)
    pub current_file: Option<PathBuf>,
    /// Optional callback to open a file through the app's default editor flow.
    pub open_file_request: Option<Arc<dyn Fn(PathBuf) -> Result<(), String> + Send + Sync>>,
    /// Optional callback to query active/inactive open editors tracked by the engine.
    pub query_open_editors: Option<Arc<dyn Fn() -> Result<Value, String> + Send + Sync>>,
    /// Optional callback to activate one of the already-open editor tabs by index.
    pub activate_open_editor_request: Option<Arc<dyn Fn(usize) -> Result<(), String> + Send + Sync>>,
}

pub trait ChatTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value>;
}

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ChatTool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_default_tools() -> Self {
        let mut this = Self::new();
        this.register(Arc::new(OpenFileInDefaultEditorTool));
        this.register(Arc::new(QueryOpenEditorsTool));
        this.register(Arc::new(ActivateOpenEditorTool));
        this.register(Arc::new(QueryAvailableFileTypesTool));
        this.register(Arc::new(QueryFileEditorsTool));
        this.register(Arc::new(QueryPluginToolsTool));
        this.register(Arc::new(QueryToolsForPluginTool));
        this.register(Arc::new(ExecutePluginToolTool));
        this
    }

    pub fn register(&mut self, tool: Arc<dyn ChatTool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn execute(&self, name: &str, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let Some(tool) = self.tools.get(name) else {
            return Err(anyhow!("Unknown tool: {name}"));
        };
        tool.execute(args, ctx)
    }

    pub fn available_tools_schema(&self) -> Vec<Value> {
        vec![
            json!({
                "name": "open_file_in_default_editor",
                "description": "Open a file in its default editor tab. Call this before plugin edit tools so edits happen in editor state, not direct file access.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" }
                    },
                    "required": ["file_path"]
                }
            }),
            json!({
                "name": "query_open_editors",
                "description": "List already open editors and indicate which one is active versus inactive.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }),
            json!({
                "name": "activate_open_editor",
                "description": "Activate one of the already-open editors by its index from query_open_editors.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "index": { "type": "integer", "minimum": 0 }
                    },
                    "required": ["index"]
                }
            }),
            json!({
                "name": "query_available_file_types",
                "description": "List all file types currently registered by plugins/editors.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }),
            json!({
                "name": "query_file_editors",
                "description": "Query which editors/plugins can handle a specific file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" }
                    },
                    "required": ["file_path"]
                }
            }),
            json!({
                "name": "query_plugin_tools",
                "description": "Query available AI tools from plugins for the current file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string", "description": "Optional file path. If not provided, uses current_file from context." }
                    }
                }
            }),
            json!({
                "name": "query_tools_for_plugin",
                "description": "Query AI tools provided by a specific plugin, optionally scoped to a file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "plugin_id": { "type": "string", "description": "Plugin id to inspect" },
                        "file_path": { "type": "string", "description": "Optional file path to filter tools by capability for that file" }
                    },
                    "required": ["plugin_id"]
                }
            }),
            json!({
                "name": "execute_plugin_tool",
                "description": "Execute an AI tool provided by a plugin.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "plugin_id": { "type": "string", "description": "Plugin id returned by query_plugin_tools. Recommended when tool names overlap." },
                        "tool_name": { "type": "string", "description": "Name of the tool to execute" },
                        "file_path": { "type": "string", "description": "File to operate on. If not provided, uses current_file from context." },
                        "tool_args": { "type": "object", "description": "Arguments to pass to the tool", "additionalProperties": true }
                    },
                    "required": ["tool_name", "tool_args"]
                }
            }),
        ]
    }
}

struct OpenFileInDefaultEditorTool;
impl ChatTool for OpenFileInDefaultEditorTool {
    fn name(&self) -> &'static str {
        "open_file_in_default_editor"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("open_file_in_default_editor.file_path is required"))?;
        let full = resolve_workspace_path(&ctx.workspace_root, file_path)?;

        let callback = ctx
            .open_file_request
            .as_ref()
            .ok_or_else(|| anyhow!("Open-file callback unavailable in this context"))?;
        callback(full.clone()).map_err(|err| anyhow!(err))?;

        Ok(json!({
            "ok": true,
            "file_path": full.display().to_string(),
            "opened": true,
        }))
    }
}

struct QueryOpenEditorsTool;
impl ChatTool for QueryOpenEditorsTool {
    fn name(&self) -> &'static str {
        "query_open_editors"
    }

    fn execute(&self, _args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let callback = ctx
            .query_open_editors
            .as_ref()
            .ok_or_else(|| anyhow!("Open-editors callback unavailable in this context"))?;
        callback().map_err(|err| anyhow!(err))
    }
}

struct ActivateOpenEditorTool;
impl ChatTool for ActivateOpenEditorTool {
    fn name(&self) -> &'static str {
        "activate_open_editor"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let index = args
            .get("index")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow!("activate_open_editor.index is required"))? as usize;

        let callback = ctx
            .activate_open_editor_request
            .as_ref()
            .ok_or_else(|| anyhow!("Activate-open-editor callback unavailable in this context"))?;
        callback(index).map_err(|err| anyhow!(err))?;

        Ok(json!({
            "ok": true,
            "index": index,
            "activated": true,
        }))
    }
}

struct QueryAvailableFileTypesTool;
impl ChatTool for QueryAvailableFileTypesTool {
    fn name(&self) -> &'static str {
        "query_available_file_types"
    }

    fn execute(&self, _args: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        let manager_lock = plugin_manager::global()
            .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
        let manager = manager_lock
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

        let file_types = manager
            .file_type_registry()
            .get_all_file_types()
            .into_iter()
            .map(|ft| {
                json!({
                    "id": ft.id.to_string(),
                    "extension": ft.extension,
                    "display_name": ft.display_name,
                    "structure": format!("{:?}", ft.structure),
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "count": file_types.len(),
            "file_types": file_types,
        }))
    }
}

struct QueryFileEditorsTool;
impl ChatTool for QueryFileEditorsTool {
    fn name(&self) -> &'static str {
        "query_file_editors"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("query_file_editors.file_path is required"))?;
        let full = resolve_workspace_path(&ctx.workspace_root, file_path)?;

        let manager_lock = plugin_manager::global()
            .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
        let manager = manager_lock
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

        let Some(file_type_id) = manager.file_type_registry().get_file_type_for_path(&full) else {
            return Ok(json!({
                "file_path": full.display().to_string(),
                "file_type": null,
                "editors": [],
            }));
        };

        let editors = manager
            .editor_registry()
            .get_editors_for_file_type(&file_type_id)
            .into_iter()
            .map(|editor_id| {
                let plugin_id = manager
                    .editor_registry()
                    .get_plugin_for_editor(&editor_id)
                    .map(|pid| pid.to_string());
                json!({
                    "editor_id": editor_id.to_string(),
                    "plugin_id": plugin_id,
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "file_path": full.display().to_string(),
            "file_type": file_type_id.to_string(),
            "editors": editors,
        }))
    }
}

struct QueryPluginToolsTool;
impl ChatTool for QueryPluginToolsTool {
    fn name(&self) -> &'static str {
        "query_plugin_tools"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| PathBuf::from(p))
            .or_else(|| ctx.current_file.clone());

        let file_path_str = file_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(no file)".to_string());

        let manager_lock = plugin_manager::global()
            .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
        let manager = manager_lock
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

        let tools = if let Some(file_path) = &file_path {
            let full = resolve_workspace_path(&ctx.workspace_root, &file_path.display().to_string())?;
            manager.build_tool_bridge_for_file(&full).all_tools()
        } else {
            manager.build_tool_bridge().all_tools()
        };

        let tool_schemas: Vec<Value> = tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.definition.name,
                    "description": tool.definition.description,
                    "category": tool.definition.category,
                    "parameters": tool.definition.parameters_json_schema,
                    "plugin_id": tool.plugin_id.to_string(),
                })
            })
            .collect();

        let mut tools_by_plugin: HashMap<String, Vec<Value>> = HashMap::new();
        for tool in &tool_schemas {
            if let Some(plugin_id) = tool.get("plugin_id").and_then(|v| v.as_str()) {
                tools_by_plugin
                    .entry(plugin_id.to_string())
                    .or_default()
                    .push(tool.clone());
            }
        }

        let plugins = tools_by_plugin
            .into_iter()
            .map(|(plugin_id, tools)| {
                json!({
                    "plugin_id": plugin_id,
                    "tool_count": tools.len(),
                    "tools": tools,
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "file_path": file_path_str,
            "tools_available": tool_schemas.len(),
            "tools": tool_schemas,
            "plugins": plugins,
        }))
    }
}

struct QueryToolsForPluginTool;
impl ChatTool for QueryToolsForPluginTool {
    fn name(&self) -> &'static str {
        "query_tools_for_plugin"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let plugin_id = args
            .get("plugin_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("query_tools_for_plugin.plugin_id is required"))?;

        let manager_lock = plugin_manager::global()
            .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
        let manager = manager_lock
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

        let tools = if let Some(file_path) = args.get("file_path").and_then(|v| v.as_str()) {
            let full = resolve_workspace_path(&ctx.workspace_root, file_path)?;
            manager
                .build_tool_bridge_for_file(&full)
                .all_tools()
                .into_iter()
            .filter(|tool| tool.plugin_id.to_string() == plugin_id)
                .map(|tool| tool.definition)
                .collect::<Vec<_>>()
        } else {
            manager
            .build_tool_bridge()
            .all_tools()
            .into_iter()
            .filter(|tool| tool.plugin_id.to_string() == plugin_id)
            .map(|tool| tool.definition)
            .collect::<Vec<_>>()
        };

        let tool_schemas = tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "category": tool.category,
                    "parameters": tool.parameters_json_schema,
                    "plugin_id": plugin_id,
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "plugin_id": plugin_id,
            "tools_available": tool_schemas.len(),
            "tools": tool_schemas,
        }))
    }
}

struct ExecutePluginToolTool;
impl ChatTool for ExecutePluginToolTool {
    fn name(&self) -> &'static str {
        "execute_plugin_tool"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let explicit_plugin_id = args.get("plugin_id").and_then(|v| v.as_str());
        let tool_name = args
            .get("tool_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("execute_plugin_tool.tool_name is required"))?;

        let tool_args = args.get("tool_args").cloned().unwrap_or_else(|| json!({}));

        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| PathBuf::from(p))
            .or_else(|| ctx.current_file.clone())
            .ok_or_else(|| anyhow!("No file path provided or available in context"))?;
        let full_file_path = resolve_workspace_path(&ctx.workspace_root, &file_path.display().to_string())?;

        let manager_lock = plugin_manager::global()
            .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
        let manager = manager_lock
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

        let bridge = manager.build_tool_bridge();
        let plugin_id = if let Some(plugin_id) = explicit_plugin_id {
            bridge
                .all_tools()
                .into_iter()
                .find(|tool| tool.plugin_id.to_string() == plugin_id)
                .map(|tool| tool.plugin_id)
                .ok_or_else(|| anyhow!("Plugin id not found: {}", plugin_id))?
        } else {
            bridge
                .plugin_for_tool(tool_name)
                .ok_or_else(|| anyhow!("Tool not found or plugin not resolvable: {}", tool_name))?
        };

        let result = manager
            .execute_plugin_ai_tool(&plugin_id, &full_file_path, tool_name, tool_args)
            .map_err(|err| anyhow!(err.to_string()))?;

        Ok(json!({
            "status": "ok",
            "plugin_id": plugin_id.to_string(),
            "tool_name": tool_name,
            "file_path": full_file_path.display().to_string(),
            "result": result,
        }))
    }
}

fn resolve_workspace_path(root: &Path, rel_or_abs: &str) -> anyhow::Result<PathBuf> {
    let p = PathBuf::from(rel_or_abs);
    let joined = if p.is_absolute() { p } else { root.join(p) };
    let canonical = joined
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", joined.display()))?;

    let root_canonical = root
        .canonicalize()
        .with_context(|| format!("Workspace root missing: {}", root.display()))?;

    if !canonical.starts_with(&root_canonical) {
        return Err(anyhow!("Path escapes workspace root"));
    }
    Ok(canonical)
}
