use plugin_editor_api::{AiToolDefinition, PluginError};
use serde_json::{json, Value};
use std::path::Path;

use crate::ai_sessions;

fn is_level_file(file_path: &Path) -> bool {
    file_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.ends_with(".level") || name.ends_with(".level.json"))
        .unwrap_or(false)
}

fn open_state_for(file_path: &Path) -> Result<std::sync::Arc<parking_lot::RwLock<crate::level_editor::LevelEditorState>>, PluginError> {
    ai_sessions::get_open_scene_state(file_path).ok_or_else(|| PluginError::Other {
        message: format!(
            "Level is not open in editor: {}. Call open_file_in_default_editor first.",
            file_path.display()
        ),
    })
}

fn object_matches_filter(object: &crate::SceneObjectData, filter: Option<&Value>) -> bool {
    let Some(filter) = filter else {
        return true;
    };

    if let Some(id) = filter.get("id").and_then(|v| v.as_str()) {
        if object.id != id {
            return false;
        }
    }

    if let Some(name_contains) = filter.get("name_contains").and_then(|v| v.as_str()) {
        if !object.name.contains(name_contains) {
            return false;
        }
    }

    if let Some(visible) = filter.get("visible").and_then(|v| v.as_bool()) {
        if object.visible != visible {
            return false;
        }
    }

    true
}

fn vec3_from_value(value: Option<&Value>) -> Option<[f32; 3]> {
    let arr = value?.as_array()?;
    if arr.len() != 3 {
        return None;
    }
    Some([
        arr[0].as_f64()? as f32,
        arr[1].as_f64()? as f32,
        arr[2].as_f64()? as f32,
    ])
}

pub fn ai_tools() -> Vec<AiToolDefinition> {
    vec![
        AiToolDefinition::new(
            "level_editor_query_objects",
            "Query objects from the currently open level editor scene (in-memory).",
            json!({
                "type": "object",
                "properties": {
                    "filter": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "name_contains": { "type": "string" },
                            "visible": { "type": "boolean" }
                        }
                    },
                    "offset": { "type": "integer", "minimum": 0 },
                    "limit": { "type": "integer", "minimum": 1 }
                }
            }),
        )
        .with_category("analysis"),
        AiToolDefinition::new(
            "level_editor_bulk_update_objects",
            "Bulk-update objects in the currently open level editor scene (in-memory only).",
            json!({
                "type": "object",
                "properties": {
                    "filter": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "name_contains": { "type": "string" },
                            "visible": { "type": "boolean" }
                        }
                    },
                    "set": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "visible": { "type": "boolean" },
                            "locked": { "type": "boolean" },
                            "position": {
                                "type": "array",
                                "items": { "type": "number" },
                                "minItems": 3,
                                "maxItems": 3
                            },
                            "rotation": {
                                "type": "array",
                                "items": { "type": "number" },
                                "minItems": 3,
                                "maxItems": 3
                            },
                            "scale": {
                                "type": "array",
                                "items": { "type": "number" },
                                "minItems": 3,
                                "maxItems": 3
                            }
                        },
                        "additionalProperties": false
                    }
                },
                "required": ["set"]
            }),
        )
        .with_category("editing"),
        AiToolDefinition::new(
            "level_editor_bulk_delete_objects",
            "Bulk-delete objects from the currently open level editor scene (in-memory only).",
            json!({
                "type": "object",
                "properties": {
                    "filter": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "name_contains": { "type": "string" },
                            "visible": { "type": "boolean" }
                        }
                    }
                }
            }),
        )
        .with_category("editing"),
    ]
}

pub fn capabilities_for_file(file_path: &Path) -> Vec<String> {
    if !is_level_file(file_path) {
        return Vec::new();
    }

    ai_tools().into_iter().map(|t| t.name).collect()
}

pub fn execute_ai_tool(
    file_path: &Path,
    tool_name: &str,
    tool_args: Value,
) -> Result<Value, PluginError> {
    let state_arc = open_state_for(file_path)?;

    match tool_name {
        "level_editor_query_objects" => {
            let state = state_arc.read();
            let objects = state.scene_database.get_all_objects();
            let filter = tool_args.get("filter");
            let offset = tool_args
                .get("offset")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let limit = tool_args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(200) as usize;

            let matched = objects
                .iter()
                .filter(|object| object_matches_filter(object, filter))
                .collect::<Vec<_>>();

            let items = matched
                .iter()
                .skip(offset)
                .take(limit)
                .filter_map(|object| serde_json::to_value(object).ok())
                .collect::<Vec<_>>();

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "total_matches": matched.len(),
                "offset": offset,
                "limit": limit,
                "items": items,
            }))
        }
        "level_editor_bulk_update_objects" => {
            let set_obj = tool_args
                .get("set")
                .and_then(|v| v.as_object())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_bulk_update_objects requires a `set` object".to_string(),
                })?;
            let filter = tool_args.get("filter");

            let mut state = state_arc.write();
            let objects = state.scene_database.get_all_objects();
            let mut updated_ids = Vec::new();

            for mut object in objects {
                if !object_matches_filter(&object, filter) {
                    continue;
                }

                let mut changed = false;

                if let Some(name) = set_obj.get("name").and_then(|v| v.as_str()) {
                    if object.name != name {
                        object.name = name.to_string();
                        changed = true;
                    }
                }
                if let Some(visible) = set_obj.get("visible").and_then(|v| v.as_bool()) {
                    if object.visible != visible {
                        object.visible = visible;
                        changed = true;
                    }
                }
                if let Some(locked) = set_obj.get("locked").and_then(|v| v.as_bool()) {
                    if object.locked != locked {
                        object.locked = locked;
                        changed = true;
                    }
                }
                if let Some(position) = vec3_from_value(set_obj.get("position")) {
                    if object.transform.position != position {
                        object.transform.position = position;
                        changed = true;
                    }
                }
                if let Some(rotation) = vec3_from_value(set_obj.get("rotation")) {
                    if object.transform.rotation != rotation {
                        object.transform.rotation = rotation;
                        changed = true;
                    }
                }
                if let Some(scale) = vec3_from_value(set_obj.get("scale")) {
                    if object.transform.scale != scale {
                        object.transform.scale = scale;
                        changed = true;
                    }
                }

                if changed && state.scene_database.update_object(object.clone()) {
                    updated_ids.push(object.id.clone());
                }
            }

            if !updated_ids.is_empty() {
                state.has_unsaved_changes = true;
            }

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "updated_count": updated_ids.len(),
                "updated_ids": updated_ids,
            }))
        }
        "level_editor_bulk_delete_objects" => {
            let filter = tool_args.get("filter");

            let mut state = state_arc.write();
            let objects = state.scene_database.get_all_objects();
            let delete_ids = objects
                .iter()
                .filter(|object| object_matches_filter(object, filter))
                .map(|object| object.id.clone())
                .collect::<Vec<_>>();

            let mut deleted_ids = Vec::new();
            for object_id in delete_ids {
                if state.scene_database.remove_object(&object_id) {
                    deleted_ids.push(object_id);
                }
            }

            if !deleted_ids.is_empty() {
                state.has_unsaved_changes = true;
            }

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "deleted_count": deleted_ids.len(),
                "deleted_ids": deleted_ids,
            }))
        }
        _ => Err(PluginError::Other {
            message: format!("Unknown Level Editor AI tool: {tool_name}"),
        }),
    }
}
