//! `.level` file save/load, straight between the world and the file format.

use std::collections::HashMap;
use std::path::Path;

use engine_fs::virtual_fs;
use pulsar_scenedb::World;

use super::components::{
    add_component_instance, get_components, get_components_metadata, remove_component,
};
use super::objects::{add_object, clear, get_all_objects};
use super::{LevelEditorCameraState, LevelEditorFileState, LevelFile, LevelMetadata};

/// Serialize the scene to a JSON level file.
pub fn save_to_file<P: AsRef<Path>>(world: &World, path: P) -> Result<(), String> {
    save_to_file_with_editor_camera(world, path, None)
}

/// Serialize the scene to a JSON level file, optionally persisting editor camera state.
pub fn save_to_file_with_editor_camera<P: AsRef<Path>>(
    world: &World,
    path: P,
    editor_camera: Option<LevelEditorCameraState>,
) -> Result<(), String> {
    profiling::profile_scope!("scene_edit::save_to_file");
    if let Some(parent_dir) = path.as_ref().parent() {
        virtual_fs::create_dir_all(parent_dir)
            .map_err(|e| format!("Failed to create directory: {e}"))?;
    }
    let objects = get_all_objects(world);
    let components = objects
        .iter()
        .map(|obj| (obj.id.clone(), get_components(world, &obj.id)))
        .collect::<HashMap<_, _>>();
    let now = chrono::Utc::now().to_rfc3339();
    // Read the existing file once: its editor camera is preserved when no fresh
    // camera state was supplied, and its #650 blueprint-binding section always
    // rides along (the editor cannot author it yet, but a re-save must never
    // destroy it).
    let existing_file = virtual_fs::read_file(path.as_ref())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|json: String| serde_json::from_str::<LevelFile>(&json).ok());
    let preserved_editor = if editor_camera.is_none() {
        existing_file.as_ref().and_then(|file| file.editor.clone())
    } else {
        None
    };
    let preserved_bindings = existing_file
        .map(|file| file.blueprint_bindings)
        .unwrap_or_default();
    let level_file = LevelFile {
        version: "2.1".into(),
        objects,
        components,
        blueprint_bindings: preserved_bindings,
        metadata: LevelMetadata {
            created: now.clone(),
            modified: now,
            editor_version: env!("CARGO_PKG_VERSION").into(),
        },
        editor: editor_camera
            .map(|camera| LevelEditorFileState {
                camera: Some(camera),
            })
            .or(preserved_editor),
    };
    let json = serde_json::to_string_pretty(&level_file)
        .map_err(|e| format!("Failed to serialize: {e}"))?;
    virtual_fs::write_file(path.as_ref(), json.as_bytes())
        .map_err(|e| format!("Failed to write file: {e}"))?;

    tracing::info!("Scene saved to: {}", path.as_ref().display());
    Ok(())
}

/// Load a scene from a JSON level file (replaces the current scene).
pub fn load_from_file<P: AsRef<Path>>(world: &mut World, path: P) -> Result<(), String> {
    load_from_file_with_editor_camera(world, path).map(|_| ())
}

/// Load a scene from a JSON level file and return any persisted editor camera state.
pub fn load_from_file_with_editor_camera<P: AsRef<Path>>(
    world: &mut World,
    path: P,
) -> Result<Option<LevelEditorCameraState>, String> {
    profiling::profile_scope!("scene_edit::load_from_file");
    let bytes =
        virtual_fs::read_file(path.as_ref()).map_err(|e| format!("Failed to read file: {e}"))?;
    let json = String::from_utf8(bytes).map_err(|e| format!("File is not valid UTF-8: {e}"))?;
    let level_file: LevelFile =
        serde_json::from_str(&json).map_err(|e| format!("Failed to parse JSON: {e}"))?;
    if !level_file.version.starts_with("2.") && !level_file.version.starts_with("1.") {
        return Err(format!(
            "Unsupported scene version: {}. Expected 1.x or 2.x",
            level_file.version
        ));
    }
    clear(world);
    // Objects are stored in DFS order so parents are always inserted first.
    let has_persisted_components = !level_file.components.is_empty();
    for obj in level_file.objects {
        let parent = obj.parent.clone();
        add_object(world, obj, parent);
    }

    // When present, persisted components are authoritative and replace defaults.
    if has_persisted_components {
        for (object_id, components) in level_file.components {
            while !get_components_metadata(world, &object_id).is_empty() {
                remove_component(world, &object_id, 0);
            }
            for component in components {
                add_component_instance(world, &object_id, component);
            }
        }
    }

    tracing::info!(
        "Scene loaded from: {} (version: {})",
        path.as_ref().display(),
        level_file.version
    );
    Ok(level_file.editor.and_then(|editor| editor.camera))
}
