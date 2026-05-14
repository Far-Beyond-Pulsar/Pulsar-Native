use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, RwLock},
};

pub use tool_registry::{ChatTool, PluginToolRegistry, ToolContext, ToolRegistry};
use tool_registry_macros::tool;

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

#[derive(Clone)]
struct RuntimeState {
    workspace_root: PathBuf,
    current_file: Option<PathBuf>,
    extras: PulsarToolExtras,
}

static RUNTIME_STATE: OnceLock<Mutex<RuntimeState>> = OnceLock::new();

fn set_runtime_state(workspace_root: PathBuf, current_file: Option<PathBuf>, extras: PulsarToolExtras) {
    let state = RUNTIME_STATE.get_or_init(|| {
        Mutex::new(RuntimeState {
            workspace_root: workspace_root.clone(),
            current_file: current_file.clone(),
            extras: extras.clone(),
        })
    });

    if let Ok(mut guard) = state.lock() {
        guard.workspace_root = workspace_root;
        guard.current_file = current_file;
        guard.extras = extras;
    }
}

fn runtime_state() -> anyhow::Result<RuntimeState> {
    let state = RUNTIME_STATE
        .get()
        .ok_or_else(|| anyhow!("Tool runtime state is not initialized"))?;
    let guard = state
        .lock()
        .map_err(|_| anyhow!("Tool runtime state lock poisoned"))?;
    Ok(guard.clone())
}

fn runtime_workspace_root() -> anyhow::Result<PathBuf> {
    Ok(runtime_state()?.workspace_root)
}

fn runtime_current_file() -> anyhow::Result<Option<PathBuf>> {
    Ok(runtime_state()?.current_file)
}

fn runtime_extras() -> anyhow::Result<PulsarToolExtras> {
    Ok(runtime_state()?.extras)
}

pub fn make_tool_context(
    workspace_root: PathBuf,
    current_file: Option<PathBuf>,
    extras: PulsarToolExtras,
) -> ToolContext {
    set_runtime_state(workspace_root.clone(), current_file.clone(), extras.clone());

    let mut ctx = ToolContext::new().with_workspace(workspace_root);
    if let Some(file) = current_file {
        ctx = ctx.with_current_file(file);
    }
    if let Some(root) = ctx.workspace_root.clone() {
        engine_fs::tooling::insert_tooling_state(&mut ctx, root);
    }
    ctx.insert_extra(EXTRAS_KEY, extras);
    ctx
}

pub fn build_default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    engine_fs::tooling::register_tools(&mut registry);
    register_pulsar_tools(&mut registry);
    tool_registry_builtin::register_builtins(&mut registry);
    registry
}

pub fn register_pulsar_tools(registry: &mut ToolRegistry) {
    registry.merge_plugin(&PluginToolRegistry::from_namespace(module_path!()));
}

/// Open a file in its default editor tab. Call this before plugin edit tools so edits happen in editor state, not direct file access.
#[tool(category = "pulsar")]
pub fn open_file_in_default_editor(file_path: String) -> anyhow::Result<Value> {
    let root = runtime_workspace_root()?;
    let full = resolve_workspace_path(&root, &file_path)?;

    let callback = runtime_extras()?
        .open_file_request
        .ok_or_else(|| anyhow!("Open-file callback unavailable in this context"))?;
    callback(full.clone()).map_err(|err| anyhow!(err))?;

    Ok(json!({
        "ok": true,
        "file_path": full.display().to_string(),
        "opened": true,
    }))
}

/// List already-open editors and indicate which is active. Returns file_path for each - use those exact paths with query_plugin_tools and execute_plugin_tool.
#[tool(category = "pulsar")]
pub fn query_open_editors() -> anyhow::Result<Value> {
    let callback = runtime_extras()?
        .query_open_editors
        .ok_or_else(|| anyhow!("Open-editors callback unavailable in this context"))?;
    callback().map_err(|err| anyhow!(err))
}

/// Switch focus to an already-open editor by its index returned from query_open_editors.
#[tool(category = "pulsar")]
pub fn activate_open_editor(index: i64) -> anyhow::Result<Value> {
    let index = usize::try_from(index).map_err(|_| anyhow!("activate_open_editor.index must be >= 0"))?;

    let callback = runtime_extras()?
        .activate_open_editor_request
        .ok_or_else(|| anyhow!("Activate-open-editor callback unavailable in this context"))?;
    callback(index).map_err(|err| anyhow!(err))?;

    Ok(json!({
        "ok": true,
        "index": index,
        "activated": true,
    }))
}

/// List all file types registered by installed plugins/editors.
#[tool(category = "pulsar")]
pub fn query_available_file_types() -> anyhow::Result<Value> {
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

/// Query which plugins/editors can handle a given file path.
#[tool(category = "pulsar")]
pub fn query_file_editors(file_path: String) -> anyhow::Result<Value> {
    let root = runtime_workspace_root()?;
    let full = resolve_workspace_path(&root, &file_path)?;

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

/// Discover AI tools available from plugins for a specific file. Use the file_path returned by query_open_editors - do not guess paths.
#[tool(category = "pulsar")]
pub fn query_plugin_tools(file_path: Option<String>) -> anyhow::Result<Value> {
    let file_path_raw = file_path.or_else(|| {
        runtime_current_file()
            .ok()
            .and_then(|p| p.map(|p| p.display().to_string()))
    });

    let Some(file_path_raw) = file_path_raw else {
        return Ok(json!({
            "ok": false,
            "error": "file_path is required. Provide the path of the file you want to query tools for.",
            "tools_available": 0,
            "tools": [],
            "plugins": [],
        }));
    };

    let root = runtime_workspace_root()?;
    let full = resolve_workspace_path_soft(&root, &file_path_raw)?;
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

/// List AI tools provided by a specific plugin, optionally scoped to a file.
#[tool(category = "pulsar")]
pub fn query_tools_for_plugin(plugin_id: String, file_path: Option<String>) -> anyhow::Result<Value> {
    let full = if let Some(file_path) = file_path {
        let root = runtime_workspace_root()?;
        Some(resolve_workspace_path_soft(&root, &file_path)?)
    } else {
        None
    };

    let manager_lock = plugin_manager::global()
        .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
    let manager = manager_lock
        .read()
        .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

    let bridge = if let Some(path) = full.as_ref() {
        manager.build_tool_bridge_for_file(path)
    } else {
        manager.build_tool_bridge()
    };

    let tool_schemas = bridge
        .all_tools()
        .into_iter()
        .filter(|tool| tool.plugin_id.to_string() == plugin_id)
        .map(|tool| {
            json!({
                "name": tool.definition.name,
                "description": tool.definition.description,
                "category": tool.definition.category,
                "parameters": tool.definition.parameters_json_schema,
                "plugin_id": tool.plugin_id.to_string(),
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "plugin_id": plugin_id,
        "file_path": full.as_ref().map(|p| p.display().to_string()),
        "tools_available": tool_schemas.len(),
        "tools": tool_schemas,
    }))
}

fn execute_plugin_tool_inner(
    tool_name: String,
    args: Value,
    plugin_id: Option<String>,
    full_file_path: PathBuf,
) -> anyhow::Result<Value> {
    let manager_lock = plugin_manager::global()
        .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
    let manager = manager_lock
        .read()
        .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

    // Resolve through the same file-scoped bridge used by query_plugin_tools so
    // execution matches file capabilities and plugin ownership for that file.
    let bridge = manager.build_tool_bridge_for_file(&full_file_path);
    let resolved_plugin_id = if let Some(explicit_plugin_id) = plugin_id.as_deref() {
        bridge
            .all_tools()
            .into_iter()
            .find(|tool| {
                tool.plugin_id.to_string() == explicit_plugin_id
                    && tool.definition.name == tool_name
            })
            .map(|tool| tool.plugin_id)
            .ok_or_else(|| anyhow!("Tool '{}' not found for plugin id '{}'", tool_name, explicit_plugin_id))?
    } else {
        let matches = bridge
            .all_tools()
            .into_iter()
            .filter(|tool| tool.definition.name == tool_name)
            .map(|tool| tool.plugin_id)
            .collect::<Vec<_>>();

        match matches.len() {
            0 => {
                return Err(anyhow!(
                    "Tool not found for file '{}': {}",
                    full_file_path.display(),
                    tool_name
                ));
            }
            1 => matches.into_iter().next().expect("len checked"),
            _ => {
                let plugin_ids = matches
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>();
                return Err(anyhow!(
                    "Ambiguous tool '{}'. Provide plugin_id. Candidates: {}",
                    tool_name,
                    plugin_ids.join(", ")
                ));
            }
        }
    };

    let result = bridge
        .execute_tool_direct(&tool_name, &full_file_path, args)
        .ok_or_else(|| {
            anyhow!(
                "No direct handler registered for tool '{}' (plugin '{}').",
                tool_name,
                resolved_plugin_id
            )
        })?
        .map_err(|err| anyhow!(err.to_string()))?;

    Ok(json!({
        "status": "ok",
        "plugin_id": resolved_plugin_id.to_string(),
        "tool_name": tool_name,
        "file_path": full_file_path.display().to_string(),
        "result": result,
    }))
}

/// Execute a plugin tool with explicit file context (preferred for LLM calls).
#[tool(category = "pulsar")]
pub fn call_plugin_tool(
    file_path: String,
    tool_name: String,
    args: Value,
    plugin_id: Option<String>,
) -> anyhow::Result<Value> {
    let root = runtime_workspace_root()?;
    let full_file_path = resolve_workspace_path_soft(&root, &file_path)?;
    execute_plugin_tool_inner(tool_name, args, plugin_id, full_file_path)
}

/// Execute an AI tool provided by a plugin. Back-compat wrapper around call_plugin_tool.
#[tool(category = "pulsar")]
pub fn execute_plugin_tool(
    tool_name: String,
    args: Value,
    plugin_id: Option<String>,
    file_path: Option<String>,
) -> anyhow::Result<Value> {
    let file_path = if let Some(path) = file_path {
        path
    } else {
        runtime_current_file()?
            .map(|p| p.display().to_string())
            .ok_or_else(|| anyhow!(
                "No file path provided. Prefer call_plugin_tool(file_path, tool_name, args, plugin_id)."
            ))?
    };

    call_plugin_tool(file_path, tool_name, args, plugin_id)
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
