use plugin_editor_api::{AiToolDefinition, PluginError};
use serde_json::{json, Value};
use std::path::Path;

use crate::ai_sessions;
use engine_backend::scene::{LightType, MeshType, ObjectType};

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

fn object_type_from_kind(kind: &str) -> Option<ObjectType> {
    match kind {
        "empty" => Some(ObjectType::Empty),
        "folder" => Some(ObjectType::Folder),
        "camera" => Some(ObjectType::Camera),
        "light_directional" => Some(ObjectType::Light(LightType::Directional)),
        "light_point" => Some(ObjectType::Light(LightType::Point)),
        "light_spot" => Some(ObjectType::Light(LightType::Spot)),
        "light_area" => Some(ObjectType::Light(LightType::Area)),
        "mesh_cube" => Some(ObjectType::Mesh(MeshType::Cube)),
        "mesh_sphere" => Some(ObjectType::Mesh(MeshType::Sphere)),
        "mesh_cylinder" => Some(ObjectType::Mesh(MeshType::Cylinder)),
        "mesh_plane" => Some(ObjectType::Mesh(MeshType::Plane)),
        "mesh_custom" => Some(ObjectType::Mesh(MeshType::Custom)),
        "particle_system" => Some(ObjectType::ParticleSystem),
        "audio_source" => Some(ObjectType::AudioSource),
        _ => None,
    }
}

fn object_type_key(object_type: &ObjectType) -> &'static str {
    match object_type {
        ObjectType::Empty => "empty",
        ObjectType::Folder => "folder",
        ObjectType::Camera => "camera",
        ObjectType::Light(LightType::Directional) => "light_directional",
        ObjectType::Light(LightType::Point) => "light_point",
        ObjectType::Light(LightType::Spot) => "light_spot",
        ObjectType::Light(LightType::Area) => "light_area",
        ObjectType::Mesh(MeshType::Cube) => "mesh_cube",
        ObjectType::Mesh(MeshType::Sphere) => "mesh_sphere",
        ObjectType::Mesh(MeshType::Cylinder) => "mesh_cylinder",
        ObjectType::Mesh(MeshType::Plane) => "mesh_plane",
        ObjectType::Mesh(MeshType::Custom) => "mesh_custom",
        ObjectType::ParticleSystem => "particle_system",
        ObjectType::AudioSource => "audio_source",
    }
}

fn bump_scene_revision(state: &mut crate::level_editor::LevelEditorState, marks_unsaved: bool) {
    state.scene_revision = state.scene_revision.saturating_add(1);
    if marks_unsaved {
        state.has_unsaved_changes = true;
    }
}

pub fn ai_tools() -> Vec<AiToolDefinition> {
    vec![
        AiToolDefinition::new(
            "level_editor_query_scene",
            "Query high-level scene state and object-type counts for the currently open level editor scene.",
            json!({
                "type": "object",
                "properties": {}
            }),
        )
        .with_category("analysis"),
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
            "level_editor_get_object",
            "Get a single object by id from the currently open level editor scene.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" }
                },
                "required": ["id"]
            }),
        )
        .with_category("analysis"),
        AiToolDefinition::new(
            "level_editor_query_selection",
            "Query current object selection in the level editor scene.",
            json!({
                "type": "object",
                "properties": {}
            }),
        )
        .with_category("analysis"),
        AiToolDefinition::new(
            "level_editor_select_object",
            "Select an object by id in the level editor scene, or clear selection.",
            json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": ["string", "null"],
                        "description": "Object id to select. Use null to clear selection."
                    }
                }
            }),
        )
        .with_category("editing"),
        AiToolDefinition::new(
            "level_editor_add_object",
            "Add a new object to the currently open level editor scene.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "kind": {
                        "type": "string",
                        "description": "One of: empty, folder, camera, light_directional, light_point, light_spot, light_area, mesh_cube, mesh_sphere, mesh_cylinder, mesh_plane, mesh_custom, particle_system, audio_source"
                    },
                    "parent_id": { "type": ["string", "null"] },
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
                "required": ["name", "kind"]
            }),
        )
        .with_category("editing"),
        AiToolDefinition::new(
            "level_editor_batch_add_objects",
            "Add multiple objects to the currently open level editor scene in one call.",
            json!({
                "type": "object",
                "properties": {
                    "objects": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "kind": { "type": "string" },
                                "parent_id": { "type": ["string", "null"] },
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
                            "required": ["name", "kind"]
                        },
                        "minItems": 1
                    }
                },
                "required": ["objects"]
            }),
        )
        .with_category("editing"),
        AiToolDefinition::new(
            "level_editor_query_children",
            "Query direct children for a parent object id, or root objects when parent_id is null/omitted.",
            json!({
                "type": "object",
                "properties": {
                    "parent_id": { "type": ["string", "null"] }
                }
            }),
        )
        .with_category("analysis"),
        AiToolDefinition::new(
            "level_editor_reparent_object",
            "Re-parent an object under a new parent (or null for root).",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "new_parent_id": { "type": ["string", "null"] }
                },
                "required": ["id"]
            }),
        )
        .with_category("editing"),
        AiToolDefinition::new(
            "level_editor_duplicate_object",
            "Duplicate an object. Optionally create multiple duplicates.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "count": { "type": "integer", "minimum": 1, "maximum": 100 }
                },
                "required": ["id"]
            }),
        )
        .with_category("editing"),
        AiToolDefinition::new(
            "level_editor_remove_object",
            "Remove a single object by id from the currently open level editor scene.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" }
                },
                "required": ["id"]
            }),
        )
        .with_category("editing"),
        AiToolDefinition::new(
            "level_editor_set_transform",
            "Set transform values on a single object by id (position/rotation/scale).",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
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
                "required": ["id"]
            }),
        )
        .with_category("editing"),
        AiToolDefinition::new(
            "level_editor_save_scene",
            "Persist the currently open scene to disk using the editor's current scene path.",
            json!({
                "type": "object",
                "properties": {}
            }),
        )
        .with_category("editing"),
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
        "level_editor_query_scene" => {
            let state = state_arc.read();
            let objects = state.scene_database.get_all_objects();
            let roots = state.scene_database.get_root_objects();
            let selected_object_id = state.scene_database.get_selected_object_id();

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
                "current_scene": state.current_scene.as_ref().map(|p| p.display().to_string()),
                "has_unsaved_changes": state.has_unsaved_changes,
                "editor_mode": format!("{:?}", state.editor_mode),
                "object_count": objects.len(),
                "root_object_count": roots.len(),
                "selected_object_id": selected_object_id,
                "counts_by_type": counts_by_type,
            }))
        }
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
        "level_editor_get_object" => {
            let object_id = tool_args
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_get_object requires `id`".to_string(),
                })?;

            let state = state_arc.read();
            let object = state.scene_database.get_object(&object_id.to_string());

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
            let selected_id = state.scene_database.get_selected_object_id();
            let selected_object = state.scene_database.get_selected_object();

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
                        message:
                            "level_editor_select_object.id must be a string or null".to_string(),
                    })
                }
            };

            let mut state = state_arc.write();
            state.scene_database.select_object(selection.clone());
            bump_scene_revision(&mut state, false);

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "selected_object_id": selection,
            }))
        }
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

            let object_type = object_type_from_kind(kind).ok_or_else(|| PluginError::Other {
                message: format!(
                    "Unsupported kind '{kind}'. Valid kinds: empty, folder, camera, light_directional, light_point, light_spot, light_area, mesh_cube, mesh_sphere, mesh_cylinder, mesh_plane, mesh_custom, particle_system, audio_source"
                ),
            })?;

            let parent_id = tool_args
                .get("parent_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let mut state = state_arc.write();
            let mut object = crate::SceneObjectData {
                id: String::new(),
                name,
                object_type,
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
                components: vec![],
                scene_path: state
                    .current_scene
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            };
            object.transform.position =
                vec3_from_value(tool_args.get("position")).unwrap_or([0.0, 0.0, 0.0]);
            object.transform.rotation =
                vec3_from_value(tool_args.get("rotation")).unwrap_or([0.0, 0.0, 0.0]);
            object.transform.scale =
                vec3_from_value(tool_args.get("scale")).unwrap_or([1.0, 1.0, 1.0]);

            let new_id = state.scene_database.add_object(object, parent_id);
            bump_scene_revision(&mut state, true);

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

                let Some(object_type) = object_type_from_kind(kind) else {
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
                    object_type,
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
                    components: vec![],
                    scene_path: state
                        .current_scene
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                };

                object.transform.position =
                    vec3_from_value(item_obj.get("position")).unwrap_or([0.0, 0.0, 0.0]);
                object.transform.rotation =
                    vec3_from_value(item_obj.get("rotation")).unwrap_or([0.0, 0.0, 0.0]);
                object.transform.scale =
                    vec3_from_value(item_obj.get("scale")).unwrap_or([1.0, 1.0, 1.0]);

                let new_id = state.scene_database.add_object(object, parent_id);
                created_ids.push(new_id);
            }

            if !created_ids.is_empty() {
                bump_scene_revision(&mut state, true);
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
                let ids = state.scene_database.get_children(parent_id);
                let objects = ids
                    .iter()
                    .filter_map(|id| state.scene_database.get_object(id))
                    .collect::<Vec<_>>();
                (ids, objects)
            } else {
                let objects = state.scene_database.get_root_objects();
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
            let new_parent_id = match new_parent_value {
                Some(Value::String(id)) => Some(id.clone()),
                Some(Value::Null) | None => None,
                _ => {
                    return Err(PluginError::Other {
                        message:
                            "level_editor_reparent_object.new_parent_id must be a string or null"
                                .to_string(),
                    })
                }
            };

            let mut state = state_arc.write();
            let moved = state
                .scene_database
                .reparent_object(&object_id, new_parent_id.clone());
            if moved {
                bump_scene_revision(&mut state, true);
            }

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

            let mut state = state_arc.write();
            let mut created_ids = Vec::new();
            for _ in 0..count {
                if let Some(new_id) = state.scene_database.duplicate_object(&source_id) {
                    created_ids.push(new_id);
                } else {
                    break;
                }
            }

            if !created_ids.is_empty() {
                bump_scene_revision(&mut state, true);
            }

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "source_id": source_id,
                "requested_count": count,
                "created_count": created_ids.len(),
                "created_ids": created_ids,
                "no_op": created_ids.is_empty(),
                "no_op_reason": if created_ids.is_empty() { "Source object not found" } else { "" },
            }))
        }
        "level_editor_remove_object" => {
            let object_id = tool_args
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_remove_object requires `id`".to_string(),
                })?
                .to_string();

            let mut state = state_arc.write();
            let removed = state.scene_database.remove_object(&object_id);
            if removed {
                // Keep selection coherent after remove.
                if state.scene_database.get_selected_object_id().as_deref() == Some(object_id.as_str()) {
                    state.scene_database.select_object(None);
                }
                bump_scene_revision(&mut state, true);
            }

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "removed": removed,
                "removed_id": if removed { Some(object_id.clone()) } else { None::<String> },
                "no_op": !removed,
                "no_op_reason": if removed { "" } else { "Object not found" },
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
            let Some(mut object) = state.scene_database.get_object(&object_id) else {
                return Ok(json!({
                    "ok": true,
                    "apply_mode": "editor_state",
                    "persists_to_disk": false,
                    "open_file": file_path.display().to_string(),
                    "updated": false,
                    "no_op": true,
                    "no_op_reason": "Object not found",
                }));
            };

            let mut changed = false;
            if let Some(position) = vec3_from_value(tool_args.get("position")) {
                if object.transform.position != position {
                    object.transform.position = position;
                    changed = true;
                }
            }
            if let Some(rotation) = vec3_from_value(tool_args.get("rotation")) {
                if object.transform.rotation != rotation {
                    object.transform.rotation = rotation;
                    changed = true;
                }
            }
            if let Some(scale) = vec3_from_value(tool_args.get("scale")) {
                if object.transform.scale != scale {
                    object.transform.scale = scale;
                    changed = true;
                }
            }

            let updated = if changed {
                state.scene_database.update_object(object)
            } else {
                false
            };

            if updated {
                bump_scene_revision(&mut state, true);
            }

            Ok(json!({
                "ok": true,
                "apply_mode": "editor_state",
                "persists_to_disk": false,
                "open_file": file_path.display().to_string(),
                "id": object_id,
                "updated": updated,
                "no_op": !updated,
                "no_op_reason": if changed {
                    if updated { "" } else { "Transform update failed in scene database" }
                } else {
                    "No transform fields changed"
                },
            }))
        }
        "level_editor_save_scene" => {
            let mut state = state_arc.write();
            let Some(path) = state.current_scene.clone() else {
                return Ok(json!({
                    "ok": false,
                    "apply_mode": "editor_state",
                    "persists_to_disk": false,
                    "open_file": file_path.display().to_string(),
                    "error": "No current scene path is set. Use the editor Save As flow first.",
                }));
            };

            match state.scene_database.save_to_file(&path) {
                Ok(_) => {
                    state.has_unsaved_changes = false;
                    state.scene_revision = state.scene_revision.saturating_add(1);
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

                if changed && state.scene_database.update_object(object.clone()) {
                    updated_ids.push(object.id.clone());
                }
            }

            if !updated_ids.is_empty() {
                bump_scene_revision(&mut state, true);
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
            let objects = state.scene_database.get_all_objects();
            let delete_ids = objects
                .iter()
                .filter(|object| object_matches_filter(object, filter))
                .map(|object| object.id.clone())
                .collect::<Vec<_>>();
            let matched_count = delete_ids.len();

            let mut deleted_ids = Vec::new();
            for object_id in delete_ids {
                if state.scene_database.remove_object(&object_id) {
                    deleted_ids.push(object_id);
                }
            }

            if !deleted_ids.is_empty() {
                bump_scene_revision(&mut state, true);
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
        _ => Err(PluginError::Other {
            message: format!("Unknown Level Editor AI tool: {tool_name}"),
        }),
    }
}
