//! The level editor's AI tools.
//!
//! Every tool is a plain function annotated with ToolbeltRS's `#[tool]`,
//! spread over the submodules by editor area. The macro submits each one to
//! `inventory` under its module path, and [`tool_registry`] collects
//! everything under this module with `PluginToolRegistry::from_namespace` --
//! adding a tool is writing the function, nothing else.
//!
//! The tool description and per-argument docs the model sees are the
//! function's rustdoc (summary + `# Arguments`), so those docs are part of
//! the tool's contract: say what a value means, its units, and what to call
//! next.
//!
//! Tools reach the open scene through the `&ToolContext`'s `current_file`
//! (the `.level` file the call was routed for) and mutate it only through
//! `execute_command`, so every edit is undoable exactly like the user's own.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::{Result, anyhow, bail};
use plugin_editor_api::{AiToolDefinition, PluginError};
use serde::Deserialize;
use serde_json::{Value, json};
use tool_registry::{PluginToolRegistry, ToolContext, ToolRegistry};

use super::sessions;
use crate::level_editor::LevelEditorState;
use crate::level_editor::commands::{CommandResult, SceneCommand, execute_command};
use crate::level_editor::scene_edit::{self, SceneObjectData};
use engine_backend::scene::{LightType, MeshType, ObjectType};
use pulsar_scenedb::World;

mod classes;
mod components;
mod objects;
mod scene;
mod splines;
mod view;

type StateArc = Arc<parking_lot::RwLock<LevelEditorState>>;

// ── Registry ─────────────────────────────────────────────────────────────────

fn tool_registry() -> &'static ToolRegistry {
    static REGISTRY: OnceLock<ToolRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = ToolRegistry::new();
        registry.merge_plugin(&PluginToolRegistry::from_namespace(module_path!()));
        registry
    })
}

fn is_level_file(file_path: &Path) -> bool {
    file_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".level") || name.ends_with(".level.json"))
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
        .map(str::to_string)
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

// ── Scene access ─────────────────────────────────────────────────────────────

/// The editor state of the level the call was routed for.
fn open_scene(ctx: &ToolContext) -> Result<StateArc> {
    let file = ctx
        .current_file
        .as_deref()
        .ok_or_else(|| anyhow!("No level file in the tool context"))?;
    sessions::get_open_scene_state(file).ok_or_else(|| {
        anyhow!(
            "Level is not open in the editor: {}. Open it first (open_file_in_default_editor).",
            file.display()
        )
    })
}

fn command_json(result: &CommandResult) -> Value {
    if result.changed {
        json!({ "changed": true, "affected_ids": result.affected_ids })
    } else {
        json!({ "changed": false, "reason": result.no_op_reason })
    }
}

fn require_object(state: &LevelEditorState, id: &str) -> Result<SceneObjectData> {
    scene_edit::objects::get_object(&state.scene.world(), id)
        .ok_or_else(|| anyhow!("No object with id '{id}'. Use level_editor_list_objects to find ids."))
}

// ── Object kinds ─────────────────────────────────────────────────────────────

const KINDS: &str = "empty, folder, camera, light_directional, light_point, light_spot, light_area, mesh_cube, mesh_sphere, mesh_cylinder, mesh_plane, mesh_custom, particle_system, audio_source";

fn object_type_from_kind(kind: &str) -> Result<ObjectType> {
    Ok(match kind {
        "empty" => ObjectType::Empty,
        "folder" => ObjectType::Folder,
        "camera" => ObjectType::Camera,
        "light_directional" => ObjectType::Light(LightType::Directional),
        "light_point" => ObjectType::Light(LightType::Point),
        "light_spot" => ObjectType::Light(LightType::Spot),
        "light_area" => ObjectType::Light(LightType::Area),
        "mesh_cube" => ObjectType::Mesh(MeshType::Cube),
        "mesh_sphere" => ObjectType::Mesh(MeshType::Sphere),
        "mesh_cylinder" => ObjectType::Mesh(MeshType::Cylinder),
        "mesh_plane" => ObjectType::Mesh(MeshType::Plane),
        "mesh_custom" => ObjectType::Mesh(MeshType::Custom),
        "particle_system" => ObjectType::ParticleSystem,
        "audio_source" => ObjectType::AudioSource,
        other => bail!("Unknown kind '{other}'. Valid kinds: {KINDS}"),
    })
}

fn object_kind(object_type: &ObjectType) -> &'static str {
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

// ── Views ────────────────────────────────────────────────────────────────────

/// Compact description of an object: enough to pick it out and place it.
fn object_summary(world: &World, object: &SceneObjectData) -> Value {
    json!({
        "id": object.id,
        "name": object.name,
        "kind": object_kind(&object.object_type),
        "parent_id": object.parent,
        "child_count": object.children.len(),
        "position": object.transform.position,
        "rotation": object.transform.rotation,
        "scale": object.transform.scale,
        "visible": object.visible,
        "locked": object.locked,
        "components": scene_edit::components::get_component_class_names(world, &object.id),
    })
}

// ── Filters ──────────────────────────────────────────────────────────────────

/// Object selector shared by the list and bulk tools. Fields are AND-combined.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ObjectFilter {
    ids: Option<Vec<String>>,
    name_contains: Option<String>,
    kind: Option<String>,
    has_component: Option<String>,
    parent_id: Option<String>,
    root_only: Option<bool>,
    descendants_of: Option<String>,
    visible: Option<bool>,
    locked: Option<bool>,
}

const FILTER_DOC: &str = "keys (all optional, AND-combined): ids [string], name_contains (case-insensitive), kind, has_component (class name), parent_id (direct children of), root_only (bool), descendants_of (id; whole subtree), visible, locked";

impl ObjectFilter {
    fn parse(value: Option<Value>) -> Result<Self> {
        match value {
            None => Ok(Self::default()),
            Some(value) => serde_json::from_value(value)
                .map_err(|e| anyhow!("Invalid filter ({e}). Filter {FILTER_DOC}")),
        }
    }

    fn matches(&self, world: &World, object: &SceneObjectData) -> bool {
        if self.ids.as_ref().is_some_and(|ids| !ids.contains(&object.id)) {
            return false;
        }
        if let Some(needle) = &self.name_contains {
            if !object.name.to_lowercase().contains(&needle.to_lowercase()) {
                return false;
            }
        }
        if self.kind.as_deref().is_some_and(|k| k != object_kind(&object.object_type)) {
            return false;
        }
        if let Some(class) = &self.has_component {
            if !scene_edit::components::get_component_class_names(world, &object.id).contains(class) {
                return false;
            }
        }
        if self.parent_id.as_ref().is_some_and(|p| object.parent.as_ref() != Some(p)) {
            return false;
        }
        if self.root_only == Some(true) && object.parent.is_some() {
            return false;
        }
        if let Some(ancestor) = &self.descendants_of {
            if !is_descendant(world, &object.id, ancestor) {
                return false;
            }
        }
        if self.visible.is_some_and(|v| v != object.visible) {
            return false;
        }
        if self.locked.is_some_and(|l| l != object.locked) {
            return false;
        }
        true
    }

    /// Every object matching the filter, in hierarchy order.
    fn select(&self, world: &World) -> Vec<SceneObjectData> {
        scene_edit::objects::get_all_objects(world)
            .into_iter()
            .filter(|object| self.matches(world, object))
            .collect()
    }
}

fn is_descendant(world: &World, id: &str, ancestor: &str) -> bool {
    let mut current = scene_edit::objects::get_object(world, id).and_then(|o| o.parent);
    while let Some(parent) = current {
        if parent == ancestor {
            return true;
        }
        current = scene_edit::objects::get_object(world, &parent).and_then(|o| o.parent);
    }
    false
}

/// Ids from an explicit list or a filter; exactly one must be given.
fn target_ids(world: &World, ids: Option<Vec<String>>, filter: Option<Value>) -> Result<Vec<String>> {
    match (ids, filter) {
        (Some(ids), None) => Ok(ids),
        (None, Some(filter)) => Ok(ObjectFilter::parse(Some(filter))?
            .select(world)
            .into_iter()
            .map(|o| o.id)
            .collect()),
        (Some(_), Some(_)) => bail!("Pass either `ids` or `filter`, not both"),
        (None, None) => bail!("Pass `ids` or `filter` to choose the objects"),
    }
}

// ── JSON helpers ─────────────────────────────────────────────────────────────

/// RFC 7386-style merge: objects merge key by key, `null` deletes, anything
/// else replaces.
fn merge_json(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                if value.is_null() {
                    target.remove(key);
                } else {
                    merge_json(target.entry(key.clone()).or_insert(Value::Null), value);
                }
            }
        }
        (target, patch) => *target = patch.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_collected_with_an_object_schema() {
        let definitions = ai_tools();
        assert!(definitions.len() >= 30, "only {} tools", definitions.len());
        for def in &definitions {
            assert!(def.name.starts_with("level_editor_"), "{}", def.name);
            assert!(!def.description.is_empty(), "{} has no description", def.name);
            assert_eq!(def.parameters_json_schema["type"], "object", "{}", def.name);
            assert!(
                def.parameters_json_schema["properties"].get("ctx").is_none(),
                "{} exposes its context",
                def.name
            );
        }
    }

    #[test]
    fn vectors_are_three_number_arrays() {
        let def = ai_tools()
            .into_iter()
            .find(|d| d.name == "level_editor_set_transform")
            .unwrap();
        let position = &def.parameters_json_schema["properties"]["position"];
        assert_eq!(position["type"], "array");
        assert_eq!(position["maxItems"], 3);
        assert!(position["description"].as_str().is_some());
    }

    #[test]
    fn capabilities_only_for_level_files() {
        assert!(capabilities_for_file(Path::new("a.level")).len() > 0);
        assert!(capabilities_for_file(Path::new("a.rs")).is_empty());
    }

    #[test]
    fn merge_replaces_scalars_and_merges_objects() {
        let mut v = json!({ "a": { "b": 1, "c": 2 }, "d": 3 });
        merge_json(&mut v, &json!({ "a": { "b": 5, "c": null }, "e": [1] }));
        assert_eq!(v, json!({ "a": { "b": 5 }, "d": 3, "e": [1] }));
    }

    /// Drives the tools the way the chat does: through `execute_ai_tool`
    /// with a level path, against a scene registered as open.
    #[test]
    fn tools_edit_an_open_level_undoably() {
        let dir = std::env::temp_dir().join(format!("le_ai_tools_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let level = dir.join("test.level");
        std::fs::write(&level, "{}").unwrap();
        let state: StateArc = Arc::new(parking_lot::RwLock::new(LevelEditorState::new()));
        sessions::register_open_scene(&level, &state);
        let call = |tool: &str, args: Value| {
            execute_ai_tool(&level, tool, args).unwrap_or_else(|e| panic!("{tool}: {e}"))
        };

        // Spawn an object with a component in one call, patching nested data.
        let spawned = call(
            "level_editor_spawn_object",
            json!({
                "name": "Lamp",
                "kind": "light_point",
                "position": [1.0, 2.0, 3.0],
                "components": [{
                    "class_name": "LightComponent",
                    "properties": { "intensity": { "intensity": 1234.0 } },
                }],
            }),
        );
        let id = spawned["created_id"].as_str().unwrap().to_string();
        assert_eq!(spawned["object"]["kind"], "light_point");
        assert_eq!(spawned["object"]["position"], json!([1.0, 2.0, 3.0]));

        let components = call("level_editor_get_components", json!({ "id": id }));
        assert_eq!(components["components"][0]["class_name"], "LightComponent");
        assert_eq!(components["components"][0]["data"]["intensity"]["intensity"], 1234.0);

        // Edit a nested field by class name.
        let edited = call(
            "level_editor_set_component_properties",
            json!({ "id": id, "class_name": "LightComponent", "properties": { "intensity": { "intensity": 50.0 } } }),
        );
        assert_eq!(edited["changed"], true);
        let components = call("level_editor_get_components", json!({ "id": id }));
        assert_eq!(components["components"][0]["data"]["intensity"]["intensity"], 50.0);

        // Data the class can't take is rejected, not dropped.
        let bad = execute_ai_tool(
            &level,
            "level_editor_set_component_properties",
            json!({ "id": id, "component_index": 0, "properties": { "intensity": { "intensity": "bright" } } }),
        );
        assert!(bad.is_err());

        // Relative moves, duplication with offsets, filters.
        call("level_editor_move_objects", json!({ "ids": [id], "translate": [0.0, 1.0, 0.0] }));
        let dupes = call(
            "level_editor_duplicate_object",
            json!({ "id": id, "count": 2, "offset": [5.0, 0.0, 0.0] }),
        );
        assert_eq!(dupes["affected_ids"].as_array().unwrap().len(), 2);
        let listed = call(
            "level_editor_list_objects",
            json!({ "filter": { "has_component": "LightComponent" } }),
        );
        assert_eq!(listed["total_matches"], 3);

        // Undo walks back the duplicate, then the component edit.
        call("level_editor_undo", json!({ "steps": 2 }));
        let listed = call("level_editor_list_objects", json!({}));
        assert_eq!(listed["total_matches"], 1);
        assert_eq!(listed["items"][0]["position"], json!([1.0, 2.0, 3.0]));
        assert!(state.read().scene.pending_renderer_resync);

        // Component structure edits.
        call("level_editor_duplicate_component", json!({ "id": id, "component_index": 0 }));
        let disabled = call(
            "level_editor_set_component_enabled",
            json!({ "id": id, "component_index": 1, "enabled": false }),
        );
        assert_eq!(disabled["components"][1]["enabled"], false);
        call("level_editor_remove_component", json!({ "id": id, "component_index": 1 }));
        let components = call("level_editor_get_components", json!({ "id": id }));
        assert_eq!(components["components"].as_array().unwrap().len(), 1);

        // Splines and deletion.
        let spline = call(
            "level_editor_create_spline",
            json!({ "points": [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 10.0]] }),
        );
        let spline_id = spline["id"].as_str().unwrap().to_string();
        let edited = call(
            "level_editor_edit_spline",
            json!({ "id": spline_id, "append_points": [[0.0, 0.0, 10.0]], "closed": true }),
        );
        assert_eq!(edited["point_count"], 4);
        call("level_editor_delete_objects", json!({ "filter": { "root_only": true } }));
        assert_eq!(call("level_editor_query_scene", json!({}))["object_count"], 0);

        sessions::unregister_open_scene(&level);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unopened_level_is_a_clear_error() {
        let err = execute_ai_tool(
            Path::new("definitely/not/open.level"),
            "level_editor_query_scene",
            json!({}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("not open"), "{err}");
    }
}
