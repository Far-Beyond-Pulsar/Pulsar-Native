//! `.level` file save/load, straight between the world and the file format.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use engine_fs::virtual_fs;
use pulsar_class::ClassRegistry;
use pulsar_scenedb::World;

use super::classes::{self, project_registry};
use super::{ComponentInstance, ObjectId, SceneObjectData};

use super::components::{
    add_component_instance, get_components, get_components_metadata, remove_component,
};
use super::objects::{add_object, clear, get_all_objects};
use super::{LevelEditorCameraState, LevelEditorFileState, LevelFile, LevelMetadata};
use crate::world_settings_data::WorldSettingsData;

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
    let settings = read_existing_world_settings(path.as_ref());
    save_with_classes_and_settings(world, path, editor_camera, settings, &project_registry())
}

/// Save a level with an explicit snapshot of its world settings.
pub fn save_to_file_with_settings<P: AsRef<Path>>(
    world: &World,
    path: P,
    editor_camera: Option<LevelEditorCameraState>,
    world_settings: WorldSettingsData,
) -> Result<(), String> {
    save_with_classes_and_settings(
        world,
        path,
        editor_camera,
        world_settings,
        &project_registry(),
    )
}

/// The objects and component lists a save writes.
///
/// Placed classes are saved by reference (#921): a class root keeps its
/// `ClassInstance` with overrides recomputed against the current class
/// defaults (only the differences), plus any component the user added;
/// the components built from class slots and the generated child objects
/// are left out, since loading rebuilds them from the class. User objects
/// parented under a generated child are saved under its nearest saved
/// ancestor.
pub(crate) fn level_contents(
    world: &World,
    registry: &ClassRegistry,
) -> (
    Vec<SceneObjectData>,
    HashMap<ObjectId, Vec<ComponentInstance>>,
) {
    let all = get_all_objects(world);
    let generated: HashSet<ObjectId> = all
        .iter()
        .filter(|obj| classes::is_generated_child(world, &obj.id))
        .map(|obj| obj.id.clone())
        .collect();
    let parent_of: HashMap<ObjectId, Option<ObjectId>> = all
        .iter()
        .map(|obj| (obj.id.clone(), obj.parent.clone()))
        .collect();
    let mut objects = Vec::with_capacity(all.len());
    let mut components = HashMap::new();
    for mut obj in all {
        if generated.contains(&obj.id) {
            continue;
        }
        while let Some(parent) = obj.parent.clone().filter(|p| generated.contains(p)) {
            obj.parent = parent_of.get(&parent).cloned().flatten();
        }
        let mut list = get_components(world, &obj.id);
        if classes::is_class_root(world, &obj.id) {
            let overrides = classes::current_overrides(world, &obj.id, registry);
            list.retain(|c| c.data.get(pulsar_class::SLOT_ID_KEY).is_none());
            for component in &mut list {
                if component.class_name == pulsar_class::CLASS_INSTANCE {
                    if let Some(instance) = &overrides {
                        component.data = instance.to_value();
                    }
                    // Only the class reference and overrides are data here.
                    if let Some(map) = component.data.as_object_mut() {
                        map.retain(|key, _| !key.starts_with("__"));
                    }
                }
            }
            // The persisted list is authoritative; don't duplicate the
            // class's component data inline.
            obj.component_instances = None;
            obj.props.remove("script_asset");
        }
        components.insert(obj.id.clone(), list);
        objects.push(obj);
    }
    (objects, components)
}

/// [`save_to_file_with_editor_camera`] with an explicit class registry.
pub(crate) fn save_with_classes<P: AsRef<Path>>(
    world: &World,
    path: P,
    editor_camera: Option<LevelEditorCameraState>,
    registry: &ClassRegistry,
) -> Result<(), String> {
    let settings = read_existing_world_settings(path.as_ref());
    save_with_classes_and_settings(world, path, editor_camera, settings, registry)
}

pub(crate) fn save_with_classes_and_settings<P: AsRef<Path>>(
    world: &World,
    path: P,
    editor_camera: Option<LevelEditorCameraState>,
    world_settings: WorldSettingsData,
    registry: &ClassRegistry,
) -> Result<(), String> {
    write_level(
        snapshot_level(world, registry, editor_camera, world_settings),
        path.as_ref(),
    )
}

fn read_existing_world_settings(path: &Path) -> WorldSettingsData {
    virtual_fs::read_file(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value.get("world_settings").cloned())
        .and_then(|settings| serde_json::from_value(settings).ok())
        .unwrap_or_default()
}

/// Everything a save needs from the world, captured in one pass.
///
/// Saving is split so the world is only needed for this snapshot: taking it
/// is pure in-memory work under the scene lock, while [`write_level`] (reading
/// the existing file, serializing, writing) needs no world and can run on any
/// thread with the lock released. See `ui::save`.
pub struct LevelSnapshot {
    objects: Vec<SceneObjectData>,
    components: HashMap<ObjectId, Vec<ComponentInstance>>,
    /// Resolved class name of each placed class instance, keyed by object id,
    /// for deciding which legacy blueprint bindings still need carrying over.
    class_names: HashMap<ObjectId, String>,
    editor_camera: Option<LevelEditorCameraState>,
    world_settings: WorldSettingsData,
}

/// Capture what [`write_level`] writes. `registry` should be the project's
/// classes (`classes::project_registry`, which scans the disk -- take it
/// before locking the world).
pub fn snapshot_level(
    world: &World,
    registry: &ClassRegistry,
    editor_camera: Option<LevelEditorCameraState>,
    world_settings: WorldSettingsData,
) -> LevelSnapshot {
    profiling::profile_scope!("scene_edit::snapshot_level");
    let (objects, components) = level_contents(world, registry);
    let class_names = objects
        .iter()
        .filter_map(|obj| {
            let instance = classes::class_instance(world, &obj.id)?;
            let name = registry
                .resolve(&instance)
                .map(|entry| entry.name.clone())
                .unwrap_or(instance.class_name);
            Some((obj.id.clone(), name))
        })
        .collect();
    LevelSnapshot {
        objects,
        components,
        class_names,
        editor_camera,
        world_settings,
    }
}

/// Write a snapshot to `path`. Needs no world or lock.
pub fn write_level(snapshot: LevelSnapshot, path: &Path) -> Result<(), String> {
    profiling::profile_scope!("scene_edit::write_level");
    let LevelSnapshot {
        objects,
        components,
        class_names,
        editor_camera,
        world_settings,
    } = snapshot;
    if let Some(parent_dir) = path.parent() {
        virtual_fs::create_dir_all(parent_dir)
            .map_err(|e| format!("Failed to create directory: {e}"))?;
    }
    let now = chrono::Utc::now().to_rfc3339();
    // Read the existing file once: its editor camera is preserved when no fresh
    // camera state was supplied, and its #650 blueprint-binding section always
    // rides along (the editor cannot author it yet, but a re-save must never
    // destroy it).
    let existing_file = virtual_fs::read_file(path)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|json: String| serde_json::from_str::<LevelFile>(&json).ok());
    let preserved_editor = if editor_camera.is_none() {
        existing_file.as_ref().and_then(|file| file.editor.clone())
    } else {
        None
    };
    // Legacy `blueprint_bindings` are no longer written: loading migrates
    // them to `ClassInstance`. Only entries the migration could not express
    // (a second class bound to one object) are carried over, so a re-save
    // never destroys them.
    let preserved_bindings = existing_file
        .map(|file| unmigrated_bindings(&class_names, file.blueprint_bindings))
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
        world_settings,
        editor: editor_camera
            .map(|camera| LevelEditorFileState {
                camera: Some(camera),
            })
            .or(preserved_editor),
    };
    let json = serde_json::to_string_pretty(&level_file)
        .map_err(|e| format!("Failed to serialize: {e}"))?;
    virtual_fs::write_file(path, json.as_bytes())
        .map_err(|e| format!("Failed to write file: {e}"))?;

    tracing::info!("Scene saved to: {}", path.display());
    Ok(())
}

/// Bindings of `bindings` not represented by a `ClassInstance` of the same
/// class on the bound object.
///
/// `class_names` is each placed instance's resolved class name
/// ([`LevelSnapshot::class_names`]).
fn unmigrated_bindings(
    class_names: &HashMap<ObjectId, String>,
    mut bindings: pulsar_scene::BlueprintBindings,
) -> pulsar_scene::BlueprintBindings {
    for (stable_id, entries) in bindings.iter_mut() {
        let Some(class_name) = class_names.get(stable_id.as_str()) else {
            // Object gone, or never migrated: nothing represents it.
            continue;
        };
        entries.retain(|binding| &binding.class_name != class_name);
    }
    bindings.retain(|_, entries| !entries.is_empty());
    bindings
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
    load_from_file_with_editor_camera_and_settings(world, path).map(|(camera, _)| camera)
}

/// Load a scene and return its persisted editor camera and world settings.
pub fn load_from_file_with_editor_camera_and_settings<P: AsRef<Path>>(
    world: &mut World,
    path: P,
) -> Result<(Option<LevelEditorCameraState>, WorldSettingsData), String> {
    load_with_classes_and_settings(world, path, &project_registry())
}

/// [`load_from_file_with_editor_camera`] with an explicit class registry.
///
/// Old class references are migrated first (`ScriptComponent` class paths
/// and `blueprint_bindings` become `ClassInstance`), then every placed class
/// is rebuilt from its current definition with its overrides applied.
pub(crate) fn load_with_classes<P: AsRef<Path>>(
    world: &mut World,
    path: P,
    registry: &ClassRegistry,
) -> Result<Option<LevelEditorCameraState>, String> {
    load_with_classes_and_settings(world, path, registry).map(|(camera, _)| camera)
}

pub(crate) fn load_with_classes_and_settings<P: AsRef<Path>>(
    world: &mut World,
    path: P,
    registry: &ClassRegistry,
) -> Result<(Option<LevelEditorCameraState>, WorldSettingsData), String> {
    profiling::profile_scope!("scene_edit::load_from_file");
    let bytes =
        virtual_fs::read_file(path.as_ref()).map_err(|e| format!("Failed to read file: {e}"))?;
    let json = String::from_utf8(bytes).map_err(|e| format!("File is not valid UTF-8: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| format!("Failed to parse JSON: {e}"))?;
    let report = pulsar_class::migrate::migrate_level_value(&mut value, registry);
    if report.changed() {
        tracing::info!(
            "Migrated {} ScriptComponent(s) and {} binding(s) to ClassInstance in {}",
            report.script_components.len(),
            report.bindings.len(),
            path.as_ref().display()
        );
    }
    let level_file: LevelFile =
        serde_json::from_value(value).map_err(|e| format!("Failed to parse JSON: {e}"))?;
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

    let unresolved = classes::rebuild_all_instances(world, registry);
    if !unresolved.is_empty() {
        tracing::warn!(
            "{} placed class instance(s) reference classes missing from this project: {:?}",
            unresolved.len(),
            unresolved
        );
    }

    tracing::info!(
        "Scene loaded from: {} (version: {})",
        path.as_ref().display(),
        level_file.version
    );
    Ok((
        level_file.editor.and_then(|editor| editor.camera),
        level_file.world_settings,
    ))
}

#[cfg(test)]
mod voxel_example_tests {
    use super::*;

    #[test]
    fn voxel_planet_example_loads_as_a_scenedb_backend_source() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../assets/examples/voxel_planet.level");
        let mut world = World::new();
        let camera = load_from_file_with_editor_camera(&mut world, path)
            .expect("example level loads")
            .expect("example camera is present");
        assert_eq!(camera.position, [0.0, 6_371_758.7, 0.0]);

        let (entries, errors) = engine_backend::scene::voxel_frame::project_voxel_entries(&world);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].renderer_id, "helio.voxel-terrain");
        assert_eq!(entries[0].generator.as_ref().unwrap().version, 1);
    }
}
