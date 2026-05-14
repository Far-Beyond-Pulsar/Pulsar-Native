use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

pub use tool_registry::{ChatTool, ToolContext, ToolRegistry};

pub type OpenFileRequest = Arc<dyn Fn(PathBuf) -> Result<(), String> + Send + Sync>;
pub type QueryOpenEditorsRequest = Arc<dyn Fn() -> Result<Value, String> + Send + Sync>;
pub type ActivateOpenEditorRequest = Arc<dyn Fn(usize) -> Result<(), String> + Send + Sync>;

#[derive(Clone, Default)]
pub struct PulsarToolExtras {
    pub plugin_bridge: Option<Arc<RwLock<plugin_manager::PluginToolBridge>>>,
    pub open_file_request: Option<OpenFileRequest>,
    pub query_open_editors: Option<QueryOpenEditorsRequest>,
    pub activate_open_editor_request: Option<ActivateOpenEditorRequest>,
}

const EXTRAS_KEY: &str = "pulsar_tool_extras";

pub fn make_tool_context(
    workspace_root: PathBuf,
    current_file: Option<PathBuf>,
    extras: PulsarToolExtras,
) -> ToolContext {
    let mut ctx = ToolContext::new().with_workspace(workspace_root);
    if let Some(file) = current_file {
        ctx = ctx.with_current_file(file);
    }
    ctx.insert_extra(EXTRAS_KEY, extras);
    ctx
}

pub fn build_default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_pulsar_tools(&mut registry);
    tool_registry_builtin::register_builtins(&mut registry);
    registry
}

pub fn register_pulsar_tools(registry: &mut ToolRegistry) {
    registry.register(Arc::new(OpenFileInDefaultEditorTool));
    registry.register(Arc::new(QueryOpenEditorsTool));
    registry.register(Arc::new(ActivateOpenEditorTool));
    registry.register(Arc::new(QueryAvailableFileTypesTool));
    registry.register(Arc::new(QueryFileEditorsTool));
    registry.register(Arc::new(QueryPluginToolsTool));
    registry.register(Arc::new(QueryToolsForPluginTool));
    registry.register(Arc::new(ExecutePluginToolTool));
}

fn extras(ctx: &ToolContext) -> Option<&PulsarToolExtras> {
    ctx.get_extra::<PulsarToolExtras>(EXTRAS_KEY)
}

fn workspace_root(ctx: &ToolContext) -> PathBuf {
    ctx.workspace_root
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
}

struct OpenFileInDefaultEditorTool;
impl ChatTool for OpenFileInDefaultEditorTool {
    fn name(&self) -> &'static str {
        "open_file_in_default_editor"
    }

    fn description(&self) -> &'static str {
        "Open a file in its default editor tab. Call this before plugin edit tools so edits happen in editor state, not direct file access."
    }

    fn category(&self) -> Option<&'static str> {
        Some("pulsar")
    }

    fn parameters_schema(&self) -> Value {
        tool_registry::tool_params! {
            req "file_path": string = "Absolute or workspace-relative path of the file to open"
        }
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("open_file_in_default_editor.file_path is required"))?;
        let full = resolve_workspace_path(&workspace_root(ctx), file_path)?;

        let callback = extras(ctx)
            .and_then(|e| e.open_file_request.as_ref())
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

    fn description(&self) -> &'static str {
        "List already-open editors and indicate which is active. Returns file_path for each - use those exact paths with query_plugin_tools and execute_plugin_tool."
    }

    fn category(&self) -> Option<&'static str> {
        Some("pulsar")
    }

    fn parameters_schema(&self) -> Value {
        tool_registry::tool_params!()
    }

    fn execute(&self, _args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let callback = extras(ctx)
            .and_then(|e| e.query_open_editors.as_ref())
            .ok_or_else(|| anyhow!("Open-editors callback unavailable in this context"))?;
        callback().map_err(|err| anyhow!(err))
    }
}

struct ActivateOpenEditorTool;
impl ChatTool for ActivateOpenEditorTool {
    fn name(&self) -> &'static str {
        "activate_open_editor"
    }

    fn description(&self) -> &'static str {
        "Switch focus to an already-open editor by its index returned from query_open_editors."
    }

    fn category(&self) -> Option<&'static str> {
        Some("pulsar")
    }

    fn parameters_schema(&self) -> Value {
        tool_registry::tool_params! {
            req "index": integer = "Zero-based index of the editor to activate"
        }
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let index = args
            .get("index")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow!("activate_open_editor.index is required"))?
            as usize;

        let callback = extras(ctx)
            .and_then(|e| e.activate_open_editor_request.as_ref())
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

    fn description(&self) -> &'static str {
        "List all file types registered by installed plugins/editors."
    }

    fn category(&self) -> Option<&'static str> {
        Some("pulsar")
    }

    fn parameters_schema(&self) -> Value {
        tool_registry::tool_params!()
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

    fn description(&self) -> &'static str {
        "Query which plugins/editors can handle a given file path."
    }

    fn category(&self) -> Option<&'static str> {
        Some("pulsar")
    }

    fn parameters_schema(&self) -> Value {
        tool_registry::tool_params! {
            req "file_path": string = "Path of the file to query editors for"
        }
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("query_file_editors.file_path is required"))?;
        let full = resolve_workspace_path(&workspace_root(ctx), file_path)?;

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

    fn description(&self) -> &'static str {
        "Discover AI tools available from plugins for a specific file. Use the file_path returned by query_open_editors - do not guess paths."
    }

    fn category(&self) -> Option<&'static str> {
        Some("pulsar")
    }

    fn parameters_schema(&self) -> Value {
        tool_registry::tool_params! {
            req "file_path": string = "Exact file_path from query_open_editors output"
        }
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path_raw = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| p.to_string())
            .or_else(|| ctx.current_file.as_ref().map(|p| p.display().to_string()));

        let Some(file_path_raw) = file_path_raw else {
            return Ok(json!({
                "ok": false,
                "error": "file_path is required. Provide the path of the file you want to query tools for.",
                "tools_available": 0,
                "tools": [],
                "plugins": [],
            }));
        };

        let full = resolve_workspace_path_soft(&workspace_root(ctx), &file_path_raw)?;
        let file_path_str = full.display().to_string();

        let manager_lock = plugin_manager::global()
            .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
        let manager = manager_lock
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

        let tools = manager.build_tool_bridge_for_file(&full).all_tools();

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
            "ok": true,
            "file_path": file_path_str,
            "tools_available": tool_schemas.len(),
            "note": if tool_schemas.is_empty() {
                format!(
                    "No tools are registered for '{}'. The file type may not be supported, or the required editor is not loaded. Check that the file is open in an editor first.",
                    file_path_str
                )
            } else {
                format!("Found {} tool(s) for this file.", tool_schemas.len())
            },
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

    fn description(&self) -> &'static str {
        "List AI tools provided by a specific plugin, optionally scoped to a file."
    }

    fn category(&self) -> Option<&'static str> {
        Some("pulsar")
    }

    fn parameters_schema(&self) -> Value {
        tool_registry::tool_params! {
            req "plugin_id": string = "Plugin id to inspect",
            opt "file_path": string = "Optional file path to filter tools by file capability"
        }
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
            let full = resolve_workspace_path(&workspace_root(ctx), file_path)?;
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

    fn description(&self) -> &'static str {
        "Execute an AI tool provided by a plugin. Call query_plugin_tools first to discover available tools and their parameters."
    }

    fn category(&self) -> Option<&'static str> {
        Some("pulsar")
    }

    fn parameters_schema(&self) -> Value {
        tool_registry::tool_params! {
            req "tool_name": string = "Name of the tool to execute (from query_plugin_tools)",
            req "tool_args": object = "Arguments matching the tool's parameter schema",
            opt "plugin_id": string = "Plugin id from query_plugin_tools - recommended when tool names may overlap",
            opt "file_path": string = "File to operate on; defaults to current context file"
        }
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
            .map(PathBuf::from)
            .or_else(|| ctx.current_file.clone())
            .ok_or_else(|| anyhow!("No file path provided or available in context"))?;

        let full_file_path =
            resolve_workspace_path_soft(&workspace_root(ctx), &file_path.display().to_string())?;

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
                .find(|tool| {
                    tool.plugin_id.to_string() == plugin_id && tool.definition.name == tool_name
                })
                .map(|tool| tool.plugin_id)
                .ok_or_else(|| anyhow!("Tool '{}' not found for plugin id '{}'", tool_name, plugin_id))?
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

fn resolve_workspace_path_soft(root: &Path, rel_or_abs: &str) -> anyhow::Result<PathBuf> {
    let p = PathBuf::from(rel_or_abs);
    let joined = if p.is_absolute() { p } else { root.join(&p) };

    if let Ok(canonical) = joined.canonicalize() {
        if let Ok(root_canonical) = root.canonicalize() {
            if !canonical.starts_with(&root_canonical) {
                return Err(anyhow!("Path escapes workspace root"));
            }
        }
        return Ok(canonical);
    }

    let mut components = Vec::new();
    for part in joined.components() {
        use std::path::Component;
        match part {
            Component::ParentDir => {
                components.pop();
            }
            Component::CurDir => {}
            other => components.push(other),
        }
    }
    let normalized: PathBuf = components.iter().collect();

    let root_normalized: PathBuf = {
        let mut c = Vec::new();
        for part in root.components() {
            use std::path::Component;
            match part {
                Component::ParentDir => {
                    c.pop();
                }
                Component::CurDir => {}
                other => c.push(other),
            }
        }
        c.iter().collect()
    };

    if root_normalized.is_absolute() && !normalized.starts_with(&root_normalized) {
        return Err(anyhow!("Path escapes workspace root"));
    }
    Ok(normalized)
}
