//! The editor's existing AI tools, through the protocol, unchanged.
//!
//! The chat panel's tools are the plugin manager's: every loaded plugin's
//! `ai_tools()` and every built-in provider's (the level editor's scene tools, the
//! Blueprint editor's). [`PluginToolBridge`](plugin_manager::tool_bridge::PluginToolBridge)
//! lists them with their JSON schemas and runs them against a file context. This
//! provider lists that same set (rebuilt on each `tools/list`, so a plugin that
//! loads later appears) and runs a call through the bridge, so the chat panel and an
//! external client use one set of tools with one behaviour.
//!
//! The bridge runs a tool against a file: the level or asset it acts on. A call
//! may pass `file_path` for that; without it the tool acts on the editor that is
//! open (the level editor picks its only open level), which is what a call that
//! names no file means in the chat panel too.

use std::path::PathBuf;
use std::sync::Arc;

use pulsar_mcp::{Annotations, ToolInfo, ToolProvider, ToolResult};
use serde_json::{Value, json};

struct AiTools;

/// What a tool name says about its effects. The tools carry no annotations of
/// their own, so this is a conservative reading of the verb: an unknown verb is
/// treated as a write.
fn annotations_for(name: &str) -> Annotations {
    let words: Vec<&str> = name.split('_').collect();
    let has = |verbs: &[&str]| words.iter().any(|word| verbs.contains(word));
    if has(&["delete", "remove", "destroy", "overwrite", "save", "clear"]) {
        Annotations::DESTRUCTIVE
    } else if has(&["list", "get", "query", "describe", "find", "read", "search", "inspect"]) {
        Annotations::READ_ONLY
    } else {
        Annotations::WRITES
    }
}

fn file_context() -> PathBuf {
    // A path inside the project that names no existing file: the tool picks the open
    // editor, and the bridge's file-system check (which wants the path inside the
    // project) allows it.
    engine_state::get_project_path()
        .map(|root| PathBuf::from(root).join(".pulsar").join("mcp-call"))
        .unwrap_or_default()
}

impl ToolProvider for AiTools {
    fn name(&self) -> &str {
        "ai-tools"
    }

    fn list(&self) -> Vec<ToolInfo> {
        let Some(manager) = plugin_manager::global() else {
            return Vec::new();
        };
        let bridge = manager.read().build_tool_bridge();
        bridge
            .all_tools()
            .into_iter()
            .map(|tool| {
                let mut schema = tool.definition.parameters_json_schema.clone();
                if !schema.is_object() {
                    schema = json!({ "type": "object", "properties": {} });
                }
                if schema.get("type").is_none() {
                    schema["type"] = json!("object");
                }
                // The file the tool acts on, when the tool does not name its own.
                let properties = schema
                    .as_object_mut()
                    .map(|object| {
                        object
                            .entry("properties")
                            .or_insert_with(|| json!({}))
                    })
                    .and_then(Value::as_object_mut);
                if let Some(properties) = properties {
                    properties.entry("file_path").or_insert_with(|| {
                        json!({
                            "type": "string",
                            "description": "Optional: the level or asset file this call is for (to pick one of several open editors). Omit to act on the open editor."
                        })
                    });
                }
                ToolInfo {
                    annotations: annotations_for(&tool.definition.name),
                    title: tool.definition.name.clone(),
                    description: tool.definition.description.clone(),
                    input_schema: schema,
                    source: tool.plugin_id.to_string(),
                    name: tool.definition.name,
                }
            })
            .collect()
    }

    fn call(&self, tool: &str, arguments: &str) -> Option<ToolResult> {
        let manager = plugin_manager::global()?;
        let bridge = manager.read().build_tool_bridge();
        bridge.all_tools().iter().find(|t| t.definition.name == tool)?;
        let mut arguments: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
        // `file_path` is ours unless the tool's own schema declares it.
        let declared = bridge
            .all_tools()
            .iter()
            .find(|t| t.definition.name == tool)
            .is_some_and(|t| {
                t.definition.parameters_json_schema["properties"]
                    .get("file_path")
                    .is_some()
            });
        let file = match arguments.get("file_path").and_then(Value::as_str) {
            Some(path) if !declared => {
                let path = PathBuf::from(path);
                if let Some(object) = arguments.as_object_mut() {
                    object.remove("file_path");
                }
                path
            }
            _ => file_context(),
        };
        let outcome = bridge.execute_tool_direct(tool, &file, arguments)?;
        Some(match outcome {
            Ok(value) => {
                let mut result = ToolResult::json(&value);
                if !value.is_object() {
                    // Structured content is an object; a bare value is text only.
                    result.structured = None;
                }
                result
            }
            Err(error) => ToolResult::error(error.to_string()),
        })
    }
}

pub(crate) fn install() {
    pulsar_mcp::add_provider(Arc::new(AiTools));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_are_read_from_the_verb() {
        assert_eq!(annotations_for("level_editor_list_objects"), Annotations::READ_ONLY);
        assert_eq!(annotations_for("level_editor_delete_objects"), Annotations::DESTRUCTIVE);
        assert_eq!(annotations_for("level_editor_save_scene"), Annotations::DESTRUCTIVE);
        assert_eq!(annotations_for("level_editor_spawn_object"), Annotations::WRITES);
        assert_eq!(annotations_for("level_editor_set_transform"), Annotations::WRITES);
    }
}
