//! The tool runtime state (workspace, current file, host callbacks) and how a `ToolContext` is made.

use anyhow::anyhow;
use engine_state::EngineContext;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tool_registry::ToolContext;

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
    pub(crate) workspace_root: PathBuf,
    current_file: Option<PathBuf>,
    pub(crate) extras: PulsarToolExtras,
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
    let ctx = EngineContext::global().ok_or_else(|| anyhow!("Engine not initialized"))?;
    let handle = ctx
        .store
        .get::<RuntimeState>()
        .ok_or_else(|| anyhow!("Tool runtime state is not initialized"))?;
    let state = handle.read().clone();
    Ok(state)
}

pub(crate) fn runtime_workspace_root() -> anyhow::Result<PathBuf> {
    Ok(runtime_state()?.workspace_root)
}

pub(crate) fn runtime_current_file() -> anyhow::Result<Option<PathBuf>> {
    Ok(runtime_state()?.current_file)
}

pub(crate) fn runtime_extras() -> anyhow::Result<PulsarToolExtras> {
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
        crate::fs_tools::insert_tooling_state(&mut ctx, root);
    }
    ctx.insert_extra(EXTRAS_KEY, extras);
    ctx
}
