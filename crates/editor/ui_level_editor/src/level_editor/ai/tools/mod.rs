use plugin_editor_api::{AiToolDefinition, PluginError};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use tool_registry::{ChatTool, ToolContext, ToolRegistry};

use super::sessions;
use crate::level_editor::commands::{execute_command, SceneCommand};
use engine_backend::scene::{LightType, MeshType, ObjectType};

fn is_level_file(file_path: &Path) -> bool {
    file_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.ends_with(".level") || name.ends_with(".level.json"))
        .unwrap_or(false)
}

fn open_state_for(
    file_path: &Path,
) -> Result<std::sync::Arc<parking_lot::RwLock<crate::level_editor::LevelEditorState>>, PluginError>
{
    sessions::get_open_scene_state(file_path).ok_or_else(|| PluginError::Other {
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

mod definitions;
mod execute;
mod registry;

pub use execute::execute_ai_tool;
pub use registry::{ai_tools, capabilities_for_file};
