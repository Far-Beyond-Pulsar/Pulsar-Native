use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

#[derive(Clone, Debug)]
pub struct ToolContext {
    pub workspace_root: PathBuf,
    /// Optional plugin tool bridge for accessing plugin tools
    pub plugin_bridge: Option<Arc<RwLock<plugin_manager::PluginToolBridge>>>,
    /// Current file being edited (if any)
    pub current_file: Option<PathBuf>,
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
        this.register(Arc::new(ReadFileTool));
        this.register(Arc::new(ListDirTool));
        this.register(Arc::new(SearchWorkspaceTool));
        this.register(Arc::new(QueryPluginToolsTool));
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
                "name": "read_file",
                "description": "Read a UTF-8 text file from workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "max_chars": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["path"]
                }
            }),
            json!({
                "name": "list_dir",
                "description": "List entries in a directory from workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                }
            }),
            json!({
                "name": "search_workspace",
                "description": "Search for text in workspace files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "max_results": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["query"]
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
                "name": "execute_plugin_tool",
                "description": "Execute an AI tool provided by a plugin.",
                "parameters": {
                    "type": "object",
                    "properties": {
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

struct ReadFileTool;
impl ChatTool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("read_file.path is required"))?;
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(20_000) as usize;

        let full = resolve_workspace_path(&ctx.workspace_root, path)?;
        let content = fs::read_to_string(&full)
            .with_context(|| format!("Failed reading file {}", full.display()))?;
        let truncated = if content.chars().count() > max_chars {
            content.chars().take(max_chars).collect::<String>()
        } else {
            content
        };

        Ok(json!({
            "path": full.display().to_string(),
            "truncated": truncated,
        }))
    }
}

struct ListDirTool;
impl ChatTool for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("list_dir.path is required"))?;
        let full = resolve_workspace_path(&ctx.workspace_root, path)?;

        let mut entries = Vec::new();
        for entry in
            fs::read_dir(&full).with_context(|| format!("Failed reading dir {}", full.display()))?
        {
            let entry = entry?;
            let p = entry.path();
            entries.push(json!({
                "name": entry.file_name().to_string_lossy().to_string(),
                "is_dir": p.is_dir(),
            }));
        }

        Ok(json!({
            "path": full.display().to_string(),
            "entries": entries,
        }))
    }
}

struct SearchWorkspaceTool;
impl ChatTool for SearchWorkspaceTool {
    fn name(&self) -> &'static str {
        "search_workspace"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("search_workspace.query is required"))?
            .to_lowercase();
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;

        let mut results = Vec::new();
        visit_files(&ctx.workspace_root, &mut |path| {
            if results.len() >= max_results {
                return;
            }
            if let Ok(content) = fs::read_to_string(path) {
                for (line_no, line) in content.lines().enumerate() {
                    if line.to_lowercase().contains(&query) {
                        results.push(json!({
                            "path": path.display().to_string(),
                            "line": line_no + 1,
                            "text": line,
                        }));
                        if results.len() >= max_results {
                            break;
                        }
                    }
                }
            }
        });

        Ok(json!({ "query": query, "results": results }))
    }
}

struct QueryPluginToolsTool;
impl ChatTool for QueryPluginToolsTool {
    fn name(&self) -> &'static str {
        "query_plugin_tools"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let bridge = ctx
            .plugin_bridge
            .as_ref()
            .ok_or_else(|| anyhow!("Plugin bridge not available"))?;

        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| PathBuf::from(p))
            .or_else(|| ctx.current_file.clone());

        let file_path_str = file_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(no file)".to_string());

        let bridge_lock = bridge
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin bridge"))?;

        let tools = if let Some(file_path) = &file_path {
            bridge_lock.tools_for_file(file_path)
        } else {
            bridge_lock.all_tools()
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

        Ok(json!({
            "file_path": file_path_str,
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
        let tool_name = args
            .get("tool_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("execute_plugin_tool.tool_name is required"))?;

        let _tool_args = args.get("tool_args").cloned().unwrap_or_else(|| json!({}));

        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| PathBuf::from(p))
            .or_else(|| ctx.current_file.clone())
            .ok_or_else(|| anyhow!("No file path provided or available in context"))?;

        let bridge = ctx
            .plugin_bridge
            .as_ref()
            .ok_or_else(|| anyhow!("Plugin bridge not available"))?;

        let bridge_lock = bridge
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin bridge"))?;

        let _tool = bridge_lock
            .tool(tool_name)
            .ok_or_else(|| anyhow!("Tool not found: {}", tool_name))?;

        // Note: We would need mutable access to the plugin instance to execute the tool.
        // This requires integration with the plugin manager. For now, return a placeholder.
        // The actual execution would go through PluginManager::execute_plugin_tool()

        Ok(json!({
            "status": "error",
            "message": "Plugin tool execution requires PluginManager integration. Please execute through the UI.",
            "tool_name": tool_name,
            "file_path": file_path.display().to_string(),
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

fn visit_files(dir: &Path, f: &mut impl FnMut(&Path)) {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|name| name == ".git" || name == "target")
            .unwrap_or(false)
        {
            continue;
        }

        if path.is_dir() {
            visit_files(&path, f);
        } else {
            f(&path);
        }
    }
}
