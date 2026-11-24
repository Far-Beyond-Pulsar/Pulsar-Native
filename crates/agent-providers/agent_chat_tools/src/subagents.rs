//! Subagent records, their store, the worker thread that runs one, and the live snapshots the chat reads.

use engine_state::{EngineContext, ResourceHandle};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::tool_context::{SubagentExecutorRequest, SubagentLlmRequest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubagentStatus {
    Pending,
    Running,
    Success,
    Error,
    Cancelled,
}

impl SubagentStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SubagentStatus::Pending => "pending",
            SubagentStatus::Running => "running",
            SubagentStatus::Success => "success",
            SubagentStatus::Error => "error",
            SubagentStatus::Cancelled => "cancelled",
        }
    }

    pub(crate) fn is_terminal(self) -> bool {
        matches!(
            self,
            SubagentStatus::Success | SubagentStatus::Error | SubagentStatus::Cancelled
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SubagentRecord {
    pub(crate) id: String,
    pub(crate) sequence: u64,
    pub(crate) name: String,
    pub(crate) task: String,
    pub(crate) model: String,
    pub(crate) instructions: String,
    pub(crate) status: SubagentStatus,
    pub(crate) created_at_ms: u64,
    pub(crate) started_at_ms: Option<u64>,
    pub(crate) finished_at_ms: Option<u64>,
    pub(crate) progress: f32,
    pub(crate) cancellation_requested: bool,
    pub(crate) result: Option<Value>,
    pub(crate) error: Option<String>,
    pub(crate) notified_to_main_agent: bool,
    pub(crate) execution_log: Vec<Value>,
}

#[derive(Default)]
pub(crate) struct SubagentStore {
    pub(crate) records: HashMap<String, SubagentRecord>,
    pub(crate) completion_queue: VecDeque<String>,
}

pub(crate) static SUBAGENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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

pub(crate) fn subagent_store() -> ResourceHandle<SubagentStore> {
    EngineContext::global()
        .expect("EngineContext not initialized")
        .store
        .get_or_init::<SubagentStore>()
}

pub(crate) fn now_ms_u64() -> u64 {
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

        let Ok(entries) = engine_fs::virtual_fs::list_dir(&path) else {
            continue;
        };

        for entry in entries {
            let entry_path = path.join(&entry.name);

            if entry.is_dir {
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

pub(crate) fn launch_subagent_worker(
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
