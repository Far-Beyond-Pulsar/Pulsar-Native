//! Tools that spawn, list, read and cancel subagents.

use anyhow::anyhow;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tool_registry_macros::tool;
use tracing::debug;

use crate::subagents::{
    launch_subagent_worker, now_ms_u64, subagent_store, SubagentRecord, SubagentStatus,
    SUBAGENT_SEQUENCE,
};
use crate::tool_context::{runtime_extras, runtime_workspace_root};

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
