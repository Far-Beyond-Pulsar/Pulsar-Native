//! Tool execution: dispatch an AI tool call into `SceneCommand` mutations.

use super::*;
use super::registry::tool_registry as editor_tool_registry;

pub fn execute_ai_tool(
    file_path: &Path,
    tool_name: &str,
    tool_args: Value,
) -> Result<Value, PluginError> {
    let ctx = ToolContext::new().with_current_file(file_path);
    editor_tool_registry()
        .execute(tool_name, tool_args, &ctx)
        .map_err(|err| PluginError::Other {
            message: err.to_string(),
        })
}

pub(super) type StateArc =
    std::sync::Arc<parking_lot::RwLock<crate::level_editor::LevelEditorState>>;

pub(super) fn execute_ai_tool_impl(
    file_path: &Path,
    tool_name: &str,
    tool_args: Value,
) -> Result<Value, PluginError> {
    let state_arc = open_state_for(file_path)?;

    if let Some(result) = queries::dispatch(tool_name, file_path, &state_arc, &tool_args)? {
        return result;
    }
    if let Some(result) = objects::dispatch(tool_name, file_path, &state_arc, &tool_args)? {
        return result;
    }
    if let Some(result) = hierarchy::dispatch(tool_name, file_path, &state_arc, &tool_args)? {
        return result;
    }
    if let Some(result) = mutations::dispatch(tool_name, file_path, &state_arc, &tool_args)? {
        return result;
    }
    if let Some(result) = bulk::dispatch(tool_name, file_path, &state_arc, &tool_args)? {
        return result;
    }

    Err(PluginError::Other {
        message: format!("Unknown Level Editor AI tool: {tool_name}"),
    })
}

mod bulk;
mod hierarchy;
mod mutations;
mod objects;
mod queries;
