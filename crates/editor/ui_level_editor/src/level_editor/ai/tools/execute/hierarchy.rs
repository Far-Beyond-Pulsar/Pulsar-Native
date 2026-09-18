use super::super::*;
use super::StateArc;

pub(super) fn dispatch(
    tool_name: &str,
    file_path: &Path,
    state_arc: &StateArc,
    tool_args: &Value,
) -> Result<Option<Result<Value, PluginError>>, PluginError> {
    match tool_name {
        "level_editor_query_children" => {
            let parent_value = tool_args.get("parent_id");
            let parent_id = match parent_value {
                Some(Value::String(id)) => Some(id.clone()),
                Some(Value::Null) | None => None,
                _ => {
                    return Err(PluginError::Other {
                        message: "level_editor_query_children.parent_id must be a string or null"
                            .to_string(),
                    })
                }
            };

            let state = state_arc.read();

            let (child_ids, child_objects) = if let Some(ref parent_id) = parent_id {
                let ids = state.scene.database.get_children(parent_id);
                let objects = ids
                    .iter()
                    .filter_map(|id| state.scene.database.get_object(id))
                    .collect::<Vec<_>>();
                (ids, objects)
            } else {
                let objects = state.scene.database.get_root_objects();
                let ids = objects.iter().map(|o| o.id.clone()).collect::<Vec<_>>();
                (ids, objects)
            };

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "parent_id": parent_id,
                "count": child_ids.len(),
                "child_ids": child_ids,
                "children": child_objects,
            }))
        }
        "level_editor_reparent_object" => {
            let object_id = tool_args
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_reparent_object requires `id`".to_string(),
                })?
                .to_string();

            let new_parent_value = tool_args.get("new_parent_id");
            let new_parent_id =
                match new_parent_value {
                    Some(Value::String(id)) => Some(id.clone()),
                    Some(Value::Null) | None => None,
                    _ => return Err(PluginError::Other {
                        message:
                            "level_editor_reparent_object.new_parent_id must be a string or null"
                                .to_string(),
                    }),
                };

            let mut state = state_arc.write();
            let result = execute_command(
                &mut state,
                SceneCommand::ReparentObject {
                    id: object_id.clone(),
                    new_parent_id: new_parent_id.clone(),
                },
            );
            let moved = result.changed;

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "object_id": object_id,
                "new_parent_id": new_parent_id,
                "moved": moved,
                "no_op": !moved,
                "no_op_reason": if moved { "" } else { "Object not found, invalid parent, or reparent was rejected" },
            }))
        }
        "level_editor_duplicate_object" => {
            let source_id = tool_args
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_duplicate_object requires `id`".to_string(),
                })?
                .to_string();

            let count = tool_args
                .get("count")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .min(100) as usize;

            let position_offset = vec3_from_value(tool_args.get("position_offset"));

            let mut state = state_arc.write();
            let result = execute_command(
                &mut state,
                SceneCommand::DuplicateObject {
                    source_id: source_id.clone(),
                    count,
                    position_offset,
                },
            );

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "source_id": source_id,
                "requested_count": count,
                "created_count": result.affected_ids.len(),
                "created_ids": result.affected_ids,
                "position_offset_applied": position_offset.is_some(),
                "no_op": !result.changed,
                "no_op_reason": result.no_op_reason,
            }))
        }
        _ => return Ok(None),
    }
    .map(|value| Some(Ok(value)))
}
