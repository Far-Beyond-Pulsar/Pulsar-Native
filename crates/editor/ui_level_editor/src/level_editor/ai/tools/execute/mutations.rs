use super::super::*;
use super::StateArc;

pub(super) fn dispatch(
    tool_name: &str,
    file_path: &Path,
    state_arc: &StateArc,
    tool_args: &Value,
) -> Result<Option<Result<Value, PluginError>>, PluginError> {
    match tool_name {
        "level_editor_remove_object" => {
            let object_id = tool_args
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_remove_object requires `id`".to_string(),
                })?
                .to_string();

            let mut state = state_arc.write();
            let result = execute_command(
                &mut state,
                SceneCommand::RemoveObject {
                    id: object_id.clone(),
                },
            );

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "removed": result.changed,
                "removed_id": if result.changed { Some(object_id) } else { None::<String> },
                "no_op": !result.changed,
                "no_op_reason": result.no_op_reason,
            }))
        }
        "level_editor_set_transform" => {
            let object_id = tool_args
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_set_transform requires `id`".to_string(),
                })?
                .to_string();

            let mut state = state_arc.write();
            let result = execute_command(
                &mut state,
                SceneCommand::SetTransform {
                    id: object_id.clone(),
                    position: vec3_from_value(tool_args.get("position")),
                    rotation: vec3_from_value(tool_args.get("rotation")),
                    scale: vec3_from_value(tool_args.get("scale")),
                },
            );

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "id": object_id,
                "updated": result.changed,
                "no_op": !result.changed,
                "no_op_reason": result.no_op_reason,
            }))
        }
        "level_editor_update_object" => {
            let object_id = tool_args
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_update_object requires `id`".to_string(),
                })?
                .to_string();

            let mut state = state_arc.write();
            let Some(mut object) = state.scene.database.get_object(&object_id) else {
                return Ok(Some(Ok(json!({
                    "ok": false,
                    "apply_mode": "editor_state",
                    "persists_to_disk": false,
                    "open_file": file_path.display().to_string(),
                    "updated": false,
                    "no_op": true,
                    "no_op_reason": "Object not found",
                }))));
            };

            if let Some(name) = tool_args.get("name").and_then(|v| v.as_str()) {
                object.name = name.to_string();
            }
            if let Some(v) = tool_args.get("visible").and_then(|v| v.as_bool()) {
                object.visible = v;
            }
            if let Some(v) = tool_args.get("locked").and_then(|v| v.as_bool()) {
                object.locked = v;
            }
            if let Some(p) = vec3_from_value(tool_args.get("position")) {
                object.transform.position = p;
            }
            if let Some(r) = vec3_from_value(tool_args.get("rotation")) {
                object.transform.rotation = r;
            }
            if let Some(s) = vec3_from_value(tool_args.get("scale")) {
                object.transform.scale = s;
            }

            let result = execute_command(&mut state, SceneCommand::UpdateObject { data: object });

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "id": object_id,
                "updated": result.changed,
                "no_op": !result.changed,
                "no_op_reason": result.no_op_reason,
            }))
        }
        "level_editor_save_scene" => {
            let mut state = state_arc.write();
            let Some(path) = state.scene.current_scene.clone() else {
                return Ok(Some(Ok(json!({
                    "ok": false,
                    "apply_mode": "editor_state",
                    "persists_to_disk": false,
                    "open_file": file_path.display().to_string(),
                    "error": "No current scene path is set. Use the editor Save As flow first.",
                }))));
            };

            match state.scene.database.save_to_file(&path) {
                Ok(_) => {
                    state.scene.has_unsaved_changes = false;
                    state.scene.revision = state.scene.revision.saturating_add(1);
                    Ok(json!({
                        "ok": true,
                        "apply_mode": "editor_state",
                        "persists_to_disk": true,
                        "open_file": file_path.display().to_string(),
                        "saved_path": path.display().to_string(),
                    }))
                }
                Err(error) => Ok(json!({
                    "ok": false,
                    "apply_mode": "editor_state",
                    "persists_to_disk": false,
                    "open_file": file_path.display().to_string(),
                    "saved_path": path.display().to_string(),
                    "error": error,
                })),
            }
        }
        _ => return Ok(None),
    }
    .map(|value| Some(Ok(value)))
}
