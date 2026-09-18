use super::super::*;
use super::StateArc;

pub(super) fn dispatch(
    tool_name: &str,
    file_path: &Path,
    state_arc: &StateArc,
    tool_args: &Value,
) -> Result<Option<Result<Value, PluginError>>, PluginError> {
    match tool_name {
        "level_editor_query_scene" => {
            let state = state_arc.read();
            let objects = crate::level_editor::scene_edit::objects::get_all_objects(&state.scene.world(), );
            let roots = crate::level_editor::scene_edit::objects::get_root_objects(&state.scene.world(), );
            let selected_object_id = crate::level_editor::scene_edit::objects::get_selected_object_id(&state.scene.world(), );

            let mut counts_by_type = std::collections::BTreeMap::new();
            for object in &objects {
                let key = object_type_key(&object.object_type).to_string();
                *counts_by_type.entry(key).or_insert(0usize) += 1;
            }

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "current_scene": state.scene.current_scene.as_ref().map(|p| p.display().to_string()),
                "has_unsaved_changes": state.scene.has_unsaved_changes,
                "editor_mode": format!("{:?}", state.scene.editor_mode),
                "object_count": objects.len(),
                "root_object_count": roots.len(),
                "selected_object_id": selected_object_id,
                "counts_by_type": counts_by_type,
            }))
        }
        "level_editor_query_objects" => {
            let state = state_arc.read();
            let objects = crate::level_editor::scene_edit::objects::get_all_objects(&state.scene.world(), );
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
        "level_editor_get_object" => {
            let object_id = tool_args
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_get_object requires `id`".to_string(),
                })?;

            let state = state_arc.read();
            let object = crate::level_editor::scene_edit::objects::get_object(&state.scene.world(), &object_id.to_string());

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "found": object.is_some(),
                "object": object,
            }))
        }
        "level_editor_query_selection" => {
            let state = state_arc.read();
            let selected_id = crate::level_editor::scene_edit::objects::get_selected_object_id(&state.scene.world(), );
            let selected_object = crate::level_editor::scene_edit::objects::get_selected_object(&state.scene.world(), );

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "selected_object_id": selected_id,
                "selected_object": selected_object,
            }))
        }
        "level_editor_select_object" => {
            let id_value = tool_args.get("id");
            let selection = match id_value {
                Some(Value::String(id)) => Some(id.clone()),
                Some(Value::Null) | None => None,
                _ => {
                    return Err(PluginError::Other {
                        message: "level_editor_select_object.id must be a string or null"
                            .to_string(),
                    })
                }
            };

            let mut state = state_arc.write();
            execute_command(
                &mut state,
                SceneCommand::SelectObject {
                    id: selection.clone(),
                },
            );

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "selected_object_id": selection,
            }))
        }
        _ => return Ok(None),
    }
    .map(|value| Some(Ok(value)))
}
