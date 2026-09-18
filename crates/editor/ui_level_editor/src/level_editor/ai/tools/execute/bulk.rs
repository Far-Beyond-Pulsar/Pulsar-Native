use super::super::*;
use super::StateArc;

pub(super) fn dispatch(
    tool_name: &str,
    file_path: &Path,
    state_arc: &StateArc,
    tool_args: &Value,
) -> Result<Option<Result<Value, PluginError>>, PluginError> {
    match tool_name {
        "level_editor_bulk_update_objects" => {
            let set_obj = tool_args
                .get("set")
                .and_then(|v| v.as_object())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_bulk_update_objects requires a `set` object".to_string(),
                })?;
            let filter = tool_args.get("filter");

            let mut state = state_arc.write();
            let objects = crate::level_editor::scene_edit::objects::get_all_objects(&state.scene.world(), );
            let mut updated_ids = Vec::new();
            let mut matched_count = 0usize;

            for mut object in objects {
                if !object_matches_filter(&object, filter) {
                    continue;
                }
                matched_count += 1;

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

                if changed {
                    let id = object.id.clone();
                    let res =
                        execute_command(&mut state, SceneCommand::UpdateObject { data: object });
                    if res.changed {
                        updated_ids.push(id);
                    }
                }
            }

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "matched_count": matched_count,
                "updated_count": updated_ids.len(),
                "no_op": updated_ids.is_empty(),
                "no_op_reason": if updated_ids.is_empty() {
                    if matched_count == 0 {
                        "No objects matched the provided filter"
                    } else {
                        "Objects matched but no field values changed"
                    }
                } else {
                    ""
                },
                "updated_ids": updated_ids,
            }))
        }
        "level_editor_bulk_delete_objects" => {
            let filter = tool_args.get("filter");

            let mut state = state_arc.write();
            let objects = crate::level_editor::scene_edit::objects::get_all_objects(&state.scene.world(), );
            let delete_ids = objects
                .iter()
                .filter(|object| object_matches_filter(object, filter))
                .map(|object| object.id.clone())
                .collect::<Vec<_>>();
            let matched_count = delete_ids.len();

            let mut deleted_ids = Vec::new();
            for object_id in delete_ids {
                let res = execute_command(
                    &mut state,
                    SceneCommand::RemoveObject {
                        id: object_id.clone(),
                    },
                );
                if res.changed {
                    deleted_ids.push(object_id);
                }
            }

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "matched_count": matched_count,
                "deleted_count": deleted_ids.len(),
                "no_op": deleted_ids.is_empty(),
                "no_op_reason": if deleted_ids.is_empty() {
                    "No objects matched the provided filter"
                } else {
                    ""
                },
                "deleted_ids": deleted_ids,
            }))
        }
        _ => return Ok(None),
    }
    .map(|value| Some(Ok(value)))
}
