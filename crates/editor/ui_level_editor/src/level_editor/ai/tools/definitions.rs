//! `AiToolDefinition` catalogue for the level editor's AI tool bridge.

use super::*;

pub(super) fn ai_tool_definitions() -> Vec<AiToolDefinition> {
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