use super::super::*;
use super::StateArc;

pub(super) fn dispatch(
    tool_name: &str,
    file_path: &Path,
    state_arc: &StateArc,
    tool_args: &Value,
) -> Result<Option<Result<Value, PluginError>>, PluginError> {
    match tool_name {
        "level_editor_add_object" => {
            let name = tool_args
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_add_object requires `name`".to_string(),
                })?
                .to_string();
            let kind = tool_args
                .get("kind")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_add_object requires `kind`".to_string(),
                })?;

            object_type_from_kind(kind).ok_or_else(|| PluginError::Other {
                message: format!(
                    "Unsupported kind '{kind}'. Valid kinds: empty, folder, camera, light_directional, light_point, light_spot, light_area, mesh_cube, mesh_sphere, mesh_cylinder, mesh_plane, mesh_custom, particle_system, audio_source"
                ),
            })?;

            let parent_id = tool_args
                .get("parent_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let mut state = state_arc.write();
            let scene_path = state
                .scene
                .current_scene
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let mut object = crate::SceneObjectData {
                id: String::new(),
                name,
                object_type: ObjectType::Empty,
                transform: Default::default(),
                visible: tool_args
                    .get("visible")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                locked: tool_args
                    .get("locked")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                parent: parent_id.clone(),
                children: vec![],
                props: Default::default(),
                scene_path,
                component_instances: None,
            };
            object.transform.position =
                vec3_from_value(tool_args.get("position")).unwrap_or([0.0, 0.0, 0.0]);
            object.transform.rotation =
                vec3_from_value(tool_args.get("rotation")).unwrap_or([0.0, 0.0, 0.0]);
            object.transform.scale =
                vec3_from_value(tool_args.get("scale")).unwrap_or([1.0, 1.0, 1.0]);

            let result = execute_command(
                &mut state,
                SceneCommand::AddObject {
                    data: object,
                    parent_id,
                },
            );
            let new_id = result.affected_ids.into_iter().next().unwrap_or_default();

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "created": true,
                "created_object_id": new_id,
            }))
        }
        "level_editor_batch_add_objects" => {
            let objects = tool_args
                .get("objects")
                .and_then(|v| v.as_array())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_batch_add_objects requires `objects` array".to_string(),
                })?;

            let mut state = state_arc.write();
            let mut created_ids = Vec::new();
            let mut errors = Vec::new();

            for (index, item) in objects.iter().enumerate() {
                let Some(item_obj) = item.as_object() else {
                    errors.push(json!({
                        "index": index,
                        "error": "object entry must be a JSON object"
                    }));
                    continue;
                };

                let Some(name) = item_obj.get("name").and_then(|v| v.as_str()) else {
                    errors.push(json!({
                        "index": index,
                        "error": "missing required field 'name'"
                    }));
                    continue;
                };

                let Some(kind) = item_obj.get("kind").and_then(|v| v.as_str()) else {
                    errors.push(json!({
                        "index": index,
                        "error": "missing required field 'kind'"
                    }));
                    continue;
                };

                let Some(_validated_kind) = object_type_from_kind(kind) else {
                    errors.push(json!({
                        "index": index,
                        "error": format!("unsupported kind '{kind}'")
                    }));
                    continue;
                };

                let parent_id = item_obj
                    .get("parent_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let mut object = crate::SceneObjectData {
                    id: String::new(),
                    name: name.to_string(),
                    object_type: ObjectType::Empty,
                    transform: Default::default(),
                    visible: item_obj
                        .get("visible")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true),
                    locked: item_obj
                        .get("locked")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    parent: parent_id.clone(),
                    children: vec![],
                    props: Default::default(),
                    scene_path: state
                        .scene
                        .current_scene
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    component_instances: None,
                };

                object.transform.position =
                    vec3_from_value(item_obj.get("position")).unwrap_or([0.0, 0.0, 0.0]);
                object.transform.rotation =
                    vec3_from_value(item_obj.get("rotation")).unwrap_or([0.0, 0.0, 0.0]);
                object.transform.scale =
                    vec3_from_value(item_obj.get("scale")).unwrap_or([1.0, 1.0, 1.0]);

                let res = execute_command(
                    &mut state,
                    SceneCommand::AddObject {
                        data: object,
                        parent_id,
                    },
                );
                if let Some(id) = res.affected_ids.into_iter().next() {
                    created_ids.push(id);
                }
            }

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "requested": objects.len(),
                "created_count": created_ids.len(),
                "created_ids": created_ids,
                "error_count": errors.len(),
                "errors": errors,
                "no_op": objects.is_empty() || created_ids.is_empty(),
            }))
        }
        _ => return Ok(None),
    }
    .map(|value| Some(Ok(value)))
}
