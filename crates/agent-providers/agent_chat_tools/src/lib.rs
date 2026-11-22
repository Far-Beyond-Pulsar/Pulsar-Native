//! The chat's tool registry: workspace file tools, editor and plugin tools, subagents and the task list.

/// The workspace file tools (`cd`, `list_files`, `tree`, ...): the chat's view of
/// the project through `engine_fs::virtual_fs`.
pub mod fs_tools;

mod plugin_tools;
mod subagent_tools;
mod subagents;
mod task_manifest;
mod tool_context;
mod workspace_paths;

pub use plugin_tools::*;
pub use subagent_tools::*;
pub use subagents::*;
pub use task_manifest::*;
pub use tool_context::*;

pub use tool_registry::{ChatTool, PluginToolRegistry, ToolContext, ToolRegistry};

pub fn build_default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    crate::fs_tools::register_tools(&mut registry);
    register_pulsar_tools(&mut registry);
    tool_registry_builtin::register_builtins(&mut registry);
    registry
}

pub fn register_pulsar_tools(registry: &mut ToolRegistry) {
    registry.merge_plugin(&PluginToolRegistry::from_namespace(module_path!()));
}
