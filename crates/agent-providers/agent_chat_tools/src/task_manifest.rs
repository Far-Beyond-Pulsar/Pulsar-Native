//! The task list the agent keeps for itself, and its tools.

use anyhow::anyhow;
use engine_state::{EngineContext, ResourceHandle};
use serde_json::{json, Value};
use tool_registry_macros::tool;

#[derive(Clone, Debug, Default)]
pub(crate) struct TaskEntry {
    pub(crate) id: String,
    title: String,
    /// "pending" | "in_progress" | "done" | "blocked"
    pub(crate) status: String,
    notes: Option<String>,
}

#[derive(Default)]
pub(crate) struct TaskManifest {
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
