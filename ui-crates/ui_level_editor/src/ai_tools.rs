use plugin_editor_api::{AiToolDefinition, PluginError};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use tool_registry::{ChatTool, ToolContext, ToolRegistry};

use crate::ai_sessions;
use crate::level_editor::commands::SceneCommand;
use engine_backend::scene::{LightType, MeshType, ObjectType};

fn is_level_file(file_path: &Path) -> bool {
    file_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.ends_with(".level") || name.ends_with(".level.json"))
        .unwrap_or(false)
}

use crate::ai_sessions::OpenSceneHandle;
use std::sync::atomic::Ordering;

fn get_scene_handle(file_path: &Path) -> Result<OpenSceneHandle, PluginError> {
    ai_sessions::get_open_scene(file_path).ok_or_else(|| PluginError::Other {
        message: format!(
            "Level is not open in editor: {}. Call open_file_in_default_editor first.",
            file_path.display()
        ),
    })
}

/// Execute a scene command against an AI-accessible scene handle.
///
/// Equivalent to the GPUI-side  but operates on an
///  instead of a locked .  Increments the
/// revision counter and marks unsaved changes on success.
fn execute_ai_command(handle: &OpenSceneHandle, cmd: SceneCommand) -> crate::level_editor::commands::CommandResult {
    use crate::level_editor::commands::{execute_command_on_db, CommandResult};
    let result = execute_command_on_db(&handle.scene_db, cmd);
    if result.changed {
        handle.has_unsaved_changes.store(true, Ordering::Relaxed);
        handle.revision.fetch_add(1, Ordering::Relaxed);
    }
    result
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
        if !object
            .name
            .to_lowercase()
            .contains(&name_contains.to_lowercase())
        {
            return false;
        }
    }

    if let Some(visible) = filter.get("visible").and_then(|v| v.as_bool()) {
        if object.visible != visible {
            return false;
        }
    }

    if let Some(object_type) = filter.get("object_type").and_then(|v| v.as_str()) {
        if object_type_key(&object.object_type) != object_type {
            return false;
        }
    }

    if let Some(locked) = filter.get("locked").and_then(|v| v.as_bool()) {
        if object.locked != locked {
            return false;
        }
    }

    if let Some(parent_id) = filter.get("parent_id") {
        match parent_id {
            Value::Null => {
                if object.parent.is_some() {
                    return false;
                }
            }
            Value::String(pid) => {
                if object.parent.as_deref() != Some(pid.as_str()) {
                    return false;
                }
            }
            _ => {}
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
        ObjectType::Blueprint => "blueprint",
    }
}

fn ai_tool_definitions() -> Vec<AiToolDefinition> {
    vec![
        AiToolDefinition::new(
            "level_editor_query_scene",
            "Query high-level scene state: object counts by type, selected object, and unsaved-changes status. Call this first to understand the scene before making edits.",
            json!({
                "type": "object",
                "properties": {}
            }),
        )
        .with_category("analysis"),
        AiToolDefinition::new(
            "level_editor_query_objects",
            "List objects in the scene with optional filtering. Supports pagination via offset/limit. Use filters to narrow results by id, name, type, visibility, locked state, or parent.",
            json!({
                "type": "object",
                "properties": {
                    "filter": {
                        "type": "object",
                        "description": "All filter fields are AND-combined. Omit to return all objects.",
                        "properties": {
                            "id": { "type": "string", "description": "Exact object id match." },
                            "name_contains": { "type": "string", "description": "Case-insensitive substring match on name." },
                            "object_type": {
                                "type": "string",
                                "description": "One of: empty, folder, camera, light_directional, light_point, light_spot, light_area, mesh_cube, mesh_sphere, mesh_cylinder, mesh_plane, mesh_custom, particle_system, audio_source"
                            },
                            "visible": { "type": "boolean" },
                            "locked": { "type": "boolean" },
                            "parent_id": {
                                "type": ["string", "null"],
                                "description": "null = root objects only; a string id = direct children of that parent."
                            }
                        },
                        "additionalProperties": false
                    },
                    "offset": { "type": "integer", "minimum": 0, "description": "Pagination offset (default 0)." },
                    "limit": { "type": "integer", "minimum": 1, "description": "Max results to return (default 200)." }
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
            "Add a single new object to the scene. Returns the assigned object id. Position/rotation are world-space; scale defaults to [1,1,1]. After adding, call level_editor_get_object with the returned id to confirm placement.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Display name for the object." },
                    "kind": {
                        "type": "string",
                        "description": "One of: empty, folder, camera, light_directional, light_point, light_spot, light_area, mesh_cube, mesh_sphere, mesh_cylinder, mesh_plane, mesh_custom, particle_system, audio_source"
                    },
                    "parent_id": { "type": ["string", "null"], "description": "Parent object id, or null/omit for root." },
                    "visible": { "type": "boolean", "description": "Defaults to true." },
                    "locked": { "type": "boolean", "description": "Defaults to false." },
                    "position": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3,
                        "description": "World-space [x, y, z]. Defaults to [0,0,0]."
                    },
                    "rotation": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3,
                        "description": "Euler angles in degrees [pitch, yaw, roll]. Defaults to [0,0,0]."
                    },
                    "scale": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3,
                        "description": "Non-uniform scale [x, y, z]. Defaults to [1,1,1]."
                    }
                },
                "required": ["name", "kind"]
            }),
        )
        .with_category("editing"),
        AiToolDefinition::new(
            "level_editor_batch_add_objects",
            "Add multiple objects in one call. Prefer this over repeated level_editor_add_object calls when creating many objects at once (e.g. populating a level). Returns created ids and any per-item errors.",
            json!({
                "type": "object",
                "properties": {
                    "objects": {
                        "type": "array",
                        "description": "Array of objects to create. Each follows the same schema as level_editor_add_object.",
                        "items": {
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
            "Duplicate an object one or more times. Each copy inherits the source transform. Use position_offset to space copies apart (offset is applied cumulatively: copy i is placed at source_position + offset * i).",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Source object id to duplicate." },
                    "count": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Number of copies to create (default 1)." },
                    "position_offset": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3,
                        "description": "Per-copy world-space offset [dx, dy, dz]. Copy i is placed at source_pos + offset * i. Useful for creating rows, grids, or stacked items."
                    }
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
            "Set absolute world-space transform on a single object. Only fields provided are changed; omitted fields keep their current values. To update name/visible/locked use level_editor_update_object instead.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Object id to update." },
                    "position": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3,
                        "description": "Absolute world-space [x, y, z]."
                    },
                    "rotation": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3,
                        "description": "Euler angles in degrees [pitch, yaw, roll]."
                    },
                    "scale": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3,
                        "description": "Non-uniform scale [x, y, z]."
                    }
                },
                "required": ["id"]
            }),
        )
        .with_category("editing"),
        AiToolDefinition::new(
            "level_editor_update_object",
            "Update any combination of properties on a single object: name, visibility, locked state, and/or transform. Only supplied fields are changed. This is the preferred tool for modifying an existing object.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Object id to update." },
                    "name": { "type": "string", "description": "New display name." },
                    "visible": { "type": "boolean" },
                    "locked": { "type": "boolean" },
                    "position": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3,
                        "description": "Absolute world-space [x, y, z]."
                    },
                    "rotation": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3,
                        "description": "Euler angles in degrees [pitch, yaw, roll]."
                    },
                    "scale": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 3,
                        "maxItems": 3,
                        "description": "Non-uniform scale [x, y, z]."
                    }
                },
                "required": ["id"]
            }),
        )
        .with_category("editing"),
        AiToolDefinition::new(
            "level_editor_save_scene",
            "Write the current in-memory scene state to disk. Always call this after a series of edits to persist changes. Reports the saved file path.",
            json!({
                "type": "object",
                "properties": {}
            }),
        )
        .with_category("editing"),
        AiToolDefinition::new(
            "level_editor_bulk_update_objects",
            "Apply the same property changes to all objects matching a filter. Useful for hiding all lights, locking all cameras, repositioning a group, etc. Omit filter to affect all objects.",
            json!({
                "type": "object",
                "properties": {
                    "filter": {
                        "type": "object",
                        "description": "All filter fields are AND-combined. Omit to match all objects.",
                        "properties": {
                            "id": { "type": "string" },
                            "name_contains": { "type": "string", "description": "Case-insensitive substring match." },
                            "object_type": {
                                "type": "string",
                                "description": "One of: empty, folder, camera, light_directional, light_point, light_spot, light_area, mesh_cube, mesh_sphere, mesh_cylinder, mesh_plane, mesh_custom, particle_system, audio_source"
                            },
                            "visible": { "type": "boolean" },
                            "locked": { "type": "boolean" },
                            "parent_id": { "type": ["string", "null"] }
                        },
                        "additionalProperties": false
                    },
                    "set": {
                        "type": "object",
                        "description": "Fields to overwrite on every matched object.",
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
            "Delete all objects matching a filter. Omit filter to delete everything. Use level_editor_query_objects first to confirm the target set before deleting.",
            json!({
                "type": "object",
                "properties": {
                    "filter": {
                        "type": "object",
                        "description": "All filter fields are AND-combined. Omit to match all objects.",
                        "properties": {
                            "id": { "type": "string" },
                            "name_contains": { "type": "string", "description": "Case-insensitive substring match." },
                            "object_type": {
                                "type": "string",
                                "description": "One of: empty, folder, camera, light_directional, light_point, light_spot, light_area, mesh_cube, mesh_sphere, mesh_cylinder, mesh_plane, mesh_custom, particle_system, audio_source"
                            },
                            "visible": { "type": "boolean" },
                            "locked": { "type": "boolean" },
                            "parent_id": { "type": ["string", "null"] }
                        },
                        "additionalProperties": false
                    }
                }
            }),
        )
        .with_category("editing"),
    ]
}

struct LevelEditorRegistryTool {
    name: &'static str,
    description: &'static str,
    category: Option<&'static str>,
    parameters_schema: Value,
}

impl ChatTool for LevelEditorRegistryTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn category(&self) -> Option<&'static str> {
        self.category
    }

    fn parameters_schema(&self) -> Value {
        self.parameters_schema.clone()
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path = ctx
            .current_file
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Current file path missing from ToolContext"))?;
        execute_ai_tool_impl(file_path, self.name, args)
            .map_err(|err| anyhow::anyhow!(err.to_string()))
    }
}

fn tool_registry() -> &'static ToolRegistry {
    static REGISTRY: OnceLock<ToolRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = ToolRegistry::new();
        for definition in ai_tool_definitions() {
            let name: &'static str = Box::leak(definition.name.clone().into_boxed_str());
            let description: &'static str =
                Box::leak(definition.description.clone().into_boxed_str());
            let category: Option<&'static str> = definition
                .category
                .as_ref()
                .map(|c| Box::leak(c.clone().into_boxed_str()) as &'static str);

            registry.register(Arc::new(LevelEditorRegistryTool {
                name,
                description,
                category,
                parameters_schema: definition.parameters_json_schema.clone(),
            }));
        }
        registry
    })
}

pub fn ai_tools() -> Vec<AiToolDefinition> {
    tool_registry()
        .definitions()
        .into_iter()
        .map(|def| {
            let mut ai_def =
                AiToolDefinition::new(def.name, def.description, def.parameters_schema);
            if let Some(category) = def.category {
                ai_def = ai_def.with_category(category);
            }
            ai_def
        })
        .collect()
}

pub fn capabilities_for_file(file_path: &Path) -> Vec<String> {
    if !is_level_file(file_path) {
        return Vec::new();
    }

    tool_registry()
        .names()
        .into_iter()
        .map(|name| name.to_string())
        .collect()
}

pub fn execute_ai_tool(
    file_path: &Path,
    tool_name: &str,
    tool_args: Value,
) -> Result<Value, PluginError> {
    let ctx = ToolContext::new().with_current_file(file_path);
    tool_registry()
        .execute(tool_name, tool_args, &ctx)
        .map_err(|err| PluginError::Other {
            message: err.to_string(),
        })
}

fn execute_ai_tool_impl(
    file_path: &Path,
    tool_name: &str,
    tool_args: Value,
) -> Result<Value, PluginError> {
    let handle = get_scene_handle(file_path)?;

    match tool_name {
        "level_editor_query_scene" => {
            let objects = handle.scene_db.get_all_objects();
            let roots = handle.scene_db.get_root_objects();
            let selected_object_id = handle.scene_db.get_selected_object_id();

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
                "current_scene": Some(file_path.display().to_string()),
                "has_unsaved_changes": handle.has_unsaved_changes.load(Ordering::Relaxed),
                "editor_mode": format!("{:?}", crate::level_editor::ui::EditorMode::Edit),
                "object_count": objects.len(),
                "root_object_count": roots.len(),
                "selected_object_id": selected_object_id,
                "counts_by_type": counts_by_type,
            }))
        }
        "level_editor_query_objects" => {
            let objects = handle.scene_db.get_all_objects();
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
            let object = handle.scene_db.get_object(&object_id.to_string());

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
            let selected_id = handle.scene_db.get_selected_object_id();
            let selected_object = handle.scene_db.get_selected_object();

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
            execute_ai_command(&handle, SceneCommand::SelectObject {
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
            let scene_path = file_path.display().to_string();
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
            };
            object.transform.position =
                vec3_from_value(tool_args.get("position")).unwrap_or([0.0, 0.0, 0.0]);
            object.transform.rotation =
                vec3_from_value(tool_args.get("rotation")).unwrap_or([0.0, 0.0, 0.0]);
            object.transform.scale =
                vec3_from_value(tool_args.get("scale")).unwrap_or([1.0, 1.0, 1.0]);

            let result = execute_ai_command(&handle, SceneCommand::AddObject {
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
                    scene_path: file_path.display().to_string(),
                };

                object.transform.position =
                    vec3_from_value(item_obj.get("position")).unwrap_or([0.0, 0.0, 0.0]);
                object.transform.rotation =
                    vec3_from_value(item_obj.get("rotation")).unwrap_or([0.0, 0.0, 0.0]);
                object.transform.scale =
                    vec3_from_value(item_obj.get("scale")).unwrap_or([1.0, 1.0, 1.0]);

                let res = execute_ai_command(&handle, SceneCommand::AddObject {
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

            let (child_ids, child_objects) = if let Some(ref parent_id) = parent_id {
                let ids = handle.scene_db.get_children(parent_id);
                let objects = ids
                    .iter()
                    .filter_map(|id| handle.scene_db.get_object(id))
                    .collect::<Vec<_>>();
                (ids, objects)
            } else {
                let objects = handle.scene_db.get_root_objects();
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
            let result = execute_ai_command(&handle, SceneCommand::ReparentObject {
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
            let result = execute_ai_command(&handle, SceneCommand::DuplicateObject {
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
        "level_editor_remove_object" => {
            let object_id = tool_args
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PluginError::Other {
                    message: "level_editor_remove_object requires `id`".to_string(),
                })?
                .to_string();
            let result = execute_ai_command(&handle, SceneCommand::RemoveObject {
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
            let result = execute_ai_command(&handle, SceneCommand::SetTransform {
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
            let Some(mut object) = handle.scene_db.get_object(&object_id) else {
                return Ok(json!({
                    "ok": false,
                    "apply_mode": "editor_state",
                    "persists_to_disk": false,
                    "open_file": file_path.display().to_string(),
                    "updated": false,
                    "no_op": true,
                    "no_op_reason": "Object not found",
                }));
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

            let result = execute_ai_command(&handle, SceneCommand::UpdateObject { data: object });

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
            let Some(path) = Some(file_path.to_path_buf()) else {
                return Ok(json!({
                    "ok": false,
                    "apply_mode": "editor_state",
                    "persists_to_disk": false,
                    "open_file": file_path.display().to_string(),
                    "error": "No current scene path is set. Use the editor Save As flow first.",
                }));
            };

            match handle.scene_db.save_to_file(&path) {
                Ok(_) => {
                    handle.has_unsaved_changes.store(false, Ordering::Relaxed);
                    handle.revision.fetch_add(1, Ordering::Relaxed);
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
            let objects = handle.scene_db.get_all_objects();
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
                        execute_ai_command(&handle, SceneCommand::UpdateObject { data: object });
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
            let objects = handle.scene_db.get_all_objects();
            let delete_ids = objects
                .iter()
                .filter(|object| object_matches_filter(object, filter))
                .map(|object| object.id.clone())
                .collect::<Vec<_>>();
            let matched_count = delete_ids.len();

            let mut deleted_ids = Vec::new();
            for object_id in delete_ids {
                let res = execute_ai_command(&handle, SceneCommand::RemoveObject {
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
        _ => Err(PluginError::Other {
            message: format!("Unknown Level Editor AI tool: {tool_name}"),
        }),
    }
}
