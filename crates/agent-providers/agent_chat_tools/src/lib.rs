use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use std::time::Instant;
use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use engine_state::{EngineContext, ResourceHandle};
use tracing::debug;

pub use tool_registry::{ChatTool, PluginToolRegistry, ToolContext, ToolRegistry};
use tool_registry_macros::tool;

pub type OpenFileRequest = Arc<dyn Fn(PathBuf) -> Result<(), String> + Send + Sync>;
pub type QueryOpenEditorsRequest = Arc<dyn Fn() -> Result<Value, String> + Send + Sync>;
pub type ActivateOpenEditorRequest = Arc<dyn Fn(usize) -> Result<(), String> + Send + Sync>;
pub type SubagentExecutorRequest =
    Arc<dyn Fn(SubagentLlmRequest) -> Result<SubagentLlmResponse, String> + Send + Sync>;

#[derive(Clone, Debug)]
pub struct SubagentLlmRequest {
    pub subagent_id: String,
    pub name: String,
    pub task: String,
    pub model: Option<String>,
    pub instructions: Option<String>,
    pub workspace_root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct SubagentLlmResponse {
    pub provider_id: String,
    pub model_used: String,
    pub assistant_message: String,
    pub streamed_chunks: Vec<String>,
    pub raw_response: Value,
    pub child_transcript: Vec<Value>,
}

#[derive(Clone, Default)]
pub struct PulsarToolExtras {
    pub plugin_bridge: Option<Arc<RwLock<plugin_manager::PluginToolBridge>>>,
    pub open_file_request: Option<OpenFileRequest>,
    pub query_open_editors: Option<QueryOpenEditorsRequest>,
    pub activate_open_editor_request: Option<ActivateOpenEditorRequest>,
    pub subagent_executor: Option<SubagentExecutorRequest>,
}

const EXTRAS_KEY: &str = "pulsar_tool_extras";

#[derive(Clone)]
struct RuntimeState {
    workspace_root: PathBuf,
    current_file: Option<PathBuf>,
    extras: PulsarToolExtras,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubagentStatus {
    Pending,
    Running,
    Success,
    Error,
    Cancelled,
}

impl SubagentStatus {
    fn as_str(self) -> &'static str {
        match self {
            SubagentStatus::Pending => "pending",
            SubagentStatus::Running => "running",
            SubagentStatus::Success => "success",
            SubagentStatus::Error => "error",
            SubagentStatus::Cancelled => "cancelled",
        }
    }

    fn is_terminal(self) -> bool {
        matches!(
            self,
            SubagentStatus::Success | SubagentStatus::Error | SubagentStatus::Cancelled
        )
    }
}

#[derive(Clone, Debug)]
struct SubagentRecord {
    id: String,
    sequence: u64,
    name: String,
    task: String,
    model: String,
    instructions: String,
    workspace_root: String,
    status: SubagentStatus,
    created_at_ms: u64,
    started_at_ms: Option<u64>,
    finished_at_ms: Option<u64>,
    progress: f32,
    cancellation_requested: bool,
    result: Option<Value>,
    error: Option<String>,
    notified_to_main_agent: bool,
    execution_log: Vec<Value>,
}

#[derive(Default)]
struct SubagentStore {
    records: HashMap<String, SubagentRecord>,
    completion_queue: VecDeque<String>,
}

static SUBAGENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

// ── Task Manifest ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct TaskEntry {
    id: String,
    title: String,
    /// "pending" | "in_progress" | "done" | "blocked"
    status: String,
    notes: Option<String>,
}

#[derive(Default)]
struct TaskManifest {
    tasks: Vec<TaskEntry>,
}

fn task_manifest() -> ResourceHandle<TaskManifest> {
    EngineContext::global()
        .expect("EngineContext not initialized")
        .store
        .get_or_init::<TaskManifest>()
}

/// Returns a JSON snapshot of current tasks for injection into system messages.
pub fn get_task_manifest_snapshot() -> Vec<Value> {
    let store = task_manifest();
    let guard = store.read();
    guard
        .tasks
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "title": t.title,
                "status": t.status,
                "notes": t.notes,
            })
        })
        .collect()
}

/// Clears the task manifest — call when starting a fresh chat.
pub fn clear_task_manifest() {
    let store = task_manifest();
    store.write().tasks.clear();
}

/// Returns running/pending sub-agents for live context injection.
pub fn get_active_subagents_snapshot() -> Vec<Value> {
    let store = subagent_store();
    let guard = store.read();
    guard
        .records
        .values()
        .filter(|r| !r.status.is_terminal())
        .map(|r| {
            json!({
                "id": r.id,
                "name": r.name,
                "task": r.task,
                "status": r.status.as_str(),
                "progress": r.progress,
            })
        })
        .collect()
}

/// Returns the first ~800 chars of the sub-agent's final assistant message,
/// or `None` if the result is not yet available.
pub fn get_subagent_result_preview(subagent_id: &str) -> Option<String> {
    let store = subagent_store();
    let guard = store.read();
    guard.records.get(subagent_id).and_then(|r| {
        r.result.as_ref().and_then(|result| {
            result
                .get("assistant_message")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.chars().take(800).collect())
        })
    })
}

fn subagent_store() -> ResourceHandle<SubagentStore> {
    EngineContext::global()
        .expect("EngineContext not initialized")
        .store
        .get_or_init::<SubagentStore>()
}

fn now_ms_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn push_subagent_completion(store: &mut SubagentStore, id: &str) {
    if !store.completion_queue.iter().any(|queued| queued == id) {
        store.completion_queue.push_back(id.to_string());
    }
}

fn summarize_workspace(workspace_root: &Path) -> Value {
    let mut file_count = 0usize;
    let mut dir_count = 0usize;
    let mut sampled_paths = Vec::new();
    let mut stack = vec![workspace_root.to_path_buf()];
    let mut visited = 0usize;
    const MAX_VISITS: usize = 1600;
    const MAX_SAMPLES: usize = 24;

    while let Some(path) = stack.pop() {
        if visited >= MAX_VISITS {
            break;
        }
        visited += 1;

        let Ok(read_dir) = fs::read_dir(&path) else {
            continue;
        };

        for entry in read_dir.flatten() {
            let entry_path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };

            if meta.is_dir() {
                dir_count += 1;
                if visited < MAX_VISITS {
                    stack.push(entry_path.clone());
                }
            } else {
                file_count += 1;
            }

            if sampled_paths.len() < MAX_SAMPLES {
                sampled_paths.push(entry_path.display().to_string());
            }
        }
    }

    json!({
        "workspace_root": workspace_root.display().to_string(),
        "visited_entries": visited,
        "file_count_estimate": file_count,
        "dir_count_estimate": dir_count,
        "sampled_paths": sampled_paths,
        "scan_limited": visited >= MAX_VISITS,
    })
}

fn launch_subagent_worker(
    subagent_id: String,
    workspace_root: PathBuf,
    subagent_executor: Option<SubagentExecutorRequest>,
) {
    thread::spawn(move || {
        let now = now_ms_u64();
        let mut name_snapshot = String::new();
        let mut task_snapshot = String::new();
        let mut model_snapshot = String::new();
        let mut instructions_snapshot = String::new();

        {
            let store = subagent_store();
            let mut guard = store.write();
            if let Some(record) = guard.records.get_mut(&subagent_id) {
                record.status = SubagentStatus::Running;
                record.started_at_ms = Some(now);
                record.progress = 0.05;
                name_snapshot = record.name.clone();
                task_snapshot = record.task.clone();
                model_snapshot = record.model.clone();
                instructions_snapshot = record.instructions.clone();
                record.execution_log.push(json!({
                    "at_ms": now,
                    "event": "worker_started",
                    "subagent_id": subagent_id.clone(),
                    "sequence": record.sequence,
                }));
            }
        }

        let checkpoints = [0.25_f32, 0.55_f32, 0.85_f32];
        for progress in checkpoints {
            thread::sleep(Duration::from_millis(350));

            let mut should_exit = false;
            {
                let store = subagent_store();
                let mut guard = store.write();
                if let Some(record) = guard.records.get_mut(&subagent_id) {
                    if record.cancellation_requested {
                        record.status = SubagentStatus::Cancelled;
                        record.progress = progress;
                        record.finished_at_ms = Some(now_ms_u64());
                        record.execution_log.push(json!({
                            "at_ms": record.finished_at_ms,
                            "event": "cancelled",
                            "progress": progress,
                        }));
                        record.result = Some(json!({
                            "status": "cancelled",
                            "message": "Subagent execution cancelled before completion.",
                            "execution_log": record.execution_log,
                        }));
                        push_subagent_completion(&mut guard, &subagent_id);
                        should_exit = true;
                    } else {
                        record.progress = progress;
                        record.execution_log.push(json!({
                            "at_ms": now_ms_u64(),
                            "event": "progress",
                            "progress": progress,
                        }));
                    }
                } else {
                    should_exit = true;
                }
            }

            if should_exit {
                return;
            }
        }

        // Dispatch the actual subagent task to a provider-backed executor when available.
        let llm_result = if let Some(executor) = subagent_executor {
            executor(SubagentLlmRequest {
                subagent_id: subagent_id.clone(),
                name: name_snapshot.clone(),
                task: task_snapshot.clone(),
                model: if model_snapshot.trim().is_empty() {
                    None
                } else {
                    Some(model_snapshot.clone())
                },
                instructions: Some(instructions_snapshot.clone()),
                workspace_root: workspace_root.clone(),
            })
        } else {
            Err("Subagent executor unavailable in this context".to_string())
        };

        let finished_at = now_ms_u64();
        {
            let store = subagent_store();
            let mut guard = store.write();
            if let Some(record) = guard.records.get_mut(&subagent_id) {
                if record.cancellation_requested {
                    record.status = SubagentStatus::Cancelled;
                    record.finished_at_ms = Some(finished_at);
                    record.progress = 1.0;
                    record.execution_log.push(json!({
                        "at_ms": finished_at,
                        "event": "cancelled_final",
                    }));
                    record.result = Some(json!({
                        "status": "cancelled",
                        "message": "Subagent execution cancelled.",
                        "execution_log": record.execution_log,
                    }));
                } else {
                    let workspace_summary = summarize_workspace(&workspace_root);
                    record.execution_log.push(json!({
                        "at_ms": finished_at,
                        "event": "workspace_scan_complete",
                        "file_count_estimate": workspace_summary
                            .get("file_count_estimate")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        "dir_count_estimate": workspace_summary
                            .get("dir_count_estimate")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                    }));

                    match llm_result {
                        Ok(response) => {
                            record.status = SubagentStatus::Success;
                            record.finished_at_ms = Some(finished_at);
                            record.progress = 1.0;
                            record.execution_log.push(json!({
                                "at_ms": finished_at,
                                "event": "llm_completed",
                                "provider_id": response.provider_id,
                                "model_used": response.model_used,
                                "chunk_count": response.streamed_chunks.len(),
                            }));
                            // Only keep the assistant's final answer and the
                            // assistant-only steps from the child transcript.
                            // Stripping streamed_chunks, raw_response, tool results,
                            // and workspace scan so the outer agent isn't overwhelmed.
                            let assistant_steps: Vec<Value> = response
                                .child_transcript
                                .iter()
                                .filter(|m| {
                                    m.get("role").and_then(|v| v.as_str()) == Some("assistant")
                                        && m.get("content")
                                            .and_then(|v| v.as_str())
                                            .map(|s| !s.trim().is_empty())
                                            .unwrap_or(false)
                                })
                                .cloned()
                                .collect();
                            record.result = Some(json!({
                                "answer": response.assistant_message,
                                "model_used": response.model_used,
                                "assistant_steps": assistant_steps,
                            }));
                        }
                        Err(err) => {
                            record.status = SubagentStatus::Error;
                            record.finished_at_ms = Some(finished_at);
                            record.progress = 1.0;
                            record.error = Some(err.clone());
                            record.execution_log.push(json!({
                                "at_ms": finished_at,
                                "event": "llm_error",
                                "error": err,
                            }));
                            record.result = Some(json!({
                                "answer": null,
                                "error": record.error,
                            }));
                        }
                    }
                }
                push_subagent_completion(&mut guard, &subagent_id);
            }
        }
    });
}

pub fn dequeue_subagent_completion_event() -> Option<Value> {
    let store = subagent_store();
    let mut guard = store.write();

    let id = guard.completion_queue.pop_front()?;
    let mut event = None;
    if let Some(record) = guard.records.get_mut(&id) {
        record.notified_to_main_agent = true;
        event = Some(json!({
            "subagent_id": record.id,
            "sequence": record.sequence,
            "name": record.name,
            "task": record.task,
            "status": record.status.as_str(),
            "created_at_ms": record.created_at_ms,
            "started_at_ms": record.started_at_ms,
            "finished_at_ms": record.finished_at_ms,
            "progress": record.progress,
            "result_available": record.result.is_some(),
            "error": record.error,
            "execution_events": record.execution_log.len(),
        }));
    }
    event
}

pub fn queued_subagent_completion_count() -> usize {
    let store = subagent_store();
    let guard = store.read();
    guard.completion_queue.len()
}

fn set_runtime_state(
    workspace_root: PathBuf,
    current_file: Option<PathBuf>,
    extras: PulsarToolExtras,
) {
    EngineContext::global()
        .expect("EngineContext not initialized")
        .store
        .insert(RuntimeState {
            workspace_root,
            current_file,
            extras,
        });
}

fn runtime_state() -> anyhow::Result<RuntimeState> {
    let ctx = EngineContext::global()
        .ok_or_else(|| anyhow!("Engine not initialized"))?;
    let handle = ctx
        .store
        .get::<RuntimeState>()
        .ok_or_else(|| anyhow!("Tool runtime state is not initialized"))?;
    let state = handle.read().clone();
    Ok(state)
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

// ── Task Manifest Tools ──────────────────────────────────────────────────────

/// Read the current task list. Returns all planned, in-progress, and completed tasks.
/// Use this to check your own progress and orient yourself after context compaction.
#[tool(category = "pulsar")]
pub fn task_list_read() -> anyhow::Result<Value> {
    let tasks = get_task_manifest_snapshot();
    Ok(json!({
        "ok": true,
        "count": tasks.len(),
        "tasks": tasks,
    }))
}

/// Replace the task list with a new set of tasks. Pass a JSON array where each
/// element has: id (string), title (string),
/// status ("pending"|"in_progress"|"done"|"blocked"), notes (optional string).
/// The list is injected into every provider call so orientation is never lost.
#[tool(category = "pulsar")]
pub fn task_list_update(tasks: Value) -> anyhow::Result<Value> {
    let arr = tasks
        .as_array()
        .ok_or_else(|| anyhow!("tasks must be a JSON array"))?;

    let mut entries = Vec::new();
    for (i, t) in arr.iter().enumerate() {
        let id = t
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let title = t
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if title.is_empty() {
            continue;
        }
        let status = t
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending")
            .to_string();
        let notes = t
            .get("notes")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let final_id = if id.is_empty() {
            format!("task-{}", i + 1)
        } else {
            id
        };
        entries.push(TaskEntry {
            id: final_id,
            title,
            status,
            notes,
        });
    }

    let count = entries.len();
    let store = task_manifest();
    store.write().tasks = entries;

    Ok(json!({ "ok": true, "count": count }))
}

// ── Editor / File Tools ──────────────────────────────────────────────────────

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
    let index =
        usize::try_from(index).map_err(|_| anyhow!("activate_open_editor.index must be >= 0"))?;

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
    let manager_lock =
        plugin_manager::global().ok_or_else(|| anyhow!("Global plugin manager not available"))?;
    let manager = manager_lock.read();

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

    let manager_lock =
        plugin_manager::global().ok_or_else(|| anyhow!("Global plugin manager not available"))?;
    let manager = manager_lock.read();

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

    let manager_lock =
        plugin_manager::global().ok_or_else(|| anyhow!("Global plugin manager not available"))?;
    let manager = manager_lock.read();

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
pub fn query_tools_for_plugin(
    plugin_id: String,
    file_path: Option<String>,
) -> anyhow::Result<Value> {
    let full = if let Some(file_path) = file_path {
        let root = runtime_workspace_root()?;
        Some(resolve_workspace_path_soft(&root, &file_path)?)
    } else {
        None
    };

    let manager_lock =
        plugin_manager::global().ok_or_else(|| anyhow!("Global plugin manager not available"))?;
    let manager = manager_lock.read();

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
    let started_at = Instant::now();
    debug!(tool = tool_name.as_str(), file = %full_file_path.display(), plugin_id = ?plugin_id, "call_plugin_tool start");
    let manager_lock =
        plugin_manager::global().ok_or_else(|| anyhow!("Global plugin manager not available"))?;
    let manager = manager_lock.read();

    // Resolve through the same file-scoped bridge used by query_plugin_tools so
    // execution matches file capabilities and plugin ownership for that file.
    debug!(tool = tool_name.as_str(), file = %full_file_path.display(), "building tool bridge for file");
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
            .ok_or_else(|| {
                anyhow!(
                    "Tool '{}' not found for plugin id '{}'",
                    tool_name,
                    explicit_plugin_id
                )
            })?
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

    debug!(tool = tool_name.as_str(), file = %full_file_path.display(), plugin_id = %resolved_plugin_id, elapsed_ms = started_at.elapsed().as_millis() as u64, "call_plugin_tool end");

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
    debug!(tool = tool_name.as_str(), file = %full_file_path.display(), plugin_id = ?plugin_id, "call_plugin_tool resolved file path");
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

/// Spawn a new subagent to handle a specific task or analysis.
/// Returns a subagent ID for tracking and retrieving results.
#[tool(category = "subagent")]
pub fn spawn_subagent(
    name: String,
    task: String,
    model: Option<String>,
    instructions: Option<String>,
) -> anyhow::Result<Value> {
    debug!(
        "spawn_subagent start name={} task={} model={:?}",
        name, task, model
    );
    let created_at_ms = now_ms_u64();
    let sequence = SUBAGENT_SEQUENCE.fetch_add(1, Ordering::SeqCst);
    let entropy = ((created_at_ms as u32).wrapping_mul(2654435761)) ^ (sequence as u32);
    let subagent_id = format!("subagent-{created_at_ms}-{sequence}-{entropy:08x}");
    let selected_model = model
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let selected_instructions = instructions.unwrap_or_default();
    let workspace_root = runtime_workspace_root().unwrap_or_else(|_| PathBuf::from("."));
    let subagent_executor = runtime_extras()
        .ok()
        .and_then(|extras| extras.subagent_executor);
    let executor_ready = subagent_executor.is_some();

    let record = SubagentRecord {
        id: subagent_id.clone(),
        sequence,
        name: name.clone(),
        task: task.clone(),
        model: selected_model.clone().unwrap_or_default(),
        instructions: selected_instructions.clone(),
        workspace_root: workspace_root.display().to_string(),
        status: SubagentStatus::Pending,
        created_at_ms,
        started_at_ms: None,
        finished_at_ms: None,
        progress: 0.0,
        cancellation_requested: false,
        result: None,
        error: None,
        notified_to_main_agent: false,
        execution_log: vec![json!({
            "at_ms": created_at_ms,
            "event": "spawn_requested",
            "sequence": sequence,
            "workspace_root": workspace_root.display().to_string(),
            "model": selected_model.clone(),
        })],
    };

    let store = subagent_store();
    store.write().records.insert(subagent_id.clone(), record);

    launch_subagent_worker(
        subagent_id.clone(),
        workspace_root.clone(),
        subagent_executor,
    );

    debug!("spawn_subagent end subagent_id={}", subagent_id);

    Ok(json!({
        "ok": true,
        "subagent_id": subagent_id,
        "name": name,
        "task": task,
        "model": selected_model,
        "instructions": selected_instructions,
        "status": "spawned",
        "created_at_ms": created_at_ms,
        "sequence": sequence,
        "workspace_root": workspace_root.display().to_string(),
        "executor": if executor_ready {
            "provider_backed_subagent"
        } else {
            "subagent_worker_thread"
        },
        "executor_ready": executor_ready,
    }))
}

/// Query the status of running or completed subagents.
/// Returns list of subagent IDs and their current status.
#[tool(category = "subagent")]
pub fn query_running_subagents() -> anyhow::Result<Value> {
    debug!("query_running_subagents start");

    let store = subagent_store();
    let guard = store.read();
    let mut running_count = 0usize;
    let mut queued_count = 0usize;
    let mut completed_count = 0usize;

    let mut subagents = guard
        .records
        .values()
        .map(|record| {
            match record.status {
                SubagentStatus::Running => running_count += 1,
                SubagentStatus::Pending => queued_count += 1,
                SubagentStatus::Success | SubagentStatus::Error | SubagentStatus::Cancelled => {
                    completed_count += 1
                }
            }

            json!({
                "id": record.id,
                "sequence": record.sequence,
                "name": record.name,
                "task": record.task,
                "status": record.status.as_str(),
                "progress": record.progress,
                "created_at_ms": record.created_at_ms,
                "started_at_ms": record.started_at_ms,
                "finished_at_ms": record.finished_at_ms,
                "result_available": record.result.is_some(),
                "notified_to_main_agent": record.notified_to_main_agent,
                "error": record.error,
                "execution_events": record.execution_log.len(),
            })
        })
        .collect::<Vec<_>>();

    subagents.sort_by(|a, b| {
        let a_seq = a.get("sequence").and_then(|v| v.as_u64()).unwrap_or(0);
        let b_seq = b.get("sequence").and_then(|v| v.as_u64()).unwrap_or(0);
        a_seq.cmp(&b_seq)
    });

    debug!("query_running_subagents end");

    Ok(json!({
        "ok": true,
        "count": subagents.len(),
        "running": running_count,
        "queued": queued_count,
        "completed": completed_count,
        "completion_queue_depth": guard.completion_queue.len(),
        "subagents": subagents,
    }))
}

/// Get the result from a completed subagent.
/// The subagent ID should come from spawn_subagent or query_running_subagents.
#[tool(category = "subagent")]
pub fn get_subagent_result(subagent_id: String) -> anyhow::Result<Value> {
    debug!("get_subagent_result start subagent_id={}", subagent_id);

    let store = subagent_store();
    let guard = store.read();
    let Some(record) = guard.records.get(&subagent_id) else {
        return Err(anyhow!("Subagent {} not found", subagent_id));
    };

    if !record.status.is_terminal() {
        return Ok(json!({
            "ok": true,
            "subagent_id": record.id,
            "status": record.status.as_str(),
            "progress": record.progress,
            "result": null,
            "message": "Subagent still running. Wait for a completion notification or poll again.",
        }));
    }

    debug!("get_subagent_result end subagent_id={}", subagent_id);

    // Extract just what the outer agent needs: the task, the answer, and any
    // intermediate assistant reasoning steps. Metadata (timestamps, sequences,
    // execution logs, workspace scans) is omitted — it only creates noise.
    let answer = record
        .result
        .as_ref()
        .and_then(|r| r.get("answer"))
        .cloned()
        .unwrap_or(Value::Null);

    let assistant_steps = record
        .result
        .as_ref()
        .and_then(|r| r.get("assistant_steps"))
        .cloned()
        .unwrap_or(Value::Array(vec![]));

    Ok(json!({
        "ok": true,
        "name": record.name,
        "task": record.task,
        "status": record.status.as_str(),
        "answer": answer,
        "error": record.error,
        "assistant_steps": assistant_steps,
    }))
}

/// Cancel a running subagent by its ID.
/// Returns success if the subagent was cancelled, or an error if not found/already complete.
#[tool(category = "subagent")]
pub fn cancel_subagent(subagent_id: String) -> anyhow::Result<Value> {
    debug!("cancel_subagent start subagent_id={}", subagent_id);

    let store = subagent_store();
    let mut guard = store.write();
    let Some(record) = guard.records.get_mut(&subagent_id) else {
        return Err(anyhow!("Subagent {} not found", subagent_id));
    };

    if record.status.is_terminal() {
        return Ok(json!({
            "ok": true,
            "subagent_id": subagent_id,
            "status": record.status.as_str(),
            "cancelled": false,
            "message": "Subagent already reached a terminal state.",
        }));
    }

    record.cancellation_requested = true;

    debug!("cancel_subagent end subagent_id={}", subagent_id);

    Ok(json!({
        "ok": true,
        "subagent_id": subagent_id,
        "status": "cancellation_requested",
        "cancelled": true,
    }))
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
